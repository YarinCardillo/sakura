"""Track action descriptors shared by the snapshot and cockpit dispatch."""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from sakura import actors, engine_home, text_limits
from sakura.flow import store, waits
from sakura.flow.tmux import paste_refusal
from sakura.tracks import TrackService
from sakura.tracks.domain import (
    ANSWER_LIMIT,
    CONVERSATION_PART_LIMIT,
    DECISION_LIMIT,
    GOAL_LIMIT,
    NAME_LIMIT,
    OPERATION_LIMIT,
    SUMMARY_LIMIT,
    TITLE_LIMIT,
    ConversationKey,
    Refusal,
    RefusalCode,
)
from sakura.tracks.starts import LaunchInputs, TrackStarts

if TYPE_CHECKING:
    from sakura.cockpit.project_actions import ProjectTarget
    from sakura.cockpit.wait_actions import WaitTarget

COLLECTION = "tracks"
RECORD = "track"
NEW, SET, DECISION, SETTLE = "tracknew", "trackset", "trackdecision", "tracksettle"
LINK, UNLINK, RETRY, REMOVE = "tracklink", "trackunlink", "trackretry", "trackremove"


@dataclass(frozen=True)
class Spec:
    """The label and input schema for a Track action."""

    label: str
    needs: tuple[str, ...]
    optional: tuple[str, ...] = ()

    def document(self, key: str) -> dict:
        """Serialize the action using the cockpit v1 descriptor shape."""
        return {"key": key, "label": self.label, "needs": list(self.needs),
                "optional": list(self.optional), "confirm": False}


SPECS = {
    NEW: Spec("New Track", ("name", "goal", "op"), ("project",)),
    SET: Spec("Edit Track", ("op",), ("name", "goal", "summary", "status", "clear")),
    DECISION: Spec("Raise decision", ("text", "op")),
    SETTLE: Spec("Answer decision", ("decision", "answer", "op")),
    LINK: Spec("Link to Track", ("op",), ("project", "task", "conversation", "title")),
    UNLINK: Spec("Unlink from Track", ("op",), ("project", "task", "conversation")),
    RETRY: Spec("Retry lane start", ("startop", "op")),
    REMOVE: Spec("Remove lane reservation", ("startop", "op")),
}
WAIT_SETTLE = Spec(SPECS[SETTLE].label, ("answer", "op"))
START_INPUTS = ("track", "op")
PHONE_FORM_QUERY = "track-decision"
RECOVERY_KEYS = {"retry": RETRY, "remove": REMOVE}


def record_keys(status: str, has_open_decisions: bool) -> tuple[str, ...]:
    """Offer changes allowed by the record's lifecycle, with close checked again on write."""
    return (SET, UNLINK, *((DECISION, LINK) if status != "closed" else ()),
            *((SETTLE,) if has_open_decisions else ()))


@dataclass(frozen=True)
class Target:
    """A collection or a Track resolved by the engine, with its project registry."""

    track_id: str | None = None
    projects: tuple[Path, ...] = ()
    offered: tuple[str, ...] = ()

    @property
    def kind(self) -> str:
        return COLLECTION if self.track_id is None else RECORD

    @property
    def key(self) -> tuple[str, ...]:
        return (COLLECTION,) if self.track_id is None else (RECORD, self.track_id)


MULTILINE = {"goal": GOAL_LIMIT, "summary": SUMMARY_LIMIT, "text": DECISION_LIMIT, "answer": ANSWER_LIMIT}
ONE_LINE = {"name": NAME_LIMIT, "title": TITLE_LIMIT, "op": OPERATION_LIMIT, "startop": OPERATION_LIMIT,
            "project": 1024, "task": store.MAX_TASK_ID_LENGTH, "status": 6, "clear": 7, "decision": 19}
CONVERSATION_FIELDS = {"provider", "account", "session"}
CONVERSATION_WIRE_LIMIT = len(CONVERSATION_FIELDS) * (CONVERSATION_PART_LIMIT * 12 + 20)
ID_PATTERN = re.compile(r"t-[0-9a-f]{8}")
DECISION_PATTERN = re.compile(r"[1-9][0-9]{0,18}")
STORAGE_PROBLEM = "Track storage is unavailable. Try again after refreshing."


def is_tracked_start(key: str, inputs: dict) -> bool:
    """A project start opts into reservations by supplying both Track inputs."""
    return key == "L" and any(part in inputs for part in START_INPUTS)


def is_decision_wait(key: tuple) -> bool:
    """Recognize the canonical Track decision key without contacting a project."""
    return (len(key) == 5 and key[:2] == ("wait", waits.TRACK_SOURCE) and key[3] == waits.TRACK_DECISION
            and bool(ID_PATTERN.fullmatch(key[2]) and DECISION_PATTERN.fullmatch(key[4])))


def require_person(actor: actors.Actor) -> None:
    """Person transports are authorized before looking up operation replays."""
    if actor.source not in actors.PERSON_SOURCES:
        raise Refusal(RefusalCode.FORBIDDEN, "Only the person can perform this Track action")


def validate(key: str, target_kind: str, inputs: dict) -> None:
    """Validate the shared desktop, phone and CLI inputs before dispatch."""
    spec = WAIT_SETTLE if key == SETTLE and target_kind == "wait" else SPECS[key]
    names = inputs.keys()
    if names - set(spec.needs + spec.optional) or set(spec.needs) - names:
        _invalid("Use the required inputs for this Track action")
    for name, value in inputs.items():
        if problem := input_problem(name, value):
            _invalid(f"Invalid {name}: {problem}")
    if key == SET:
        edits = names - {"op"}
        if not edits or ("clear" in edits and (inputs["clear"] != "summary" or "summary" in edits)):
            _invalid("Supply a change. Use clear=summary on its own or with changes to other fields")
    if key in (LINK, UNLINK):
        _link_inputs(key, names - {"op"})


def input_problem(name: str, value: object) -> str | None:
    """Apply a Track field's own length and line rules."""
    if not isinstance(value, str):
        return "must be text"
    if name in MULTILINE:
        limit = MULTILINE[name]
        if len(value) > limit:
            return f"the limit is {limit} characters"
        if name == "summary" and not value.strip():
            return paste_refusal(value) if value else None
        return text_limits.text_problem(value, limit)
    if name == "conversation":
        _conversation(value)
        return None
    return text_limits.one_line_problem(value, ONE_LINE[name])


def validate_start(inputs: dict) -> None:
    """Require the complete reservation identity when either tracked L input is supplied."""
    if not all(name in inputs for name in START_INPUTS):
        _invalid("Supply track and op together")
    if not isinstance(inputs["track"], str) or not ID_PATTERN.fullmatch(inputs["track"]):
        _invalid("Invalid Track id")
    if problem := input_problem("op", inputs["op"]):
        _invalid(f"Invalid op: {problem}")


def mutate(key: str, target: Target | WaitTarget, inputs: dict, actor: actors.Actor) -> dict:
    """Call the service for one validated person mutation and encode its result."""
    require_person(actor)
    service = TrackService()
    fields = dict(inputs)
    op_id = fields.pop("op")
    if key == SETTLE and target.kind == "wait":
        value = service.settle_decision(target.key[2], _number(target.key[4]), fields["answer"],
                                        actor=actor, op_id=op_id, replay_only=target.is_replay_only)
    else:
        value = _mutate_record(service, key, target, fields, actor, op_id)
    return asdict(value)


def _mutate_record(service, key, target, fields, actor, op_id):
    ids = {"actor": actor, "op_id": op_id}
    track_id = target.track_id
    if key == NEW:
        project = _project(target, fields.pop("project")) if "project" in fields else None
        return service.create(**fields, **ids, project=project)
    if key == SET:
        if fields.pop("clear", None):
            fields["summary"] = ""
        return service.update(track_id, **fields, **ids)
    if key == DECISION:
        return service.add_decision(track_id, **fields, **ids)
    if key == SETTLE:
        return service.settle_decision(track_id, _number(fields["decision"]), fields["answer"], **ids)
    if key == REMOVE:
        return TrackStarts(service).remove(fields["startop"], **ids, track_id=track_id)
    return _link(service, key, target, fields, ids)


def launch(key: str, target: Target | ProjectTarget, inputs: dict, actor: actors.Actor, run: Callable) -> str:
    """Start or recover a reservation using the cockpit's process runner."""
    def dispatch(argv, *, cwd, env, stdin, timeout, **kwargs):
        return run(argv, cwd=cwd, env=env, stdin=stdin, timeout=timeout)

    starts = TrackStarts()
    if key == RETRY:
        require_person(actor)
        return starts.retry(target.track_id, inputs["startop"], actor=actor, op_id=inputs["op"], run=dispatch)
    project = engine_home.project_id_for_path(target.repo)
    request = LaunchInputs(inputs["text"], inputs.get("preset"), inputs.get("level"))
    saved = starts.reserve(inputs["track"], project, request, actor, inputs["op"])
    return starts.launch(saved.op_id, actor, run=dispatch)


def _project(target, value):
    folder = Path(value)
    if not folder.is_absolute() or folder not in target.projects:
        raise Refusal(RefusalCode.NOT_FOUND, "Choose a registered project")
    return folder


def _number(value):
    if not DECISION_PATTERN.fullmatch(value):
        _invalid("Use a positive decimal decision id")
    return int(value)


def _link_inputs(key, names):
    conversation = {"conversation", "title"} if key == LINK else {"conversation"}
    if names not in ({"project", "task"}, conversation):
        _invalid("Use either a project and task or a conversation key with a title when linking")


def _link(service, key, target, fields, ids):
    if "conversation" in fields:
        conversation = _conversation(fields["conversation"])
        if key == LINK:
            return service.link_conversation(target.track_id, conversation, fields["title"], **ids)
        return service.unlink_conversation(target.track_id, conversation, **ids)
    method = service.link_lane if key == LINK else service.unlink_lane
    return method(target.track_id, _project(target, fields["project"]), fields["task"], **ids)


def _conversation(value):
    if len(value) > CONVERSATION_WIRE_LIMIT:
        _invalid("The conversation key is too long")
    try:
        parts = json.loads(value)
    except (ValueError, RecursionError):
        _invalid("Use a conversation key with provider, account and session")
    if (not isinstance(parts, dict) or parts.keys() != CONVERSATION_FIELDS
            or any(not isinstance(part, str) or not part.strip()
                   or text_limits.one_line_problem(part, CONVERSATION_PART_LIMIT) for part in parts.values())):
        _invalid("Use a bounded conversation key with provider, account and session")
    return ConversationKey(**parts)


def _invalid(reason):
    raise Refusal(RefusalCode.INVALID_INPUT, reason)
