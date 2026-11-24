"""The cockpit's one table of actions: each runs a single engine command in the repository of its target.

The table is the authorization layer. `run` looks the key up, checks the target's kind and the inputs, and only
then spawns; every attempt, done or refused, is appended to the cockpit action log with the full actor.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import signal
import sqlite3
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import asdict, dataclass, replace
from datetime import UTC, datetime
from functools import partial
from pathlib import Path

from sakura import actors, engine_home, programs, text_limits
from sakura.accounts import goose
from sakura.accounts import registry as account_registry
from sakura.accounts.providers import GOOSE
from sakura.cockpit import (
    account_actions,
    account_login,
    account_removal,
    attention,
    background_actions,
    decided,
    host_actions,
    outside_actions,
    project_actions,
    proposal_actions,
    review_waits,
    settings,
    track_actions,
    wait_actions,
)
from sakura.cockpit.account_actions import KIND_ACCOUNT, AccountTarget
from sakura.cockpit.host_actions import HostTarget
from sakura.cockpit.items import GroupRow, Item
from sakura.cockpit.project_actions import KIND_PROJECT, ProjectTarget
from sakura.cockpit.wait_actions import KIND_WAIT, WaitTarget
from sakura.connectors import contract as connector_contract
from sakura.flow import (
    ask_words,
    checks,
    config,
    exits,
    lane_ceiling,
    pace,
    pace_accounts,
    presets,
    resume,
    store,
    tasks,
    waits,
)
from sakura.flow.render import strip_controls
from sakura.flow.surface import KIT_BIN as PROGRAM
from sakura.flow.surface import UNFORWARDED_VARIABLES
from sakura.lanes.identity import NAME as LANE_NAME
from sakura.lanes.input import MAX_TEXT, MODEL_NAME
from sakura.scheduled import definition as scheduled_definition
from sakura.scheduled import dispatch as scheduled_dispatch
from sakura.scheduled import inbox as scheduled_inbox
from sakura.scheduled import launchd as scheduled_launchd
from sakura.scheduled import store as scheduled_store
from sakura.setup import apply as setup_apply
from sakura.tracks.database import SchemaError
from sakura.tracks.domain import Refusal as TrackRefusal
from sakura.tracks.domain import RefusalCode as TrackRefusalCode
from sakura.usage import leeway

ACTION_LOG = Path("cockpit") / "actions.jsonl"
ACTION_TIMEOUT_SECONDS = 120
GROUP_GRACE_SECONDS = 3
GROUP_POLL_SECONDS = 0.05
MAX_LINE = 200
MAX_FOLDER = 1024
GOOSE_ACCOUNT_ACTIONS = frozenset({"l", "n", "e", "X", "accountcheck"})
PROJECT_FOLDER_MISSING = "The project folder is missing. Restore it and try again."
LOG_FILE_MODE, LOG_DIRECTORY_MODE = 0o600, 0o700
ROLE_NAME = re.compile(r"[a-z][a-z0-9_]{0,39}")
VERDICT = re.compile("ok|ko")
REMEMBER = re.compile("yes")
DIAL_STEP = re.compile(r"[a-z0-9_-]{1,40}")
RULE_PATTERN = re.compile(r"[a-z_]+(\.[a-z_]+){0,2}")
LEVEL_PATTERN = re.compile(r"[a-z]{1,12}")
_ACCOUNT_LIST_ITEM = pace_accounts.ACCOUNT_ID.pattern
ACCOUNT_LIST_PATTERN = re.compile(rf"({_ACCOUNT_LIST_ITEM}(,{_ACCOUNT_LIST_ITEM}){{0,{pace_accounts.MAX_ACCOUNTS - 1}}})?")
APPROVED_GATE = "spec"
SEPARATOR = "--"
RESUME_COMMAND = "sakura flow resume"
RESUME_FAILED_TEXT = "the engine could not resume it"
APPROVE_FAILED_TEXT = "the engine did not take the approval"
RESTORE_FAILED_TEXT = "the engine could not put the workspace back"
START_FAILED_TEXT = "the engine did not start the lane"
DIGEST_FAILED_TEXT = "the digest could not be started"
BACKGROUND_FAILED_TEXT = "the engine did not take the change"
ADD_PROJECT_FAILED_TEXT = "the project was not added"
STATUS_AWAITING = waits.STATUS_AWAITING
# What each action key decides when it answers an ask; `k` and `r` say more (the verdict, a choice or text).
DECISION_ANSWERS = {"a": "approve", "y": "accept", "N": "reject", "s": "stop", "C": "close"}
KIND_FLOW, KIND_LANE, KIND_GROUP, KIND_OUTSIDE = "flow", "lane", "group", outside_actions.KIND_OUTSIDE

Target = Item | GroupRow | AccountTarget | WaitTarget | ProjectTarget | HostTarget | scheduled_store.ScheduledTask | track_actions.Target


class ActionRefused(ValueError):
    """Raised, before anything is spawned, when an action does not apply or its inputs are not acceptable."""


@dataclass(frozen=True)
class Invocation:
    """One engine command: the argument list (no shell) and the repository it runs in."""

    argv: list[str]
    cwd: Path
    handler: Callable[[actors.Actor], tuple[int, str]] | None = None  # set: runs in this process, argv only describes it


@dataclass(frozen=True)
class ActionResult:
    """What an attempt did: the exact command that ran, its exit code and its output without terminal controls.

    `audit` is empty, or says why the action log row of this run could not be written.
    """

    invocation: Invocation
    exit_code: int
    output: str
    audit: str = ""


@dataclass(frozen=True)
class Action:
    """A control: the target kinds it applies to, the inputs it needs, whether it asks first, and its command."""

    key: str
    label: str
    kinds: frozenset
    needs: Callable[[Target], tuple[str, ...]]
    confirm: bool
    build: Callable[[Target, dict], Invocation]
    optional: tuple[str, ...] = ()
    remote: bool = False
    local_inputs: tuple[str, ...] = ()
    local_wait_kind: str = ""
    client_text: Callable[[str], str] | None = None  # the only words of the command a client gets (no raw output)
    refused_exits: frozenset = frozenset({exits.USAGE})  # the exit codes that mean the command refused, not failed
    result_text: Callable[[str], str] | None = None  # the only words of a run that worked a client gets (default: none)


def run_in_own_group(argv: list[str], *, cwd: Path, env: dict, stdin: int, timeout: float) -> subprocess.CompletedProcess:
    """Run `argv` in a process group of its own and return its output (text, undecodable bytes replaced).

    On a timeout, or any other interruption, the whole group is ended (SIGTERM, a short grace, then SIGKILL)
    and reaped before this raises, so no descendant of the command outlives the call.
    """
    with subprocess.Popen(argv, cwd=cwd, env=env, stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, errors="replace", start_new_session=True) as process:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except BaseException:
            _end_group(process)
            raise
        return subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)


def _end_group(process: subprocess.Popen) -> None:
    for signal_number in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, signal_number)
        except ProcessLookupError:
            return
        except PermissionError:  # macOS: a group of only unreaped zombies; wait for them below
            pass
        if _group_gone(process, GROUP_GRACE_SECONDS):
            return


def _group_gone(process: subprocess.Popen, grace_seconds: float) -> bool:
    """Reap the group leader, then report whether no process of its group is left within `grace_seconds`."""
    deadline = time.monotonic() + grace_seconds
    while True:
        process.poll()
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:  # macOS: only unreaped zombies are left, which init reaps at once
            pass
        if time.monotonic() >= deadline:
            return False
        time.sleep(GROUP_POLL_SECONDS)


def run(key: str, target: Target, inputs: dict, actor: actors.Actor, run: Callable = run_in_own_group,
        *, log_path: Path | None = None) -> ActionResult:
    """Run the action `key` on `target` as `actor` and return its result; never retries.

    Raises ActionRefused (after logging the refusal) when the key is unknown, does not apply to the target's
    kind, or the inputs or the actor are not acceptable. `log_path` defaults to the cockpit action log in the
    state folder of the engine home (`engine_home.state_dir`). Once the command has run the result stands: a log row
    that cannot be written is reported in `audit` and on stderr, never raised.
    """
    invocation = plan(key, target, inputs, actor, log_path=log_path)
    if key in track_actions.SPECS or track_actions.is_tracked_start(key, inputs):
        try:
            result = _run_track(key, target, inputs, actor, invocation, run)
        except TrackRefusal as refusal:
            log_refusal(log_path, key, target.key, actor, str(refusal))
            raise
    elif key == proposal_actions.START:
        result = _run_proposal_start(target, inputs, actor, invocation, run)
    else:
        result = _spawn(invocation, run, ACTION_TIMEOUT_SECONDS, actor)
    outcome = "done" if result.exit_code == 0 else "failed"
    if result.exit_code == 0:
        _keep_decision(key, target, inputs, actor, log_path)
    try:
        _write_log(log_path, key, target.key, actor, outcome, result.exit_code)
    except OSError as error:
        reason = _clean(str(error))
        print(f"cockpit: cannot write {_destination(log_path)}: {reason}: {outcome} {_clean(key)!r} on "
              f"{_audit_target(target.key)}", file=sys.stderr)
        return replace(result, audit=f"not written: {reason}")
    return result


def plan(key: str, target: Target, inputs: dict, actor: actors.Actor, *, log_path: Path | None = None) -> Invocation:
    """Return the command `run` would spawn without spawning it; a refusal is logged and raised.

    Raises ActionRefused. A failing log write never replaces the refusal (it is reported on stderr).
    """
    try:
        return _authorize(key, target, inputs, actor)
    except (ActionRefused, TrackRefusal) as refusal:
        log_refusal(log_path, key, target.key, actor, str(refusal))
        raise


def log_refusal(path: Path | None, key: str, target_key: tuple, actor: actors.Actor, reason: str) -> None:
    """Append a refused attempt to the action log; a log that cannot be written is reported on stderr only."""
    try:
        _write_log(path, key, target_key, actor, "refused", None)
    except OSError as error:
        label = key if key in ACTIONS else "unknown action"
        print(f"cockpit: cannot write {_destination(path)}: {_clean(str(error))}: refused {label!r}", file=sys.stderr)


def refusal(key: str, result: ActionResult) -> str | None:
    """The reason the command of action `key` refused, when that action reports its exit code as a refusal, else None."""
    action = ACTIONS.get(key)
    if action is None or action.client_text is None or result.exit_code not in action.refused_exits:
        return None
    return action.client_text(result.output)


def client_result(key: str, result: ActionResult) -> ActionResult:
    """The result as a client may see it: for an action with `client_text`, no raw output (it can hold paths); a run
    that worked gives what the action's `result_text` makes of it, nothing without one."""
    action = ACTIONS.get(key)
    if action is None or action.client_text is None:
        return result
    if result.exit_code == 0:
        shown = action.result_text(result.output) if action.result_text else ""
        return replace(result, output=shown)
    return replace(result, output=action.client_text(result.output))


def target_kind(target: Target) -> str:
    """Return `flow`, `lane`, `group`, `account`, `wait`, `project` or `host`."""
    if isinstance(target, (Item, AccountTarget, WaitTarget, ProjectTarget, HostTarget, scheduled_store.ScheduledTask,
                           track_actions.Target)):
        return target.kind
    return KIND_GROUP


def applicable(target: Target) -> list[Action]:
    """The actions that apply to `target`: by its kind, for a wait by what that kind of wait takes, and for a flow task by
    its state: only what the engine accepts is offered (`_flow_offers`)."""
    kind = target_kind(target)
    found = [action for action in ACTIONS.values() if kind in action.kinds and not _needs_missing_folder(target, action.key)]
    if kind == host_actions.KIND_HOST:
        return [action for action in found if host_actions.allows(target, action.key)]
    if kind == KIND_WAIT:
        offered = [action for action in found if wait_actions.allows(target, action.key)]
        return _blocked_actions(target) + offered
    if isinstance(target, scheduled_store.ScheduledTask):
        enabled = target.definition["enabled"]
        hidden = set()
        if enabled and not target.connector_pause:
            hidden.add(scheduled_definition.RESUME)
        if not enabled:
            hidden.add(scheduled_definition.PAUSE)
        if not enabled or target.connector_pause:
            hidden.add(scheduled_definition.RUN)
        return [action for action in found if action.key not in hidden]
    if isinstance(target, track_actions.Target):
        return [action for action in found if action.key in target.offered]
    if kind == KIND_FLOW:
        return [action for action in found if _flow_offers(target, action.key)]
    if isinstance(target, AccountTarget):
        return [action for action in found if _account_offers(target, action.key)]
    return found


def _blocked_actions(target: WaitTarget) -> list[Action]:
    if target.folder_missing or target.lane is None or target.wait_kind != waits.BLOCKED:
        return []
    return [ACTIONS[key] for key in wait_actions.BLOCKED_ACTIONS if _flow_offers(target.lane, key)]


STATE_ACTIONS = ("R", "s", "C", "T", "a")  # the flow actions that depend on the lane's state
LEGACY_OFFERS = ("R", "s", "a")  # what a card with no status (made without the engine's state) offers
FINISHED = ("report_ready", "cleaned")
DISCARDABLE = ("failed", "stopped")
RECONTRACTABLE = ("blocked", "failed")
RECONTRACT_PHASE = "plan"


def _flow_offers(item: Item, key: str) -> bool:
    """Whether the engine accepts action `key` on this flow task. A card with no status (made without the engine's state)
    offers what it always offered: approve, resume and stop."""
    if key not in STATE_ACTIONS or not item.status:
        return key in LEGACY_OFFERS or key not in STATE_ACTIONS
    if key == "a":
        return item.agent_state == "gate"
    if key == "s":
        return item.status not in FINISHED
    if key == "C":
        return item.status in DISCARDABLE
    if key == "T":
        return item.status in RECONTRACTABLE and item.phase == RECONTRACT_PHASE and item.has_contract
    return _not_resumable(item) is None


def public_argv(key: str, invocation: Invocation) -> list[str]:
    """Keep private connector form fields out of command previews."""
    return ["sakura", *invocation.argv[1:3]] if key in connector_contract.ACTIONS else invocation.argv


def _authorize(key: str, target: Target, inputs: dict, actor: actors.Actor) -> Invocation:
    action = ACTIONS.get(key)
    if action is None:
        raise ActionRefused(f"unknown action {key!r}")
    tracked_start = track_actions.is_tracked_start(key, inputs)
    if _needs_missing_folder(target, key) and not tracked_start:
        raise ActionRefused(PROJECT_FOLDER_MISSING)
    from_blocked_wait = isinstance(target, WaitTarget) and key in wait_actions.BLOCKED_ACTIONS
    if from_blocked_wait:
        if action not in _blocked_actions(target):
            raise ActionRefused(f"action {key!r} is unavailable for this wait")
        target = target.lane
    kind = target_kind(target)
    if key in connector_contract.ACTIONS and target != host_actions.CONNECTORS:
        raise ActionRefused("invalid_request")
    if kind not in action.kinds:
        raise ActionRefused(f"action {key!r} does not apply to a {kind}")
    if isinstance(target, AccountTarget) and not _account_offers(target, key):
        raise ActionRefused("this action is unavailable for the selected account")
    if key in scheduled_definition.ACTIONS and (actor.source not in (actors.CLI_SOURCE, actors.APP_SOURCE)
                                                or action not in applicable(target)):
        raise ActionRefused("scheduled_action_unavailable")
    if key in track_actions.SPECS or (tracked_start and actor.source != actors.MASTER_SOURCE):
        track_actions.require_person(actor)
    if key in track_actions.SPECS and isinstance(target, WaitTarget) and not track_actions.is_decision_wait(target.key):
        raise ActionRefused("This wait is not a Track decision")
    _check_inputs(action, target, inputs)
    if from_blocked_wait and key in ("s", "C"):
        inputs = {**inputs, "lane_revision": target.resume_revision}
    if actor.source == "overlay":
        inputs = remote_inputs(key, inputs)
    invocation = _track_invocation(key, target, inputs) if tracked_start else action.build(target, inputs)
    return _with_actor(invocation, _actor_text(actor))


def _needs_missing_folder(target: Target, key: str) -> bool:
    if isinstance(target, WaitTarget):
        return target.folder_missing and key not in (*attention.ACTIONS, "markreviewed", proposal_actions.DISMISS)
    return isinstance(target, (Item, ProjectTarget)) and target.folder_missing


def _actor_text(actor: actors.Actor) -> str:
    text = actors.format(actor)
    try:
        actors.parse(text)
    except actors.ActorError as error:
        raise ActionRefused(f"invalid actor: {error}") from error
    return text


def _with_actor(invocation: Invocation, actor_text: str) -> Invocation:
    argv = list(invocation.argv)
    argv.insert(argv.index(SEPARATOR), f"--actor={actor_text}")
    return Invocation(argv, invocation.cwd, invocation.handler)


def _child_environment() -> dict[str, str]:
    """The cockpit's environment without the variables that would bind a child to the cockpit's own tmux.

    The child is a sakura command that drives the person's tmux server itself, so `TMUX_TMPDIR` stays; the
    repository-configured commands it spawns (lane setup, down, status) get the private one from `lanes.worktrees`.
    """
    environment = {key: value for key, value in os.environ.items() if key not in UNFORWARDED_VARIABLES}
    return {**environment, actors.TRUSTED_ENV: "1"}


def _keep_decision(key: str, target: Target, inputs: dict, actor: actors.Actor, log_path: Path | None) -> None:
    """Keep the answer a finished action gave to an ask (`cockpit/decided.py`); an action that answers nothing keeps nothing."""
    answer = _answer_of(key, inputs)
    wait = _open_ask(key, target)
    if answer is None or wait is None:
        return
    choice, text = inputs.get("choice"), inputs.get("text", inputs.get("note"))
    state_dir = Path(log_path).parent if log_path else decided.default_state_dir()
    decided.record(state_dir, wait=wait, answer=answer, choice=choice, text=text, actor=_actor_text(actor),
                   at=datetime.now(UTC))


def _answer_of(key: str, inputs: dict) -> str | None:
    """The word for what action `key` decides, or None for an action that decides nothing."""
    if key == "k":
        return f"mark_{inputs['verdict']}"
    if key == "r":
        return "choice" if inputs.get("choice") else "reply"
    return DECISION_ANSWERS.get(key)


def _open_ask(key: str, target: Target) -> dict | None:
    """The ask `target` stands for as the cockpit knows it: a wait, or a lane at the gate or blocked that the card
    answers (a card made without a status says nothing of what it waits on)."""
    if isinstance(target, WaitTarget):
        return decided.wait_fields(target.wait)
    if not isinstance(target, Item) or target.kind != KIND_FLOW:
        return None
    task = target.key[2]
    if key == "a" and target.status == STATUS_AWAITING:
        return decided.fallback_wait(target.repo, task, waits.APPROVAL, _gate_of(target))
    if key in ("s", "C") and target.status in waits.BLOCKED_STATUSES:
        return decided.fallback_wait(target.repo, task, waits.BLOCKED, waits.BLOCKED)
    return None


def _spawn(invocation: Invocation, run: Callable, timeout: float, actor: actors.Actor | None = None) -> ActionResult:
    if invocation.handler is not None:
        exit_code, output = invocation.handler(actor)
        return ActionResult(invocation, exit_code, strip_controls(output))
    try:
        done = run(invocation.argv, cwd=invocation.cwd, env=_child_environment(), stdin=subprocess.DEVNULL,
                   timeout=timeout)
    except (OSError, subprocess.TimeoutExpired) as error:
        return ActionResult(invocation, exits.FAILED, strip_controls(f"cannot run the command: {error}"))
    return ActionResult(invocation, done.returncode, strip_controls((done.stdout or "") + (done.stderr or "")))


# Inputs

def _check_inputs(action: Action, target: Target, inputs: dict) -> None:
    if action.key in track_actions.SPECS:
        track_actions.validate(action.key, target_kind(target), inputs)
        return
    tracked_start = track_actions.is_tracked_start(action.key, inputs)
    if tracked_start:
        track_actions.validate_start(inputs)
    needed = action.needs(target)
    if action.key in connector_contract.ACTIONS:
        if set(inputs) != set(needed) or any(not isinstance(value, str) for value in inputs.values()):
            raise ActionRefused("invalid_request")
        return
    unknown = sorted(set(inputs) - set(needed) - set(action.optional))
    missing = [name for name in needed if name not in inputs]
    if unknown or missing:
        if action.key in scheduled_definition.ACTIONS:
            raise ActionRefused("invalid_scheduled_inputs")
        takes = ", ".join(needed + tuple(f"{name} (optional)" for name in action.optional)) or "no inputs"
        problem = f"unexpected {', '.join(unknown)}" if unknown else f"missing {', '.join(missing)}"
        raise ActionRefused(f"action {action.key!r} takes {takes}; {problem}")
    for name, value in inputs.items():
        if tracked_start and name in track_actions.START_INPUTS:
            continue
        reason = _input_problem(name, value, target_kind(target), action.key)
        if reason:
            if action.key in scheduled_definition.ACTIONS:
                raise ActionRefused(reason if reason in ("invalid_definition", "invalid_operation") else "invalid_scheduled_inputs")
            raise ActionRefused(f"invalid {name}: {reason}")


def _input_problem(name: str, value: object, kind: str, action_key: str) -> str | None:
    if not isinstance(value, str):
        return "not text"
    if action_key in scheduled_definition.ACTIONS and name in ("definition", "op"):
        if name == "op":
            return None if scheduled_definition.OP.fullmatch(value) else "invalid_operation"
        try:
            scheduled_definition.decode(value)
        except scheduled_store.Refused as error:
            return str(error)
        return None
    if action_key == "R" and name == "note":
        return resume.note_problem(value)
    if name in ("op", "choices"):
        return setup_apply.input_problem(name, value)
    if name == "text":
        return _text_problem(value)
    if name in ("folder", "project"):
        return text_limits.one_line_problem(value, MAX_FOLDER)
    if name in account_actions.INPUT_PROBLEMS:
        return account_actions.INPUT_PROBLEMS[name](value)
    pattern = _PATTERNS.get((name, kind), _PATTERNS.get((name, "")))
    if pattern is not None:
        return None if pattern.fullmatch(value) else f"does not match {pattern.pattern}"
    return text_limits.one_line_problem(value, MAX_LINE)


def _text_problem(value: str) -> str | None:
    return text_limits.text_problem(value, MAX_TEXT)


_PATTERNS = {("provider", ""): re.compile("|".join(account_login.PROVIDERS)), ("name", ""): LANE_NAME, ("rule", ""): RULE_PATTERN, ("preset", ""): LANE_NAME, ("level", ""): LEVEL_PATTERN, ("accounts", ""): ACCOUNT_LIST_PATTERN, ("role", ""): ROLE_NAME, ("step", ""): DIAL_STEP,
             ("model", KIND_LANE): MODEL_NAME, ("verdict", ""): VERDICT, ("remember", ""): REMEMBER}


# Commands: options first, then `--` and the positionals, so no input can be read as an option

def _command(target: Target, words: tuple[str, ...], options: list[str], positionals: list[str]) -> Invocation:
    return Invocation([str(PROGRAM), *words, *options, SEPARATOR, *positionals], target.repo)


def _group_option(item: Item) -> list[str]:
    return [f"--group={item.key[1]}"]


def _task(item: Item | WaitTarget) -> str:
    return item.key[2]


def _outside(call: outside_actions.Call) -> Invocation:
    return Invocation(call.argv, call.cwd, call.handler)


def _close_lane(lane: Item, inputs: dict) -> Invocation:
    if lane.kind == KIND_OUTSIDE:
        return _outside_call(outside_actions.close, lane, inputs)
    return _command(lane, ("lane", "done"), _group_option(lane), [lane.key[2]])


def _change_model(item: Item, inputs: dict) -> Invocation:
    if item.kind == KIND_FLOW:
        return _command(item, ("flow", "role"), [], [_task(item), inputs["role"], inputs["model"]])
    return _command(item, ("lane", "model"), _group_option(item), [item.key[2], inputs["model"]])


def _dial(item: Item, inputs: dict) -> Invocation:
    return _command(item, ("flow", "dial"), [], [_task(item), inputs["step"]])


def _approve(target: Item | WaitTarget, inputs: dict) -> Invocation:
    gate = _gate_of(target)
    _require_pending(target, gate)
    remember = "remember" in inputs
    if remember and gate != checks.APPROVAL_GATE:
        raise ActionRefused(f"remember applies only to the {checks.APPROVAL_GATE} gate, not {gate}")
    options = ["--continue", "--detach", *_revision_option(target), *(["--remember"] if remember else [])]
    return _command(target, ("flow", "approve"), options, [_task(target), gate])


def _require_pending(target: Item | WaitTarget, gate: str) -> None:
    """Refuse at the first call, before any confirmation token, when the gate has nothing pending (`waits.gate_pending`,
    read from the task's own state now): a wait comes from the engine's state, and a card made without it (no status) is
    left to the engine's own check."""
    if isinstance(target, WaitTarget) or not target.status:
        return
    try:
        state = store.TaskStore(target.repo, _task(target)).read_state()
    except (store.StateError, OSError):
        raise ActionRefused(f"no longer waiting: {_task(target)} cannot be read; nothing was done") from None
    if not waits.gate_pending(state, gate):
        raise ActionRefused(ask_words.NOT_WAITING_ANY_MORE)


def _revision_option(target: Item | WaitTarget) -> list[str]:
    """`--revision=<what the person was shown>` for a wait or a card at a gate: the engine checks it under the task lock."""
    revision = target.revision if isinstance(target, WaitTarget) else target.approval_revision
    return [f"--revision={revision}"] if revision else []


def _gate_of(target: Item | WaitTarget) -> str:
    """The gate the target waits at: a wait names it; an item's next command does, the spec gate when it names none."""
    if isinstance(target, WaitTarget):
        if target.wait_kind != waits.APPROVAL:
            raise ActionRefused(f"this wait is not an approval but {target.wait_kind}")
        return target.subject
    words = target.next_command.split()
    return words[-2] if words[:3] == ["sakura", "flow", "approve"] and len(words) >= 5 else APPROVED_GATE


def _outside_call(build: Callable, target: Target, inputs: dict) -> Invocation:
    try:
        return _outside(build(target, inputs))
    except outside_actions.NotAnswerable as problem:
        raise ActionRefused(str(problem)) from problem


def _mark(wait: Target, inputs: dict) -> Invocation:
    _require_legacy_answer(wait)
    if wait.wait_kind != waits.OWNER_ROW:
        raise ActionRefused(f"only an owner row takes a mark, not {wait.wait_kind}")
    if wait.is_outside:
        return _outside_call(outside_actions.mark, wait, inputs)
    options = ["--continue", "--detach", *_revision_option(wait), *([f"--note={inputs['note']}"] if "note" in inputs else [])]
    return _command(wait, ("flow", "mark"), options, [_task(wait), wait.subject, inputs["verdict"]])


def _restore(wait: Target, inputs: dict) -> Invocation:
    if wait.wait_kind != waits.OWNER_ROW or wait.is_outside:
        raise ActionRefused("only an owner row of a flow task can have its workspace restored")
    expected = [f"--expect-workspace={wait.wait.workspace_head}", f"--expect-head={wait.wait.engine_head}",
                f"--row={wait.subject}", f"--revision={wait.revision}"]
    return _command(wait, ("flow", "restore-head"), expected, [_task(wait)])


def _reply(wait: Target, inputs: dict) -> Invocation:
    _require_legacy_answer(wait)
    if wait.wait_kind != waits.QUESTION:
        raise ActionRefused(f"only a question takes a reply, not {wait.wait_kind}")
    if wait.is_outside:
        return _outside_call(outside_actions.reply, wait, inputs)
    return _reply_flow(wait, inputs)


def _require_legacy_answer(wait: WaitTarget) -> None:
    if wait_actions.inbox_kind(wait.wait) == waits.PROPOSAL or proposal_actions.outcome(wait.wait.id):
        raise ActionRefused("Use Start or Dismiss to resolve this proposal.")


def _reply_flow(wait: Target, inputs: dict) -> Invocation:
    """Answer an engine lane's question: `sakura flow reply`, which records the reply and steers the lane."""
    choice, text = inputs.get("choice", ""), inputs.get("text", "")
    if bool(choice) == bool(text):
        raise ActionRefused("give a choice or text, one of them")
    if choice and choice not in wait.wait.choices:
        raise ActionRefused(f"{choice!r} is not one of the choices")
    return _command(wait, ("flow", "reply"), [f"--choice={choice}" if choice else f"--text={text}"],
                    [_task(wait), wait.subject, wait.revision])


def _decide_proposal(verb: str, wait: Target) -> Invocation:
    if wait.wait_kind == scheduled_inbox.APPROVAL:
        def decide(actor: actors.Actor) -> tuple[int, str]:
            if actor.source not in (actors.CLI_SOURCE, actors.APP_SOURCE):
                return exits.USAGE, "scheduled_action_unavailable"
            try:
                tasks = scheduled_store.Store()
                accepted = tasks.decide(wait.subject, wait.revision, verb == "accept", datetime.now(UTC))
                if accepted:
                    _reconcile_scheduled(tasks)
            except scheduled_store.Refused as error:
                return exits.USAGE, str(error)
            return exits.OK, ""

        return Invocation([str(PROGRAM), "scheduled", verb, SEPARATOR, wait.subject, wait.revision], wait.repo, decide)
    if wait.wait_kind == waits.AUTOMATION:
        return _command(wait, ("automations", verb), [], [wait.subject, wait.revision])
    if wait.wait_kind != waits.PROPOSAL:
        raise ActionRefused(f"only a proposal can be {verb}ed, not {wait.wait_kind}")
    return _command(wait, ("proposal", verb), [], [wait.subject, wait.revision])


def _accept_proposal(wait: Target, inputs: dict) -> Invocation:
    return _decide_proposal("accept", wait)


def _reject_proposal(wait: Target, inputs: dict) -> Invocation:
    return _decide_proposal("reject", wait)


def _attention_action(key: str, label: str, *required: str) -> Action:
    def build(target: WaitTarget, inputs: dict) -> Invocation:
        if not wait_actions.allows(target, key):
            raise ActionRefused("this wait is already awake")
        now = datetime.now(UTC)
        try:
            until = attention.deadline(inputs["until"], now) if key == "snooze" else None
        except ValueError as error:
            raise ActionRefused(str(error)) from error

        def apply(actor: actors.Actor) -> tuple[int, str]:
            try:
                attention.update(target.wait.id, key, now, until)
            except (OSError, ValueError) as error:
                print(f"cockpit: could not save Inbox attention: {error}", file=sys.stderr)
                return exits.FAILED, "could not save Inbox attention"
            return exits.OK, ""

        return Invocation([str(PROGRAM), "cockpit", key, SEPARATOR, target.wait.id], target.repo, apply)

    return Action(key, label, frozenset({KIND_WAIT}), _needs(*required), False, build, remote=True)


def _mark_reviewed(target: WaitTarget, inputs: dict) -> Invocation:
    if target.wait_kind != waits.REVIEW:
        raise ActionRefused("this wait is not a pull request review")
    task = store.TaskStore(target.repo, target.wait.task, folder_missing=target.folder_missing)
    return Invocation([str(PROGRAM), "cockpit", "mark-reviewed", SEPARATOR, target.wait.id], target.repo,
                      lambda actor: review_waits.resolve(task, target.wait, _actor_text(actor)))


def _start_proposal(target: WaitTarget, inputs: dict) -> Invocation:
    prior = proposal_actions.outcome(target.wait.id)
    if prior.get("answer") == proposal_actions.STARTED:
        return _command(ProjectTarget(Path(prior["repo"])), ("flow", "start"), [], [])
    _require_proposal(target)
    if target.wait.finding:
        saved = target.wait.finding
        inputs = {**{key: saved[key] for key in ("project", "preset", "account") if saved[key]}, **inputs}
    repo = proposal_actions.project_for(target.wait, target.projects, inputs.get("project", ""))
    if repo is None:
        raise ActionRefused("Choose a registered project to start this proposal.")
    if repo in target.missing_projects:
        raise ActionRefused(PROJECT_FOLDER_MISSING)
    wait = target.wait
    title = tasks.split_title(inputs.get("title", ""))[0]
    brief = title if title and title != tasks.split_title(wait.title)[0] else "Review the selected Inbox proposal."
    context = {name: getattr(wait, name) for name in ("source", "author", "title", "detail")}
    invocation = _start_lane(ProjectTarget(repo), {**inputs, "text": brief})
    options = [f"--inbox-op={proposal_actions.operation_id(wait.id)}", f"--inbox-context={json.dumps(context)}"]
    if account := inputs.get("account"):
        if not config.ACCOUNT_ID.fullmatch(account):
            raise ActionRefused("Choose a valid account id.")
        options.append(f"--set=budget.account={account}")
    argv = list(invocation.argv)
    separator = argv.index(SEPARATOR)
    argv[separator:separator] = options
    return replace(invocation, argv=argv)


def _run_proposal_start(target: WaitTarget, inputs: dict, actor: actors.Actor, invocation: Invocation,
                        run: Callable) -> ActionResult:
    def spawn() -> tuple[int, str]:
        result = _spawn(invocation, run, ACTION_TIMEOUT_SECONDS, actor)
        return result.exit_code, result.output

    try:
        options = [arg for arg in invocation.argv[3:] if not arg.startswith("--actor=")]
        code, output = proposal_actions.start(target.wait, inputs["op"], invocation.cwd, options, _actor_text(actor),
                                              spawn, tasks.created_lane_id)
    except (OSError, ValueError) as error:
        print(f"cockpit: could not save proposal start: {error}", file=sys.stderr)
        code, output = exits.FAILED, "Could not save the proposal start. Retry start."
    return ActionResult(invocation, code, strip_controls(output))


def start_scheduled_lane(target: WaitTarget, inputs: dict, actor: actors.Actor) -> ActionResult:
    """Use the existing lane reservation for an authorized scheduled tool call."""
    invocation = _with_actor(_start_proposal(target, inputs), _actor_text(actor))
    return _run_proposal_start(target, inputs, actor, invocation, run_in_own_group)


def _dismiss_proposal(target: WaitTarget, inputs: dict) -> Invocation:
    _require_proposal(target)

    def apply(actor: actors.Actor) -> tuple[int, str]:
        try:
            return proposal_actions.dismiss(target.wait, _actor_text(actor))
        except (OSError, ValueError) as error:
            print(f"cockpit: could not dismiss proposal: {error}", file=sys.stderr)
            return exits.FAILED, "Could not save the dismissal. Retry dismiss."

    return Invocation([str(PROGRAM), "cockpit", "dismiss", SEPARATOR, target.wait.id], target.repo, apply)


def _require_proposal(target: WaitTarget) -> None:
    if wait_actions.inbox_kind(target.wait) != waits.PROPOSAL:
        raise ActionRefused("this wait is not a proposal")


def _start_lane(project: ProjectTarget, inputs: dict) -> Invocation:
    """Open a new lane in the project from the task text: always a new one, never a resume of an equal text.

    A `preset` and a lane pace `level` are applied to the new lane at creation (`flow start --preset`, `--set`)."""
    options = ["--detach", "--new", f"--message={inputs['text']}"]
    if "preset" in inputs:
        # The person's own word, as a lane switch is: moving to a lighter preset needs no separate `relax`.
        options += [f"--preset={_known_preset(inputs['preset'])}", "--relax=preset"]
    if "level" in inputs:
        options.append(f"--set=knobs.lane_pace={_lane_level(inputs['level'])}")
    return _command(project, ("flow", "start"), options, [])


def _known_preset(name: str) -> str:
    if name not in presets.names():
        raise ActionRefused("invalid preset: not a known preset")
    return name


def _lane_level(level: str) -> str:
    if level not in pace.LANE_LEVELS:
        raise ActionRefused(f"invalid level: use one of {', '.join(pace.LANE_LEVELS)}")
    return level


def _add_project(host: HostTarget, inputs: dict) -> Invocation:
    """Register the Git repository of a folder, in this process: `sakura flow register`, with the refusals as sentences."""
    folder = inputs["folder"]
    return Invocation([str(PROGRAM), "flow", "register", SEPARATOR, folder], host.repo,
                      lambda actor: project_actions.add(folder))


def _add_project_result(output: str) -> str:
    """What adding a project says of the project it added: the project as JSON (name and folder)."""
    return output.strip()


def _add_project_refusal_text(output: str) -> str:
    """The last line the add gave, or a fixed text when it looks like it holds a path."""
    return _last_line(output, ADD_PROJECT_FAILED_TEXT)


def _digest_now(host: HostTarget, inputs: dict) -> Invocation:
    """Start a digest run in the background (the model run outlasts an action's time); the command refuses at once
    when another run is going."""
    return _command(host, ("digest", "run"), ["--detach"], [])


def _digest_refusal_text(output: str) -> str:
    """The last line the digest command printed, or a fixed text when it is empty or looks like it holds a path."""
    return _last_line(output, DIGEST_FAILED_TEXT)


def _clean(item: Item, inputs: dict) -> Invocation:
    """Discard a failed or stopped lane: its workspaces, branches and repository go, nothing is delivered."""
    if item.status not in DISCARDABLE:
        raise ActionRefused(f"cannot clean up {_task(item)}: only a failed or stopped lane can be discarded")
    return _command(item, ("flow", "cleanup"), ["--discard", *_lane_revision_option(inputs)], [_task(item)])


def _recontract(item: Item, inputs: dict) -> Invocation:
    if not _flow_offers(item, "T"):
        raise ActionRefused(f"cannot re-contract {_task(item)}: it needs a blocked or failed lane at the plan phase "
                            "with a saved contract")
    return _command(item, ("flow", "recontract"), [], [_task(item)])


def _stop(item: Item, inputs: dict) -> Invocation:
    return _command(item, ("flow", "stop"), _lane_revision_option(inputs), [_task(item)])


def _lane_revision_option(inputs: dict) -> list[str]:
    return [f"--revision={inputs['lane_revision']}"] if "lane_revision" in inputs else []


def _resume(item: Item, inputs: dict) -> Invocation:
    if reason := _not_resumable(item):
        raise ActionRefused(f"cannot resume {_task(item)}: {reason}")
    options = ["--detach", f"--stopped-revision={item.resume_revision}"]
    if note := inputs.get("note"):
        options.append(f"--note={note}")
    return _command(item, ("flow", "resume"), options, [_task(item)])


def _not_resumable(item: Item) -> str | None:
    """Why the item cannot be resumed, or None when it stands still and its next step is a resume."""
    if item.agent_state == "working":
        return "it is working"
    if item.agent_state == "gate":
        return "it waits for your approval"
    if item.status == "failed":
        return "it failed, which a resume does not change; clean it up or re-contract it"
    if item.no_retry:
        return f"a resume would only block again: {item.no_retry}"
    if item.agent_state == "blocked" and item.status != "blocked":
        return "it is blocked or failed, which a resume does not change"
    if not item.next_command.startswith(RESUME_COMMAND):
        return "it is finished"
    return None


def _resume_refusal_text(output: str) -> str:
    """The last line the resume command printed, or a fixed text when it is empty or looks like it holds a path."""
    return _last_line(output, RESUME_FAILED_TEXT)


def _restore_refusal_text(output: str) -> str:
    """The last line the restore command printed, or a fixed text."""
    return _last_line(output, RESTORE_FAILED_TEXT)


def _approval_refusal_text(output: str) -> str:
    """The last line the approve command printed (`no longer waiting: ...` for a stale answer), or a fixed text."""
    return _last_line(output, APPROVE_FAILED_TEXT)


def _start_refusal_text(output: str) -> str:
    """The last line the start command printed: the ceiling's own sentence names lanes (`project/task`), which the
    path rule of `_last_line` would hide; anything else follows that rule."""
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if lines and lines[-1].startswith(lane_ceiling.REFUSAL_PREFIX):
        return lines[-1][:MAX_LINE * 2]
    return _last_line(output, START_FAILED_TEXT)


def _last_line(output: str, fallback: str) -> str:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    text = lines[-1] if lines else ""
    return fallback if not text or "/" in text else text[:MAX_LINE]


def _write(item: Item, inputs: dict) -> Invocation:
    if item.kind == KIND_OUTSIDE:
        return _outside_call(outside_actions.steer, item, inputs)
    if item.kind == KIND_FLOW:
        return _command(item, ("flow", "steer"), [], [_task(item), inputs["text"]])
    return _command(item, ("lane", "say"), _group_option(item), [item.key[2], inputs["text"]])


def _model_needs(target: Target) -> tuple[str, ...]:
    return ("role", "model") if target_kind(target) == KIND_FLOW else ("model",)


def _needs(*names: str) -> Callable[[Target], tuple[str, ...]]:
    return lambda target: names


LEEWAY_VERBS = {"rename": "rename", "disable": "hide", "enable": "show", "remove": "remove"}
LEEWAY_ONLY_VERBS = frozenset({"remove"})
LEEWAY_FAILED_TEXT = "leeway did not take the change"


def _account_offers(account: AccountTarget, key: str) -> bool:
    return key in GOOSE_ACCOUNT_ACTIONS if account.provider == GOOSE else key != "accountcheck"


def _account_action(key: str, label: str, word: str, inputs: tuple[str, ...] = (), *, confirm: bool = False) -> Action:
    """An action running `sakura accounts <word> PROVIDER ID <inputs>`, with the registry's refusals reported.

    Rename, disable and enable go to Leeway's account book instead (`leeway accounts rename|hide|show`) when a Leeway
    helper is found, since that book is where the apps keep names and hidden accounts.
    """
    def build(account: AccountTarget, given: dict) -> Invocation:
        if "plan" in given and (reason := account_actions.plan_choice_problem(account.provider, given["plan"])):
            raise ActionRefused(f"invalid plan: {reason}")
        invocation = _command(account, ("accounts", word), [], [account.provider, account.id, *(given[n] for n in inputs)])
        if account.provider == GOOSE:
            if word == "remove":
                return replace(invocation, handler=lambda actor: _detach_goose(account.id))
            return invocation
        if word in LEEWAY_ONLY_VERBS:
            _require_removable(account.id)
        if word in LEEWAY_VERBS and leeway.is_available():
            return replace(invocation, handler=lambda actor: _leeway_account(LEEWAY_VERBS[word], account.id, given))
        return invocation
    return Action(key, label, frozenset({KIND_ACCOUNT}), _needs(*inputs), confirm, build,
                  client_text=account_actions.refusal_text)


def _detach_goose(account_id: str) -> tuple[int, str]:
    try:
        account_removal.detach_goose(account_id)
    except (account_registry.AccountsConfigError, OSError, engine_home.EngineHomeError):
        return exits.USAGE, "sakura: error: could not detach the selected Goose account"
    return exits.OK, "Goose account detached"


def _account_check(account: AccountTarget, inputs: dict) -> Invocation:
    def check(actor: actors.Actor) -> tuple[int, str]:
        try:
            return exits.OK, json.dumps(goose.check(account.id))
        except programs.ProgramError as error:
            return exits.USAGE, str(error)
        except (account_registry.AccountsConfigError, OSError, engine_home.EngineHomeError):
            return exits.USAGE, "could not check the selected Goose account"

    return Invocation([str(PROGRAM), "cockpit", "accountcheck", SEPARATOR, *account.key], account.repo, check)


def _require_removable(account_id: str) -> None:
    """Refuse, with the reason, a removal Leeway cannot do or the engine still depends on (`account_removal.py`)."""
    if not leeway.is_available():
        raise ActionRefused("removing an account needs the Leeway helper, which was not found")
    try:
        in_book = any(row["id"] == account_id for row in leeway.accounts())
    except leeway.LeewayError as error:
        raise ActionRefused(f"removing an account needs Leeway's account book: {error}") from None
    if not in_book:
        raise ActionRefused(f"{account_id} is not in Leeway's account book, so it cannot be removed here")
    if reason := account_removal.refusal(account_id):
        raise ActionRefused(f"cannot remove {account_id}: {reason}")


def _leeway_account(verb: str, account_id: str, given: dict) -> tuple[int, str]:
    """Run the account verb on Leeway's book in this process; (exit code, one line)."""
    try:
        leeway.account_action(verb, account_id, given.get("label"))
    except leeway.LeewayError as error:
        return exits.FAILED, str(error)
    except ValueError as error:  # an id or a name the helper's parser would read as an option
        return exits.USAGE, str(error)
    if verb == "remove":
        account_removal.forget(account_id)
    return exits.OK, f"leeway accounts {verb} {account_id}"


RULE_REFUSED_TEXT = "the engine did not take the change"
ACCOUNT_LOGIN_FAILED_TEXT = "the account was not added"


def _set_rule(host: HostTarget, inputs: dict) -> Invocation:
    """Change one rule of the lane flow: `sakura engine set`, which validates and keeps it."""
    return _command(host, ("engine", "set"), [], [inputs["rule"], inputs["value"]])


def _refresh_models(host: HostTarget, inputs: dict) -> Invocation:
    """Refresh the chosen account or all accounts through the engine catalog command."""
    if inputs and inputs.keys() != {"provider", "account"}:
        raise ActionRefused("model refresh needs both provider and account")
    options = [f"--{key}={inputs[key]}" for key in ("provider", "account") if key in inputs]
    return _command(host, ("engine", "models"), options, ["refresh"])


def _phone_change(word: str, value: str, host: HostTarget) -> Invocation:
    return _command(host, ("cockpit", "phone"), ["--json", word], [value])


def _allow_device(host: HostTarget, inputs: dict) -> Invocation:
    """Let a device of the Mac's own login open the phone page (`sakura cockpit phone allow`)."""
    return _phone_change("allow", _device(inputs["device"]), host)


def _remove_device(host: HostTarget, inputs: dict) -> Invocation:
    """Stop a device from opening the phone page (`sakura cockpit phone remove`)."""
    return _phone_change("remove", _device(inputs["device"]), host)


def _phone_port(host: HostTarget, inputs: dict) -> Invocation:
    """Set the phone page's port (`sakura cockpit phone port`)."""
    value = str(inputs["port"])
    if not value.isdigit() or not 1 <= int(value) <= settings.MAX_PORT:
        raise ActionRefused(f"invalid port: use a whole number from 1 to {settings.MAX_PORT}")
    return _phone_change("port", value, host)


def _device(value: object) -> str:
    if not isinstance(value, str) or not settings.NODE_PATTERN.fullmatch(value) or value.startswith("-"):
        raise ActionRefused("invalid device: use the id of a device of your overlay")
    return value


def _use_preset(host: HostTarget, inputs: dict) -> Invocation:
    return _command(host, ("preset", "use"), [], [inputs["preset"]])


def _save_preset(host: HostTarget, inputs: dict) -> Invocation:
    return _command(host, ("preset", "save"), [], [inputs["name"]])


def _as_given(output: str) -> str:
    """What a client gets of an action that worked: its output as it is."""
    return output.strip()


def _account_login_refusal_text(output: str) -> str:
    """The last line the account action gave: a fixed sentence of `account_login` (they name no path of the host)."""
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    return lines[-1][:MAX_LINE] if lines else ACCOUNT_LOGIN_FAILED_TEXT


def _login_folder(host: HostTarget, inputs: dict) -> Invocation:
    """Make an empty login folder for a provider and give the command to log in (`account_login.new_login_folder`)."""
    provider = inputs["provider"]
    return Invocation([str(PROGRAM), "accounts", "login-folder", SEPARATOR, provider], host.repo,
                      lambda actor: account_login.new_login_folder(provider))


def _add_account(host: HostTarget, inputs: dict) -> Invocation:
    """Add a login folder as an account of Leeway's book under the name the person typed (`account_login.add_account`)."""
    folder, provider, label = inputs["folder"], inputs["provider"], inputs["label"]
    upstream = None
    if "choices" in inputs:
        if provider != GOOSE:
            raise ActionRefused("provider configuration is available for Goose accounts")
        try:
            upstream = account_registry.provider_config(json.loads(inputs["choices"]))
        except (ValueError, RecursionError):
            raise ActionRefused("choices must contain only the upstream provider slug") from None
    return Invocation([str(PROGRAM), "accounts", "add", SEPARATOR, provider, folder, label], host.repo,
                      lambda actor: account_login.add_account(folder, provider, label, upstream))


def _setup_apply(host: HostTarget, inputs: dict) -> Invocation:
    if host != host_actions.ENGINE:
        raise ActionRefused("setup applies to the host engine")
    op = inputs["op"]
    try:
        choices = setup_apply.parse_choices(op, inputs["choices"])
    except setup_apply.SetupRefused as error:
        raise ActionRefused(str(error)) from error
    argv = [str(PROGRAM), "setup", "apply", f"--op={op}", SEPARATOR]
    return Invocation(argv, host.repo, lambda actor: setup_apply.run(op, choices))


def _engine_pace(host: HostTarget, inputs: dict) -> Invocation:
    """Set the engine's pace: where each account lands at its reset (`sakura engine pace`)."""
    return _command(host, ("engine", "pace"), [], [inputs["level"]])


def _pace_accounts(project: ProjectTarget, inputs: dict) -> Invocation:
    """Set the accounts pace may move this project's steps to (`sakura engine pace-accounts`); an empty list allows none."""
    return _command(project, ("engine", "pace-accounts"), [], [inputs["accounts"]])


def _lane_pace(item: Item, inputs: dict) -> Invocation:
    """Set one lane's own pace: its line moves, the reserve never does (`sakura flow pace`)."""
    return _command(item, ("flow", "pace"), [], [_task(item), inputs["level"]])


def _lane_preset(item: Item, inputs: dict) -> Invocation:
    """Switch one lane to another preset at its next step (`sakura flow preset`)."""
    return _command(item, ("flow", "preset"), [], [_task(item), inputs["preset"]])


def _lane_step(item: Item, inputs: dict) -> Invocation:
    """Turn one optional step on or off for one lane, from its next step (`sakura flow step`)."""
    return _command(item, ("flow", "step"), [], [_task(item), inputs["step"], inputs["state"]])


def _rule_refusal_text(output: str) -> str:
    """The last line the rule command printed (its reason), or a fixed text."""
    return _last_line(output, RULE_REFUSED_TEXT)


def _engine_actions() -> dict[str, Action]:
    host = frozenset({host_actions.KIND_HOST})
    return {action.key: action for action in (
        Action("K", "Set rule", host, _needs("rule", "value"), False, _set_rule, client_text=_rule_refusal_text),
        Action("U", "Use preset", host, _needs("preset"), False, _use_preset, client_text=_rule_refusal_text),
        Action("V", "Save rules as preset", host, _needs("name"), False, _save_preset, client_text=_rule_refusal_text),
        Action("Q", "Set pace", host, _needs("level"), False, _engine_pace, client_text=_rule_refusal_text),
        Action("setupapply", "Apply first-launch choices", host, _needs("op", "choices"), False, _setup_apply,
               client_text=_as_given, result_text=_as_given),
        Action("M", "Refresh models", host, _needs(), False, _refresh_models, optional=("provider", "account"),
               client_text=_rule_refusal_text),
        Action("F", "Allow device on the phone page", host, _needs("device"), False, _allow_device,
               client_text=_rule_refusal_text),
        Action("Z", "Remove device from the phone page", host, _needs("device"), False, _remove_device,
               client_text=_rule_refusal_text),
        Action("E", "Set phone page port", host, _needs("port"), False, _phone_port, client_text=_rule_refusal_text),
        Action("g", "Make a login folder for an account", host, _needs("provider"), False, _login_folder,
               client_text=_account_login_refusal_text, result_text=_as_given),
        Action("j", "Add account", host, _needs("folder", "provider", "label"), False, _add_account, optional=("choices",),
               client_text=_account_login_refusal_text, result_text=_as_given),
        Action("W", "Set accounts pace may use", frozenset({KIND_PROJECT}), _needs("accounts"), False, _pace_accounts,
               client_text=_rule_refusal_text),
        Action("q", "Set lane pace", frozenset({KIND_FLOW}), _needs("level"), False, _lane_pace, remote=True,
               client_text=_rule_refusal_text),
        Action("S", "Switch lane preset", frozenset({KIND_FLOW}), _needs("preset"), False, _lane_preset,
               client_text=_rule_refusal_text),
        Action("t", "Turn lane step on or off", frozenset({KIND_FLOW}), _needs("step", "state"), False, _lane_step,
               client_text=_rule_refusal_text),
    )}



def _track_invocation(key: str, target: Target, inputs: dict) -> Invocation:
    return Invocation([str(PROGRAM), "track", key, SEPARATOR], Path("."))


def _run_track(key, target, inputs, actor, invocation, run):
    try:
        if key in ("L", track_actions.RETRY):
            created = track_actions.launch(key, target, inputs, actor, run)
            output = f"{tasks.CREATED_LANE_PREFIX}{created}" if key == "L" else created
        else:
            output = json.dumps(track_actions.mutate(key, target, inputs, actor), ensure_ascii=True)
    except subprocess.TimeoutExpired:
        raise TrackRefusal(TrackRefusalCode.LANE_UNAVAILABLE,
                           "The lane start could not be confirmed. Retry the reservation") from None
    except (sqlite3.Error, SchemaError, engine_home.EngineHomeError, OSError):
        raise TrackRefusal(TrackRefusalCode.LANE_UNAVAILABLE, track_actions.STORAGE_PROBLEM) from None
    return ActionResult(invocation, exits.OK, output)


def _track_action(key, spec):
    kinds = {track_actions.COLLECTION} if key == track_actions.NEW else {track_actions.RECORD}
    needs = _needs(*spec.needs)
    if key == track_actions.SETTLE:
        kinds.add(KIND_WAIT)

        def needs(target):
            return track_actions.WAIT_SETTLE.needs if isinstance(target, WaitTarget) else spec.needs

    return Action(key, spec.label, frozenset(kinds), needs, False, partial(_track_invocation, key),
                  optional=spec.optional, remote=key == track_actions.SETTLE)


ACTIONS = {action.key: action for action in (
    *(_track_action(key, spec) for key, spec in track_actions.SPECS.items()),
    Action("d", "Close lane", frozenset({KIND_LANE, KIND_OUTSIDE}), _needs(), True, _close_lane),
    Action("m", "Change model", frozenset({KIND_FLOW, KIND_LANE}), _model_needs, False, _change_model, remote=True),
    Action("D", "Change dial", frozenset({KIND_FLOW}), _needs("step"), False, _dial, remote=True),
    Action("a", "Approve", frozenset({KIND_FLOW, KIND_WAIT}), _needs(), True, _approve, optional=("remember",),
           client_text=_approval_refusal_text),
    Action("k", "Mark", frozenset({KIND_WAIT}), _needs("verdict"), False, _mark, optional=("note",)),
    Action("H", "Put the workspace back on the engine's head", frozenset({KIND_WAIT}), _needs(), True, _restore, remote=True,
           client_text=_restore_refusal_text),
    Action("r", "Reply", frozenset({KIND_WAIT}), _needs(), False, _reply, remote=True, optional=("choice", "text")),
    Action("y", "Accept", frozenset({KIND_WAIT}), _needs(), True, _accept_proposal, remote=True, local_wait_kind=waits.AUTOMATION),
    Action("N", "Reject", frozenset({KIND_WAIT}), _needs(), True, _reject_proposal, remote=True),
    _attention_action("read", "Mark read"),
    _attention_action("unread", "Mark unread"),
    _attention_action("snooze", "Snooze", "until"),
    _attention_action("unsnooze", "Unsnooze"),
    Action("start", "Start a lane", frozenset({KIND_WAIT}), _needs("op"), False, _start_proposal, remote=True,
           optional=("project", "preset", "account", "title"), local_inputs=("preset", "account"), client_text=_start_refusal_text,
           result_text=tasks.created_lane_id),
    Action("dismiss", "Dismiss", frozenset({KIND_WAIT}), _needs(), False, _dismiss_proposal, remote=True,
           client_text=lambda output: output),
    Action("markreviewed", "Mark reviewed", frozenset({KIND_WAIT}), _needs(), False, _mark_reviewed, remote=True,
           client_text=lambda output: output),
    Action("R", "Resume", frozenset({KIND_FLOW}), _needs(), True, _resume, remote=True, client_text=_resume_refusal_text,
           optional=("note",), refused_exits=frozenset({exits.USAGE, exits.BUSY})),
    Action("L", "Start lane", frozenset({KIND_PROJECT}), _needs("text"), False, _start_lane,
           optional=("preset", "level", *track_actions.START_INPUTS), client_text=_start_refusal_text, result_text=tasks.created_lane_id),
    Action("P", "Add project", frozenset({host_actions.KIND_HOST}), _needs("folder"), False, _add_project,
           client_text=_add_project_refusal_text, result_text=_add_project_result),
    Action("s", "Stop", frozenset({KIND_FLOW}), _needs(), True, _stop, remote=True),
    Action("C", "Clean up", frozenset({KIND_FLOW}), _needs(), True, _clean),
    Action("T", "Re-contract", frozenset({KIND_FLOW}), _needs(), True, _recontract, remote=True),
    Action("G", "Digest now", frozenset({host_actions.KIND_HOST}), _needs(), False, _digest_now, remote=True,
           client_text=_digest_refusal_text, refused_exits=frozenset({exits.USAGE, exits.BUSY})),
    Action("w", "Write to agent", frozenset({KIND_FLOW, KIND_LANE, KIND_OUTSIDE}), _needs("text"), False, _write, remote=True),
    _account_action("l", "Rename", "rename", ("label",)),
    _account_action("c", "Set reserve", "set-reserve", ("percent",)),
    _account_action("n", "Disable", "disable", confirm=True),
    _account_action("e", "Enable", "enable"),
    _account_action("p", "Add plan period", "plan", ("plan", "since")),
    _account_action("X", "Remove", "remove", confirm=True),
    Action("accountcheck", "Check connection", frozenset({KIND_ACCOUNT}), _needs(), False, _account_check,
           client_text=_as_given, result_text=_as_given),
)}

def _reconcile_scheduled(tasks: scheduled_store.Store) -> None:
    try:
        scheduled_launchd.reconcile(tasks)
    except scheduled_store.Refused:
        # The definition is committed. The snapshot surfaces the persisted reconciliation problem.
        pass


def _scheduled_action(key: str, label: str, *required: str, confirm: bool = False) -> Action:
    kind = host_actions.KIND_HOST if key == scheduled_definition.CREATE else scheduled_definition.TASK_KIND

    def build(target: Target, inputs: dict) -> Invocation:
        def apply(actor: actors.Actor) -> tuple[int, str]:
            tasks = scheduled_store.Store()
            now = datetime.now(UTC)
            try:
                if key == scheduled_definition.RUN:
                    run_id = target.replayed_run
                    if run_id is None:
                        run_id = scheduled_dispatch.run_now(tasks, target.id, target.revision, inputs["op"], now)
                        scheduled_launchd.kick()
                    return exits.OK, scheduled_definition.encode({"run_id": run_id})
                if key == scheduled_definition.CREATE:
                    result = tasks.create(scheduled_definition.decode(inputs["definition"]), inputs["op"], now)
                else:
                    result = tasks.change(target.id, target.revision, key, inputs, now)
            except scheduled_store.Refused as error:
                return exits.USAGE, str(error)
            _reconcile_scheduled(tasks)
            return exits.OK, scheduled_definition.encode({"id": result.id, "revision": result.revision})

        return Invocation([str(PROGRAM), "cockpit", key, SEPARATOR, *target.key], target.repo, apply)

    return Action(key, label, frozenset({kind}), _needs(*required), confirm, build,
                  client_text=str, result_text=str)


ACTIONS.update(_engine_actions())
ACTIONS.update({action.key: action for action in (
    _scheduled_action(scheduled_definition.CREATE, "Create task", "definition", "op"),
    _scheduled_action(scheduled_definition.UPDATE, "Edit task", "definition"),
    _scheduled_action(scheduled_definition.PAUSE, "Pause"),
    _scheduled_action(scheduled_definition.RESUME, "Resume"),
    _scheduled_action(scheduled_definition.DELETE, "Delete", confirm=True),
    _scheduled_action(scheduled_definition.RUN, "Run now", "op"),
)})


def is_local_only(key: str, target_key: tuple) -> bool:
    """Return the action's transport policy without resolving the target."""
    action = ACTIONS.get(key)
    if key == track_actions.SETTLE:
        return not track_actions.is_decision_wait(target_key)
    if (key in ("y", "N") and len(target_key) == wait_actions.KEY_LENGTH and target_key[0] == KIND_WAIT
            and target_key[3] == scheduled_inbox.APPROVAL):
        return True
    return action is not None and (not action.remote or (
        action.local_wait_kind and len(target_key) == wait_actions.KEY_LENGTH
        and target_key[0] == KIND_WAIT and target_key[3] == action.local_wait_kind
    ))


def remote_inputs(key: str, inputs: dict) -> dict:
    """Remove host-only overrides from a phone action's input."""
    action = ACTIONS.get(key)
    return {name: value for name, value in inputs.items() if name not in action.local_inputs} if action else inputs


# The background work: always-on services, automations, connectors, the digest's settings, priority to the person

_SWITCH = re.compile(f"{background_actions.YES}|{background_actions.NO}")
_PATTERNS[("on", "")] = _SWITCH


def _background(build: Callable[[dict], background_actions.Command]) -> Callable[[HostTarget, dict], Invocation]:
    """Make the build function of a host action from a `background_actions` function."""
    def command(host: HostTarget, inputs: dict) -> Invocation:
        try:
            words, options, positionals = build(inputs)
        except (background_actions.Refusal, connector_contract.Refusal) as refusal:
            raise ActionRefused(str(refusal)) from refusal
        return _command(host, words, options, positionals)
    return command


def _background_refusal_text(output: str) -> str:
    """The last line the command printed, or a fixed text when it is empty or looks like it holds a path."""
    return _last_line(output, BACKGROUND_FAILED_TEXT)


ACTIONS.update({action.key: action for action in (
    Action("connectoradd", "Add connector", frozenset({host_actions.KIND_HOST}), _needs("op", "choices"), True,
           _background(partial(connector_contract.command, "add")), client_text=connector_contract.client_reason,
           result_text=connector_contract.client_receipt, remote=False),
    Action("connectoredit", "Edit connector", frozenset({host_actions.KIND_HOST}),
           _needs("op", "name", "revision", "choices"), True,
           _background(partial(connector_contract.command, "edit")), client_text=connector_contract.client_reason,
           result_text=connector_contract.client_receipt, remote=False),
    Action("connectordisconnect", "Disconnect connector", frozenset({host_actions.KIND_HOST}),
           _needs("op", "name", "revision"), True,
           _background(partial(connector_contract.command, "revoke")), client_text=connector_contract.client_reason,
           result_text=connector_contract.client_receipt, remote=False),
    Action("connectorreplace", "Review replacement", frozenset({host_actions.KIND_HOST}),
           _needs("op", "name", "revision", "candidate", "candidate_revision"), True,
           _background(partial(connector_contract.command, "replace")), client_text=connector_contract.client_reason,
           result_text=connector_contract.client_receipt, remote=False),
    Action("B", "Always-on service", frozenset({host_actions.KIND_HOST}), _needs("name", "on"), False,
           _background(background_actions.service_command), client_text=_background_refusal_text),
    Action("O", "Connect", frozenset({host_actions.KIND_HOST}), _needs("kind"), False,
           _background(background_actions.connect_command), optional=("label", "address"),
           client_text=_background_refusal_text),
    Action("o", "Disconnect", frozenset({host_actions.KIND_HOST}), _needs("name"), False,
           _background(background_actions.disconnect_command), client_text=_background_refusal_text),
    Action("I", "Digest settings", frozenset({host_actions.KIND_HOST}), _needs(), False,
           _background(background_actions.digest_settings_command), optional=background_actions.DIGEST_SETTING_INPUTS,
           client_text=_background_refusal_text),
    Action("J", "Digest source on or off", frozenset({host_actions.KIND_HOST}), _needs("name", "on"), False,
           _background(background_actions.digest_source_command), client_text=_background_refusal_text),
    Action("A", "Automation on or off", frozenset({host_actions.KIND_HOST}), _needs("name", "on"), False,
           _background(background_actions.automation_command), client_text=_background_refusal_text),
    Action("Y", "Give priority to my sessions", frozenset({host_actions.KIND_HOST}), _needs("on"), False,
           _background(background_actions.yield_command), remote=True, client_text=_background_refusal_text),
)})


LOCAL_ONLY_KEYS = frozenset(action.key for action in ACTIONS.values() if not action.remote)


# Log

def _destination(path: Path | None) -> Path:
    return Path(path) if path else engine_home.state_dir() / ACTION_LOG


def _audit_target(target_key: tuple) -> dict:
    if not target_key:
        return {}
    digest = hashlib.sha256(json.dumps(target_key, separators=(",", ":")).encode()).hexdigest()
    return {"kind": target_key[0] if target_key[0] in {KIND_FLOW, KIND_LANE, KIND_GROUP, KIND_OUTSIDE, KIND_WAIT,
                                                     KIND_ACCOUNT, KIND_PROJECT, host_actions.KIND_HOST,
                                                     track_actions.COLLECTION, track_actions.RECORD} else "other",
            "id": digest}


def _write_log(path: Path | None, key: str, target_key: tuple, actor: actors.Actor,
               outcome: str, exit_code: int | None) -> None:
    row = {"time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "key": key if key in ACTIONS else "unknown",
           "target": _audit_target(target_key), "actor": asdict(actor), "outcome": outcome, "exit_code": exit_code}
    if outcome == "refused":
        row["reason"] = "action refused"
    line = json.dumps(_clean(row)) + "\n"
    destination = _destination(path)
    destination.parent.mkdir(mode=LOG_DIRECTORY_MODE, parents=True, exist_ok=True)
    descriptor = os.open(destination, os.O_WRONLY | os.O_APPEND | os.O_CREAT, LOG_FILE_MODE)
    with os.fdopen(descriptor, "a", encoding="utf-8") as log:
        fcntl.flock(log, fcntl.LOCK_EX)
        try:
            log.write(line)
            log.flush()
        finally:
            fcntl.flock(log, fcntl.LOCK_UN)


def _clean(value):
    if isinstance(value, str):
        return strip_controls(value)
    if isinstance(value, (list, tuple)):
        return [_clean(item) for item in value]
    if isinstance(value, dict):
        return {key: _clean(item) for key, item in value.items()}
    return value
