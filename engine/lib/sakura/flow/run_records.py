"""Run records in authenticated task state: what each role run was, how it ended and how to reopen it."""

from __future__ import annotations

import json
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from sakura.accounts.providers import ADAPTERS
from sakura.flow import acp, logread, roles, run_log, task_repository
from sakura.flow.store import TaskStore

RUN_STATES = ("starting", "running", "finished", "aborted", "orphaned")
LIVE_STATES = ("starting", "running")
RESUMABLE_STATES = ("finished", "aborted", "orphaned")
SESSION_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
RUN_NAME = re.compile(r"[A-Za-z0-9_]+-[0-9]+(?:\.[0-9]+)*")
FLOW_ENV_PREFIX = "SAKURA_FLOW_"
LOG_HEAD_BYTES = 1 << 20
# provider -> (environment variable naming its config root, that root under the home directory when unset)
TRANSCRIPT_ROOTS = {provider: (adapter.env_var, adapter.default_dir.removeprefix("~/")) for provider, adapter in ADAPTERS.items()}
SESSION_EVENTS = {"goose": (acp.SESSION_EVENT, None, "session_id"), "claude": ("system", "init", "session_id"), "codex": ("thread.started", None, "thread_id")}
DESCRIBED_FIELDS = ("name", "role", "phase", "provider", "model", "effort", "route", "reason", "host", "state",
                    "started_at", "ended_at", "exit_code", "session_id", "cwd", "account", "resumes", "step")
STEP_OF_ROLE = {"planner": "plan", "plan_reviewer": "plan_review", "builder": "build", "code_reviewer": "task_review",
                "evaluator": "checks"}  # the pipeline step each role serves (`flow/pace_check.py`)
CODEX_SANDBOX = {True: "read-only", False: "workspace-write"}


class RunRecordError(RuntimeError):
    """Raised when a run record cannot be created or updated as asked."""


class RunNotResumable(RuntimeError):
    """Raised when a run has no safe interactive resume."""


def begin(store: TaskStore, name: str, role: str, phase: str, provider: str, model: str, effort: str,
          host: str, cwd: Path, env: dict, *, take_steer: bool = False, route: str = "", reason: str = "",
          account: str | None = None, resumes: str | None = None, account_ref: dict | None = None,
          stored_in: str | None = None, session_id: str = "", pending_text: str | None = None,
          also: Callable[[dict], None] | None = None, step: str = "", acp_binding: dict | None = None) -> list[dict]:
    """Record run `name` as `starting` before its process launches.

    Raises RunRecordError for an unsafe or duplicate name, or when `cwd` is neither the repository root nor a
    workspace recorded in state. With `take_steer` the pending operator steers are taken out of state in the
    same update and returned, so a steer is delivered to exactly one run. `route` and `reason` say which model
    route decided the run and why; both are empty when the router did not decide it (records written before
    routing lack them). `step` is the pipeline step the run serves (`plan`, `plan_review`, `build`, `task_review`, `final_review`, `checks`; empty for a role with none). `also` is called with the state inside the same update, so what the record replaces (the queue
    entries and pending messages a continuation takes over) changes in the same write.
    """
    if not RUN_NAME.fullmatch(name):
        raise RunRecordError(f"run name {name!r} is not a log stem such as build-1")
    taken: list[dict] = []

    def change(state: dict) -> None:
        runs = state.setdefault("runs", {})
        if name in runs:
            raise RunRecordError(f"run {name} is already recorded")
        runs[name] = {
            "name": name, "role": role, "phase": phase, "provider": provider, "model": model, "effort": effort,
            "route": route, "reason": reason, "host": host, "identity": None,
            "cwd": str(checked_cwd(store, state, cwd)), "env": flow_env(env),
            "transcript_root": stored_in or transcript_root(provider, env), "state": "starting", "started_at": _now(),
            "ended_at": None, "exit_code": None, "session_id": session_id, "account": account, "account_ref": account_ref, "resumes": resumes, "step": step,
            **({"pending_text": pending_text} if pending_text is not None else {})}
        if acp_binding is not None:
            runs[name]["acp_binding"] = acp_binding
        if take_steer:
            taken.extend(state.get("steer_pending") or [])
            state["steer_pending"] = []
            if acp_binding is not None and taken:
                runs[name]["acp_steers"] = list(taken)
        if also:
            also(state)

    store.update_state(change)
    return taken


def resume_name(state: dict, root: str) -> str:
    """The name of the next run that continues the conversation whose first run is `root`: `<root>.2`, then `.3`..."""
    taken = [int(match[1]) for name in state.get("runs", {}) if (match := re.fullmatch(rf"{re.escape(root)}\.([0-9]+)", name))]
    return f"{root}.{max(taken, default=1) + 1}"


def take_pending(store: TaskStore, name: str) -> str:
    """Remove and return the message run `name` was to start with and has not launched with ("" when none)."""
    taken: list[str] = []

    def change(state: dict) -> None:
        record = _record(state, name)
        if record.get("pending_text"):
            taken.append(record["pending_text"])
        if "pending_text" in record:
            record["pending_text"] = None

    store.update_state(change)
    return "".join(taken)


def mark_failed(store: TaskStore, name: str, also: Callable[[dict], None] | None = None) -> None:
    """Mark the ended run `name` as stopped without an answer (the provider refused it), with `also` changing the state in
    the same write."""
    def change(state: dict) -> None:
        _record(state, name)["state"] = "aborted"
        if also:
            also(state)

    store.update_state(change)


def mark_running(store: TaskStore, name: str, identity: dict) -> None:
    """Store the launcher identity of run `name` and mark it `running`."""
    def change(state: dict) -> None:
        record = _record(state, name)
        record["identity"] = dict(identity)
        record["state"] = "running"  # its pending message stays until the log shows the run read it (`message_taken`)

    store.update_state(change)


def message_taken(store: TaskStore, record: dict) -> bool:
    """Whether the log of the run `record` shows that it read the message it was started with."""
    if record.get("acp_accepted") is True:
        return True
    head = _log_head(log_path(store, record["name"])).encode(errors="replace")
    return run_log.took_message(roles.RoleSpec(record.get("provider", "")), head)


def settle_pending(store: TaskStore, name: str) -> bool:
    """Clear the pending message of run `name` when its log shows it was read; True when it is clear now."""
    record = store.read_state()["runs"][name]
    if not record.get("pending_text"):
        return True
    if not message_taken(store, record):
        return False
    take_pending(store, name)
    return True


def finish(store: TaskStore, name: str, exit_code: int | None, aborted: bool = False,
           also: Callable[[dict, dict], None] | None = None, *, acp_failure: str = "") -> None:
    """Finalize run `name` with the session id from its log, then write its done file; `also(state, record)` changes
    what else must change with it, in the same write (only when this call ends the run).

    A run already finished, aborted or orphaned keeps its first outcome. The done file appears only after the
    state write, so a watcher that sees it can trust the record.
    """
    provider = store.read_state()["runs"][name]["provider"]
    session_id = session_id_from_log(provider, _log_head(log_path(store, name)))
    taken = message_taken(store, store.read_state()["runs"][name])
    changed: list[bool] = []

    def change(state: dict) -> None:
        record = _record(state, name)
        if record["state"] not in LIVE_STATES:
            return
        record.update(state="aborted" if aborted else "finished", ended_at=_now(), exit_code=exit_code,
                      session_id=session_id or record.get("session_id", ""))
        if record.get("acp_binding"):
            _settle_acp(state, record, taken, acp_failure)
        if (taken or (not aborted and not record.get("acp_binding"))) and record.get("pending_text"):
            record["pending_text"] = None
        if also:
            also(state, record)
        changed.append(True)

    store.update_state(change)
    if changed:
        _write_done(store, name, exit_code)


def reconcile(store: TaskStore, is_alive: Callable[[dict], bool]) -> list[str]:
    """Mark `starting` or `running` records whose process is gone as `orphaned`, with the session id their log holds
    (so the conversation can be continued); return their names.

    `is_alive` gets the launcher identity with `run` and `host` added, the shape of an `active_runs` entry.
    A record without an identity never launched, so it is dead without asking.
    """
    live = {name: run for name, run in store.read_state().get("runs", {}).items() if run["state"] in LIVE_STATES}
    dead = [name for name, run in live.items() if is_dead(run, is_alive)]
    sessions = {name: session_id_of_run(store, live[name]) for name in dead}
    accepted = {name: message_taken(store, live[name]) for name in dead if live[name].get("acp_binding")}
    orphaned: list[str] = []

    def change(state: dict) -> None:
        for name in dead:
            record = state["runs"][name]
            if record["state"] in LIVE_STATES:
                record.update(state="orphaned", ended_at=_now(), session_id=sessions[name] or record.get("session_id", ""))
                if name in accepted:
                    _settle_acp(state, record, accepted[name], "adapter_orphaned")
                    if accepted[name] and record.get("pending_text"):
                        record["pending_text"] = None
                orphaned.append(name)

    if dead:
        store.update_state(change)
    for name in orphaned:
        _write_done(store, name, None)
    return orphaned




def acknowledge_acp_prompt(store: TaskStore, name: str) -> None:
    """Persist the prompt acknowledgement independently of the streamed reply size."""
    def change(state: dict) -> None:
        record = _record(state, name)
        if record["state"] in LIVE_STATES:
            record["acp_accepted"] = True

    store.update_state(change)

def set_acp_permission(store: TaskStore, name: str, pending: bool) -> None:
    """Record a permission check only while its run still owns the request."""
    def change(state: dict) -> None:
        record = _record(state, name)
        if pending and record["state"] in LIVE_STATES:
            record["acp_permission"] = True
        else:
            record.pop("acp_permission", None)

    store.update_state(change)


def _settle_acp(state: dict, record: dict, accepted: bool, failure: str) -> None:
    record.pop("acp_permission", None)
    steers = record.pop("acp_steers", ())
    if failure:
        record["acp_failure"] = failure
    if steers and not accepted:
        queued = state.get("steer_pending") or []
        identities = {entry["id"] for entry in queued}
        state["steer_pending"] = [*(entry for entry in steers if entry["id"] not in identities), *queued]

def is_dead(record: dict, is_alive: Callable[[dict], bool]) -> bool:
    """Whether a `starting` or `running` record has no process: it never launched, or its launcher says it is gone."""
    return not (record["identity"] and is_alive(_entry(record)))


def session_id_of_run(store: TaskStore, record: dict) -> str:
    """The session id the log of the run `record` holds, or an empty string."""
    return session_id_from_log(record.get("provider", ""), _log_head(log_path(store, record["name"])))


def session_id_from_log(provider: str, text: str) -> str:
    """Return the provider session id from the first session event in `text`, or an empty string.

    The id is accepted only when it fully matches the UUID format, so a forged or malformed value never
    reaches an argv.
    """
    if provider not in SESSION_EVENTS:
        return ""
    event_type, subtype, key = SESSION_EVENTS[provider]
    for line in text.splitlines():
        event = _json_object(line)
        if event.get("type") == event_type and (subtype is None or event.get("subtype") == subtype):
            value = event.get(key)
            valid = acp.valid_session_id(value) if provider == "goose" else isinstance(value, str) and SESSION_ID.fullmatch(value)
            return value if valid else ""
    return ""


def resume_argv(record: dict, common_dir: Path) -> list[str]:
    """Return the argv that reopens a finished run interactively within the role's original limits.

    Never carries permission-skipping or hook-trust-bypass flags. Raises RunNotResumable.
    """
    name = record.get("name", "?")
    if record.get("state") not in RESUMABLE_STATES:
        raise RunNotResumable(f"run {name} is {record.get('state')}; stop it first: sakura flow stop <task>")
    session_id = record.get("session_id", "")
    if not (isinstance(session_id, str) and SESSION_ID.fullmatch(session_id)):
        raise RunNotResumable(f"no session id recorded for {name}")
    if record.get("provider") == "claude":
        return _claude_resume(record, session_id, common_dir)
    if record.get("provider") == "codex":
        return _codex_resume(record, session_id, common_dir)
    raise RunNotResumable(f"run {name} has no resumable provider ({record.get('provider')})")


def describe(record: dict, common_dir: Path) -> dict:
    """Return the operator view of a run record: facts, plus its resume argv or why it has none.

    Identity, environment and paths other than the working directory stay out of the view.
    """
    view = {key: record.get(key) for key in DESCRIBED_FIELDS}
    view["step"] = record.get("step") or STEP_OF_ROLE.get(record.get("role", ""), "")  # older records: the role's own step
    if record.get("acp_binding"):
        return {**view, "resume": None, "not_resumable": None, "acp": True, "acp_failure": record.get("acp_failure")}
    try:
        view["resume"], view["not_resumable"] = resume_argv(record, common_dir), None
    except RunNotResumable as refusal:
        view["resume"], view["not_resumable"] = None, str(refusal)
    return view


def log_path(store: TaskStore, name: str) -> Path:
    """Return where run `name` writes its log; derived, never stored."""
    return store.runs_dir / f"{name}.log"


def done_path(store: TaskStore, name: str) -> Path:
    """Return the file whose appearance tells watchers run `name` has ended; derived, never stored."""
    return store.runs_dir / f"{name}.done"


def _claude_resume(record: dict, session_id: str, common_dir: Path) -> list[str]:
    argv = ["claude", "--resume", session_id, "--model", record["model"], "--add-dir", str(common_dir)]
    return argv + ["--tools", roles.EVALUATOR_TOOLS] if record["role"] == "evaluator" else argv


def _codex_resume(record: dict, session_id: str, common_dir: Path) -> list[str]:
    is_read_only = record["role"] in roles.READ_ONLY_ROLES
    argv = ["codex", "resume", session_id, "--sandbox", CODEX_SANDBOX[is_read_only], "-C", record["cwd"]]
    return argv if is_read_only else argv + ["--add-dir", str(common_dir)]


def _record(state: dict, name: str) -> dict:
    try:
        return state["runs"][name]
    except KeyError:
        raise RunRecordError(f"run {name} is not recorded") from None


def _entry(record: dict) -> dict:
    return {"run": record["name"], "host": record["host"], **record["identity"]}


def checked_cwd(store: TaskStore, state: dict, cwd: Path) -> Path:
    """Return `cwd` resolved; raise RunRecordError unless it is the repository root, a workspace recorded in `state`
    or the workspace of a role repository the state records."""
    resolved = Path(cwd).resolve()
    allowed = {store.repo.resolve()}
    allowed.update(Path(entry["path"]).resolve() for entry in state.get("workspaces", {}).values() if entry.get("path"))
    allowed.update(Path(entry["workspace"]).resolve()
                   for entry in (state.get(task_repository.ROLE_REPOSITORIES_KEY) or {}).values())
    if resolved not in allowed:
        raise RunRecordError(f"cwd {resolved} is not the repository or a recorded workspace")
    return resolved


def flow_env(env: dict) -> dict:
    """Return the allowlisted engine variables of `env`: string values whose names carry the flow prefix."""
    return {key: value for key, value in env.items() if key.startswith(FLOW_ENV_PREFIX) and isinstance(value, str)}


def transcript_root(provider: str, env: dict, home: Path | None = None) -> str:
    """Return the provider config root `env` selects (default `<home>/<default dir>`), or "" for an unknown provider."""
    if provider not in TRANSCRIPT_ROOTS:
        return ""
    variable, default = TRANSCRIPT_ROOTS[provider]
    configured = env.get(variable)
    return str(Path(configured).expanduser().resolve() if configured else (home or Path.home()) / default)


def _write_done(store: TaskStore, name: str, exit_code: int | None) -> None:
    store.runs_dir.mkdir(parents=True, exist_ok=True)
    done_path(store, name).write_text("" if exit_code is None else f"{exit_code}\n")


def _log_head(path: Path) -> str:
    try:
        return logread.read_head(path, LOG_HEAD_BYTES)
    except (FileNotFoundError, logread.UnreadableLog):
        return ""


def _json_object(line: str) -> dict:
    try:
        event = json.loads(line)
    except ValueError:
        return {}
    return event if isinstance(event, dict) else {}


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")
