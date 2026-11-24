"""The JSON face of the cockpit, for a dashboard (document: docs/cockpit-api.md).

Pure documents over the same snapshot and the same controller as the pages: nothing here decides, runs or
stores anything. Every string passes through `strip_controls`; every time is ISO 8601 in UTC.
"""

from __future__ import annotations

import json
import shlex
import sys
from datetime import UTC, datetime
from functools import cache
from pathlib import Path
from typing import TYPE_CHECKING

from sakura import engine_home, keeper, repository
from sakura.accounts import display
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.automations import registry as automation_registry
from sakura.cockpit import (
    account_actions,
    actions,
    digest_reading,
    host_actions,
    lane_facts,
    leeway_view,
    proposal_actions,
    remote,
    track_actions,
    wait_actions,
)
from sakura.cockpit.accounts import AccountRow
from sakura.cockpit.actions import Target
from sakura.cockpit.gate import Identity
from sakura.cockpit.items import GroupRow, Item
from sakura.cockpit.snapshot import Snapshot
from sakura.connectors import contract as connector_contract
from sakura.connectors import registry as connector_registry
from sakura.digest import config as digest_config
from sakura.digest import run as digest_run
from sakura.digest import source_words
from sakura.flow import (
    ask_words,
    config,
    inbox,
    lane_ceiling,
    pace,
    pace_accounts,
    presets,
    rules,
    store,
    yield_person,
)
from sakura.flow.render import strip_controls
from sakura.flow.waits import Wait
from sakura.scheduled import definition as scheduled_definition
from sakura.scheduled import dispatch as scheduled_dispatch
from sakura.scheduled import migrate as scheduled_migrate
from sakura.scheduled import morning as scheduled_morning
from sakura.scheduled import store as scheduled_store
from sakura.setup import apply as setup_apply
from sakura.tracks.domain import RefusalCode as TrackRefusalCode
from sakura.usage import report

if TYPE_CHECKING:
    from sakura.cockpit.controller import Confirm, Controller, Outcome

API_VERSION = 1
PHONE_API_VERSION = 2
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
STATUS_OK, STATUS_BAD_REQUEST = 200, 400
FIELD_CONFIRM = "confirm"
APPROVE_KEY = "a"
NO_TITLE = "this lane"
PROVIDER_TIMES = ("period_start", "resets_at", "utilization_at")
# The snapshot account contract: a field added to AccountRow reaches every node only by being listed here.
ACCOUNT_FIELDS = ("provider", "id", "key", "label", "state", "enabled", "week_pct", "week_resets_at",
                  "five_hour_pct", "reserve_week_pct", "reserve_week_pct_own", "open_alerts")
STOP_AT_FULL = 100.0
KEEPER_UNREADABLE = "the keeper's state cannot be read"
CONNECTORS_UNREADABLE = "the connectors cannot be read"
DIGEST_UNREADABLE = "the digest settings cannot be read"
PRESET_UNUSABLE = "this preset file cannot be read"  # the engine's own reason can name a file: never sent to a client


class Api:
    """The three calls the server routes under /api/, over one controller."""

    def __init__(self, controller: Controller) -> None:
        self._controller = controller

    def session(self, identity: Identity, csrf: str) -> dict:
        """Who the gate recognized, and a CSRF token for that identity."""
        return {**session_document(identity, csrf), "api": PHONE_API_VERSION}

    def snapshot(self) -> dict:
        """The snapshot, from the few seconds old copy the pages share."""
        document = snapshot_document(self._controller.snapshot())
        return remote.document({**document, "api": PHONE_API_VERSION}, self._controller.remote_ids)

    def action(self, fields: dict, identity: Identity) -> tuple[int, dict]:
        """Decide one action request: the HTTP status and the result document."""
        from sakura.cockpit.controller import (
            PROOF_SOURCE_OVERLAY,
            Refused,
            overlay_actor,
        )

        outcome = self._controller.perform(self._controller.phone_fields(fields), overlay_actor(identity), proof_field=FIELD_CONFIRM,
                                           proof_source=PROOF_SOURCE_OVERLAY)
        denied = isinstance(outcome, Refused) and outcome.denied
        return (STATUS_BAD_REQUEST if denied else STATUS_OK), action_document(outcome, phone_key=fields.get("key", ""))


def target_id(target: Target) -> str:
    """The stable id of an item, group or account: its key as compact JSON. Both faces accept it as the target."""
    return json.dumps(list(target.key), separators=(",", ":"))


def dumps(document: dict) -> str:
    """The one serialization of a document, so the CLI and the server print the same bytes."""
    return json.dumps(document, ensure_ascii=True)


def session_document(identity: Identity, csrf: str) -> dict:
    """`GET /api/session`."""
    return _clean({"api": API_VERSION, "csrf": csrf,
                   "identity": {"login": identity.login, "node_id": identity.node_id, "device": identity.device}})



def scheduled_document() -> dict:
    """Read Scheduled and its engine-owned action targets."""
    envelope = {"tasks": [], "templates": scheduled_morning.templates(), "target": target_id(host_actions.SCHEDULED),
                "actions": _actions(host_actions.SCHEDULED)}
    try:
        storage = scheduled_store.Store()
        tasks = storage.tasks()
        migration = scheduled_migrate.status(storage)
        owns_digest = scheduled_migrate.owns_digest(storage)
        connectors = {item.id: item for item in connector_registry.load()} if tasks else {}
    except (scheduled_store.Refused, connector_registry.ConnectorError):
        return {**envelope, "actions": [], "problem": scheduled_store.STORE_PROBLEM}
    now = datetime.now(UTC)
    envelope["tasks"] = [_scheduled_task(task, connectors, now) for task in tasks
                         if not task.definition.get("legacy_digest") or owns_digest]
    if migration:
        envelope["problem"] = migration
    elif (storage.path.parent / "launchd-problem").exists():
        envelope["problem"] = "launchd_reconciliation_failed"
    return envelope


def _scheduled_task(task, connectors: dict, now: datetime) -> dict:
    state, reason, label = task.status(connectors)
    offered = _actions(task)
    if label is None and state == "connector_unavailable":
        missing = next((key for key in scheduled_definition.required_connectors(task.definition) if key not in connectors), None)
        label = task.definition.get("legacy_labels", {}).get(missing)
    if state == "connector_unavailable":
        offered = [action for action in offered if action["key"] not in (scheduled_definition.RUN, scheduled_definition.RESUME)]
    public = {key: task.definition[key] for key in scheduled_definition.REQUIRED | scheduled_definition.OPTIONAL.keys()}
    return {"id": task.id, "revision": task.revision, **public, "state": state,
            "reason": reason, "connector_label": label, "target": target_id(task), "actions": offered,
            "next_at": scheduled_dispatch.next_at(task, now) if state == "active" else None}


def _tracks(snapshot: Snapshot) -> dict:
    view = snapshot.tracks
    if view is None:
        return {}
    if view.problem:
        return {"tracks": None, "track_problem": view.problem}
    return {"tracks": view.rows, "track_actions": [track_actions.SPECS[track_actions.NEW].document(track_actions.NEW)]}


def _item_track(item: Item, snapshot: Snapshot) -> dict:
    if item.kind != "flow" or snapshot.tracks is None:
        return {}
    return {"track": snapshot.tracks.lane_ref(item.repo, item.key[-1])}


def _project_tracks(repo: Path, snapshot: Snapshot) -> dict:
    view = snapshot.tracks
    if view is None or view.problem or repo in snapshot.missing_projects:
        return {}
    return {"track_choices": view.choices}


def snapshot_document(snapshot: Snapshot) -> dict:
    """`GET /api/snapshot` and `sakura cockpit snapshot --json`."""
    deferred = [{**entry, "at": _utc(entry.get("at"))} for entry in snapshot.deferred]
    events = [{**event, "at": _utc(event.get("at"))} for event in snapshot.events]
    view = leeway_view.read(_actions)
    local_accounts = [_account(row) for row in snapshot.accounts if not view.is_available or row.provider == GOOSE]
    accounts = _named_accounts([*(view.accounts if view.is_available else []), *local_accounts])
    usage = _named_usage(view.usage if view.is_available else _usage(snapshot), accounts)
    engine_level = _engine_level()

    @cache
    def project_pace_accounts(repo: Path, folder_missing: bool) -> dict:
        return _pace_accounts(repo, accounts, folder_missing=folder_missing)

    return _clean({"api": API_VERSION, "mode": snapshot.mode, "deferred": deferred, "load": snapshot.load,
                   "usage": usage,
                   "accounts": accounts,
                   "problems": _problems(snapshot, view), "waits": [_wait(wait, snapshot) for wait in snapshot.waits],
                   "groups": [_group(row) for row in snapshot.groups],
                   "items": [{**_named_steps(_item(item), accounts), **_lane_pace(item, engine_level, snapshot),
                              **project_pace_accounts(item.repo, item.folder_missing), **_item_track(item, snapshot)}
                             for item in snapshot.items], "events": events, "host": _host(),
                   "projects": [{**_project(repo, snapshot), **project_pace_accounts(repo, repo in snapshot.missing_projects),
                                 **_project_tracks(repo, snapshot)}
                                for repo in snapshot.projects],
                   "decided": [{**entry, "at": _utc(entry.get("at"))} for entry in snapshot.decided],
                   "engine": _engine(snapshot), **_connectors_document(), "digest": _digest(),
                   "automations": _automations(), "scheduled": scheduled_document(), **_tracks(snapshot)})


def action_document(outcome: Outcome, *, phone_key: str | None = None) -> dict:
    """The answer to an action request: `confirm`, `done` or `refused`."""
    from sakura.cockpit.controller import Confirm, Done

    if isinstance(outcome, Confirm):
        if phone_key is not None:
            return _clean({"result": "confirm", "token": outcome.token, "expires": _utc_epoch(outcome.expires),
                           "summary": remote.text(ask_words.confirmation_summary(
                               outcome.action.label, _approval_subject(outcome) or NO_TITLE))})
        preview = {"result": "confirm", "token": outcome.token, "expires": _utc_epoch(outcome.expires),
                   "summary": _summary(outcome), "command": _command_line(outcome)}
        if outcome.display_args:
            preview["display_args"] = outcome.display_args
        return _clean(preview)
    if isinstance(outcome, Done):
        result = outcome.result if phone_key is None else remote.result(phone_key, outcome.result)
        done = {"result": "done", "exit_code": result.exit_code, "output": result.output}
        argv = result.invocation.argv
        if (phone_key is None and argv[1:2] == ["connectors"]
                and argv[2:3] in (["add"], ["edit"], ["revoke"], ["replace"])
                and (receipt := connector_contract.receipt(result.output))):
            done["connector"] = receipt
        return _clean({**done, **({"audit": result.audit} if result.audit else {})})
    is_track_refusal = outcome.reason_code in set(TrackRefusalCode) and (phone_key is None or phone_key == track_actions.SETTLE)
    reason = outcome.reason if phone_key is None or outcome.reason in remote.SAFE_REFUSALS or is_track_refusal else remote.REFUSED
    refusal = {"result": "refused", "reason": reason}
    code = outcome.reason_code or reason
    if is_track_refusal:
        refusal.update(reason_code=code, reason_args=outcome.reason_args or {})
    elif code in connector_contract.REASONS and (phone_key is None or code == "local_only"):
        refusal.update(reason_code=code, reason_args={})
    return _clean(refusal)


# Rows

def _item(item: Item) -> dict:
    return {"id": target_id(item), "key": list(item.key), "kind": item.kind, "repo": None if item.kind == digest_reading.KIND_DIGEST else str(item.repo),
            "worktree": str(item.worktree) if item.worktree else None, "branch": item.branch,
            "last_commit": item.last_commit, "stack": item.stack, "agent_state": item.agent_state, "pr": item.pr,
            "models": dict(item.models), "pending": list(item.pending), "usage": report.usage_json(item.usage),
            "next_command": item.next_command, "since": _utc(item.since), "last_activity_at": _utc(item.last_activity_at),
            "progress": _progress(item), "delivery": item.delivery, "title": item.title, "author": item.author,
            "links": [{"label": label, "url": url} for label, url in item.links], "unread": item.unread,
            "folder_missing": item.folder_missing,
            "actions": _actions(item), **item.facts}


def _progress(item: Item) -> dict | None:
    found = item.progress
    if found is None:
        return None
    if item.kind == "flow":
        return lane_facts.step_progress(found)
    return {"done": found.done, "total": found.total, "phase": found.phase}


def _wait(wait: Wait, view: Snapshot | None = None) -> dict:
    target = view.wait_target(wait) if view else wait_actions.target_of(wait)
    date = digest_reading.date_of_wait(wait)
    project, lane_title = ("", digest_reading.DIGEST_TITLE) if date else (wait.project, wait.lane_title)
    kind = wait_actions.inbox_kind(wait)
    proposal = {"proposal": proposal_actions.defaults(wait, target.projects, target.missing_projects)} if kind == "proposal" else {}
    track = {"track": wait.track} if wait.track else {}
    return {"id": target_id(target), "key": list(target.key), "kind": kind, **track, "subkind": wait.kind, "repo": wait.repo, **proposal,
            "group": wait_actions.inbox_group(wait), "section": wait_actions.inbox_section(wait),
            "resolution": wait_actions.OPEN, "project": project, "task": wait.task, "lane_title": lane_title, "digest": date, "title": wait.title,
            "detail": wait.detail, "technical": wait.technical, "findings": list(wait.findings), "since": _utc(wait.since), "source": wait.source, "author": wait.author,
            "links": [{"label": label, "url": url} for label, url in wait.links],
            "choices": list(wait.choices), "is_read": wait.is_read,
            "is_snoozed": wait.is_snoozed, "snoozed_until": _utc(wait.snoozed_until),
            "wants_note": wait.wants_note,
            "folder_missing": wait.folder_missing,
            "actions": _actions(target)}


def _group(row: GroupRow) -> dict:
    return {"id": target_id(row), "key": list(row.key), "name": row.name, "repo": str(row.repo),
            "session": row.session, "lanes": row.lanes, "problem": row.problem, "next_command": row.next_command,
            "actions": _actions(row)}


def _host() -> dict:
    """This machine's own controls (`G` Digest now), so a dashboard can draw them: a fixed target, never the snapshot's."""
    return {"id": target_id(host_actions.DIGEST), "key": list(host_actions.DIGEST.key),
            "actions": _actions(host_actions.DIGEST)}


def _engine_rules() -> dict:
    """The engine's own rules (`K`, `U`, `V`): the preset in force, whether it has unsaved changes, and the fixed target.

    A config that does not load answers with no preset (the problem is in `problems`), never an error.
    """
    target = host_actions.ENGINE
    document = {"id": target_id(target), "key": list(target.key), "preset": "", "preset_label": "", "edited": False,
                "pace": None, "target_pct": None, "lanes_min": None, "lanes": None, "pace_accounts": {},
                "presets": [], "actions": _actions(target)}
    try:
        described = rules.describe(None)
        loaded = config.load(None)
    except (config.FlowConfigError, engine_home.EngineHomeError):
        return {**document, "presets": _preset_choices("")}
    knobs = loaded.values["knobs"]
    return {**document, "preset": described["preset"], "preset_label": described["preset_label"],
            "edited": described["edited"], "pace": knobs["pace"],
            "target_pct": pace.TARGETS[knobs["pace"]], "lanes_min": knobs["lanes_min"], "lanes": knobs["lanes"],
            "pace_accounts": described["pace_accounts"], "presets": _preset_choices(described["preset"])}


def _preset_choices(chosen: str) -> list[dict]:
    """The presets a person may pick, `chosen` marked: the legacy names are left out unless one of them is in force."""
    try:
        return [{"name": entry["name"], "label": entry["label"], "chosen": entry["chosen"],
                 "description": entry["description"] if entry["usable"] else PRESET_UNUSABLE}
                for entry in presets.listing(chosen) if not entry["legacy"] or entry["chosen"]]
    except engine_home.EngineHomeError:
        return []


def _problems(snapshot: Snapshot, view: leeway_view.LeewayView) -> list[str]:
    """The snapshot's problems, why Leeway's usage is missing when it is, and the settings the engine reads but ignores
    (a retired knob a file still sets)."""
    try:
        warnings = config.load(None).warnings
    except (config.FlowConfigError, engine_home.EngineHomeError):
        warnings = []
    return [*snapshot.problems, *([view.problem] if view.problem else []), *warnings]


def _named_accounts(accounts: list[dict]) -> list[dict]:
    """The accounts with the names a person reads: `label` (Leeway's, else words made from the id), `plan_label` and
    the label of every plan the provider offers, next to each plan id (`accounts/display.py`)."""
    return [{**row, "label": display.account_label(row["id"], str(row["label"] or ""), row["provider"]),
             "plan_label": display.plan_label(row["provider"], row.get("plan")),
             "plans": [{**period, "label": display.plan_label(row["provider"], period["plan"])} for period in row["plans"]],
             "plan_choice_labels": {plan: display.plan_label(row["provider"], plan) for plan in ADAPTERS[row["provider"]].plan_choices}}
            for row in accounts]


def _named_usage(usage: dict, accounts: list[dict]) -> dict:
    """The usage with the account's name next to each account id (Leeway's usage names the account of each row)."""
    names = {row["id"]: row["label"] for row in accounts}

    def named(row: dict) -> dict:
        if "account" not in row:
            return row
        return {**row, "account_label": names.get(row["account"]) or display.account_label(row["account"], "", row["provider"])}

    return {**usage, "providers": [named(row) for row in usage["providers"]]}


def _named_steps(document: dict, accounts: list[dict]) -> dict:
    """A lane's steps and reviews with the name of the account each runs on next to its id; none for the project's own."""
    names = {row["id"]: row["label"] for row in accounts}

    def named(entry: dict) -> dict:
        found = entry.get("account")
        return {**entry, "account_label": (names.get(found) or display.account_label(found)) if found else None}

    return {**document, **{group: [named(entry) for entry in document[group]]
                           for group in ("steps", "reviews") if group in document}}


def _lane_pace(item: Item, engine_level: str, snapshot: Snapshot) -> dict:
    """A flow lane's pace, one of the five levels (`pace.LANE_LEVELS`), and `pace_pending`, whether the person's choice of it
    waits for the lane's next step: the pace is the one the person chose as soon as `flow pace` accepted it. Its own, else the
    engine's `engine_level`; null for anything that is not a flow lane (never pending). It is read from the lane's state here
    too, so a lane whose config cannot be read still shows it."""
    if item.kind != "flow":
        return {"pace": None, "pace_pending": False}
    try:
        task = store.TaskStore(item.repo, item.key[2], folder_missing=item.folder_missing)
        state = snapshot.flow_state(task)
        chosen = inbox.pending_pace(task, snapshot.flow_pending(task))
    except (store.StateError, OSError, repository.RepositoryError):
        return {"pace": None, "pace_pending": False}
    current = lane_facts.pace_of(state, engine_level)
    return {"pace": chosen or current, "pace_pending": bool(chosen) and chosen != current}


def _engine_level() -> str:
    """The engine's pace now; the default level when its config cannot be read."""
    try:
        return config.load(None).values["knobs"]["pace"]
    except (config.FlowConfigError, engine_home.EngineHomeError):
        return pace.DEFAULT_LEVEL


def _pace_accounts(repo: Path, accounts: list[dict], *, folder_missing: bool = False) -> dict:
    """Every account of the snapshot with whether pace may move a step of `repo`'s project to it (`flow/pace_accounts.py`);
    a project that cannot be read allows none."""
    try:
        allowed = [] if folder_missing else pace_accounts.allowed(repo, None)
    except (repository.RepositoryError, engine_home.EngineHomeError):
        allowed = []
    return {"pace_accounts": [{"id": row["id"], "label": row["label"], "pace_allowed": row["id"] in allowed}
                              for row in accounts if row["provider"] != GOOSE]}


def _project(repo: Path, snapshot: Snapshot) -> dict:
    """A registered project, with or without lanes: the target of starting a lane and how many of its lanes are open."""
    target = snapshot.project_target(repo)
    offered = _actions(target)
    if not _project_tracks(repo, snapshot).get("track_choices"):
        for action in offered:
            if action["key"] == "L":
                action["optional"] = [name for name in action["optional"] if name not in track_actions.START_INPUTS]
    open_lanes = sum(1 for item in snapshot.items if item.kind == "flow" and item.repo == repo
                     and item.status not in lane_ceiling.FINISHED_STATUSES)
    return {"id": target_id(target), "key": list(target.key), "name": repo.name, "folder": str(repo),
            "folder_missing": target.folder_missing, "branch": "" if target.folder_missing else _checked_out_branch(repo),
            "open_lanes": open_lanes, "actions": offered}


_BRANCH_REF = "ref: refs/heads/"


def _checked_out_branch(repo: Path) -> str:
    """The branch the project's folder has checked out, read from its HEAD file with no Git process: a snapshot
    runs often and Git can be slow on a large repository. Empty on a detached head or an unreadable folder."""
    dot_git = repo / ".git"
    try:
        if dot_git.is_file():
            pointer = dot_git.read_text(encoding="utf-8").strip().removeprefix("gitdir:").strip()
            dot_git = (repo / pointer).resolve()
        head = (dot_git / "HEAD").read_text(encoding="utf-8").strip()
    except (OSError, UnicodeDecodeError):
        return ""
    return head.removeprefix(_BRANCH_REF) if head.startswith(_BRANCH_REF) else ""


def _engine(snapshot: Snapshot) -> dict:
    """The engine's own rules (`K`, `U`, `V`, `Q`) and its background work: the keeper, whether lanes give priority to
    the person's sessions and whether any waits for them now (`Y`); one fixed target, one list of actions."""
    default = setup_apply.master_account()
    launch_route = "acp" if default and default["provider"] == "goose" else "legacy"
    return {**_engine_rules(), **({"master_account": default} if default else {}), "master_launch_route": launch_route,
            "keeper": _keeper(), "yield_to_person": yield_person.is_enabled(),
            "yielding": _is_yielding(snapshot), "target": target_id(host_actions.ENGINE)}


def _is_yielding(snapshot: Snapshot) -> bool:
    """Whether any lane waits for the person to stop working with an agent elsewhere (`flow/yield_person.py`)."""
    for item in snapshot.items:
        try:
            if (item.kind == "flow" and not item.folder_missing
                    and snapshot.flow_state(store.TaskStore(item.repo, item.key[2])).get(yield_person.YIELD_KEY)):
                return True
        except (store.StateError, OSError, repository.RepositoryError):
            continue  # a lane whose state cannot be read shows as blocked by itself
    return False


def _keeper() -> dict:
    try:
        found = keeper.status(datetime.now(UTC))
    except OSError as error:
        print(f"cockpit: {strip_controls(str(error))}", file=sys.stderr)  # holds a home path: never sent to a client
        found = {"on": False, "since": None, "last_restart": None, "needs_person": [], "phone_page": False,
                 "problem": KEEPER_UNREADABLE}
    last = found["last_restart"]
    return {**found, "since": _utc(found["since"]), "last_restart": last and {**last, "at": _utc(last["at"])},
            "target": target_id(host_actions.SERVICES), "actions": _actions(host_actions.SERVICES)}


def _connectors_document() -> dict:
    """The connectors as the last check left them, or none and a fixed sentence when their file cannot be read."""
    target = host_actions.CONNECTORS
    controls = {"version": connector_contract.VERSION, "target": target_id(target),
                "actions": [row for row in _actions(target) if row["key"] in connector_contract.ACTIONS]}
    try:
        return {"connector_controls": controls, "connectors": connector_registry.rows(versioned=True)}
    except connector_registry.ConnectorError:
        return {"connector_controls": controls, "connectors": [], "connectors_problem": CONNECTORS_UNREADABLE}


def _digest() -> dict:
    """The digest's settings and sources with each source's last good run, and the controls that change them."""
    controls = {"target": target_id(host_actions.DIGEST_SETTINGS), "actions": _actions(host_actions.DIGEST_SETTINGS),
                "connect": {"target": target_id(host_actions.CONNECTORS),
                            "actions": [row for row in _actions(host_actions.CONNECTORS) if row["key"] in ("O", "o")]}}
    try:
        if scheduled_migrate.owns_digest():
            return {**controls, "actions": [], "problem": scheduled_migrate.MIGRATED}
        settings = digest_config.load_or_default()
        found = digest_run.status(settings, datetime.now(UTC))
    except (digest_config.ConfigError, scheduled_store.Refused, OSError) as error:
        print(f"cockpit: {strip_controls(str(error))}", file=sys.stderr)  # holds a home path: never sent to a client
        return {**controls, "problem": DIGEST_UNREADABLE}
    last_good = dict(found.sources)
    return {**controls, "at": settings.at, "lookback_hours": settings.lookback_hours, "language": settings.language,
            "model": settings.model, "account": settings.account, "next_at": _utc(found.next_at.isoformat()),
            "last_run_at": _utc(found.last_run_at), "sources": [
                {"id": source.id, "label": source_words.label_of(source), "reads": source_words.reads_of(source),
                 "kind": _source_kind(source), "connector": source.connector or None,
                 "enabled": source.enabled, "last_good_run": _utc(last_good.get(source.id))}
                for source in settings.sources]}


def _automations() -> dict:
    """The automations (definitions and the always-on services) and the control that switches them."""
    try:
        items = automation_registry.rows()
    except automation_registry.AutomationRefused:
        items = []
    return {"items": items, "target": target_id(host_actions.AUTOMATIONS),
            "actions": _actions(host_actions.AUTOMATIONS)}


def _source_kind(source: digest_config.Source) -> str:
    return "connector" if source.connector else "feed" if source.feed else "program"


def _actions(target: Target) -> list[dict]:
    return [{"key": action.key, "label": action.label, "needs": list(action.needs(target)),
             "optional": list(action.optional), "confirm": action.confirm} for action in actions.applicable(target)]


def _usage(snapshot: Snapshot) -> dict:
    document = report.week_json(snapshot.usage)
    document["now"] = _utc(document["now"])
    for provider in document["providers"]:
        for name in PROVIDER_TIMES:
            provider[name] = _utc(provider[name])
        if provider["projection"]:
            provider["projection"]["limit_at"] = _utc(provider["projection"]["limit_at"])
    return document


def _account(row: AccountRow) -> dict:
    target = account_actions.target_of(row)
    reserve = row.reserve_week_pct
    goose_fields = {"provider_config": row.provider_config, "readiness": row.readiness, "reason": row.reason,
                    "forecast_available": False} if row.provider == GOOSE else {}
    return {**{name: getattr(row, name) for name in ACCOUNT_FIELDS}, "week_resets_at": _utc(row.week_resets_at),
            "stop_at_pct": None if reserve is None else round(STOP_AT_FULL - reserve, 6),
            "plans": [{"plan": plan, "since": since} for plan, since in row.plans],
            "plan_choices": list(ADAPTERS[row.provider].plan_choices),
            "target": target_id(target), "actions": _actions(target) if row.is_writable else [], **goose_fields}


def _summary(outcome: Confirm) -> str:
    """What the person confirms. An approval is said in the words of the ask they read (`flow/ask_words.py`), with no
    gate, revision or lane id; the command that runs is in its own field (`_command_line`)."""
    if outcome.action.key in connector_contract.ACTIONS:
        return outcome.action.label
    subject = _approval_subject(outcome)
    if subject:
        return ask_words.confirmation_summary(outcome.action.label, subject)
    if isinstance(outcome.target, account_actions.AccountTarget):  # no install path, no working directory
        return f"{outcome.action.label} on {outcome.target.key[-1]}: {account_actions.shown_command(outcome.invocation.argv)}"
    command = shlex.join(outcome.invocation.argv)
    return f"{outcome.action.label} on {outcome.target.key[-1]}: run in {outcome.invocation.cwd}: {command}"


def _approval_subject(outcome: Confirm) -> str:
    """The title of the ask an answer to a wait is about, or the title of the lane an action on its card is about (never
    its folder name or its command); empty for any other action."""
    target = outcome.target
    if isinstance(target, scheduled_store.ScheduledTask):
        return target.definition["title"]
    if isinstance(target, wait_actions.WaitTarget):
        return target.wait.title
    if isinstance(target, Item) and target.kind == actions.KIND_FLOW:
        return target.title or target.branch or NO_TITLE
    if outcome.action.key == APPROVE_KEY and isinstance(target, Item):
        return target.title or target.branch or NO_TITLE
    return ""


def _command_line(outcome: Confirm) -> str:
    """The command the confirmation runs, for a reader that wants it: kept out of the summary."""
    if isinstance(outcome.target, account_actions.AccountTarget):
        return account_actions.shown_command(outcome.invocation.argv)
    return shlex.join(actions.public_argv(outcome.action.key, outcome.invocation))


# Values

def _utc(value: object) -> str | None:
    """An ISO 8601 time as UTC with a Z; a time without a zone is taken as UTC, empty or missing is None, and text
    that is no time stays as it was."""
    if not value:
        return None
    try:
        moment = datetime.fromisoformat(str(value))
    except ValueError:
        return str(value)
    if moment.tzinfo is None:
        moment = moment.replace(tzinfo=UTC)
    return moment.astimezone(UTC).strftime(TIME_FORMAT)


def _utc_epoch(seconds: int) -> str:
    return datetime.fromtimestamp(seconds, UTC).strftime(TIME_FORMAT)


def _clean(value):
    if isinstance(value, str):
        return strip_controls(value)
    if isinstance(value, dict):
        return {_clean(key): _clean(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_clean(item) for item in value]
    return value
