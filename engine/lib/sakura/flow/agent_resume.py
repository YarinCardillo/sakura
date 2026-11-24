"""Continuing an agent's conversation after its run ended: the person's message starts a new run of the same chain.

The new run is named `<first run>.<n>`, records the run it continues (`resumes`) and has the same role, model, effort,
folder and account. It goes through the guard like any launch (pace, account reserve, budget); a refusal is returned in
words. A Claude run resumes with `--resume` and the message as its first stream-json line, so it takes live input like any
Claude run; a Codex run resumes as `codex exec resume` with the message on stdin. One run of a chain is started at a time:
the decision and the record are made under the chain's lock, and a message that arrives while one starts finds its run
`starting` and goes to it live, or is held for it.

A worker process (`sakura flow agent-worker`) supervises the run, as the engine does for the runs of a lane: the engine
may not be running when the person writes to a finished agent. A worker also waits for a running run that takes no live
input to end and then continues it with the messages held for it. One worker waits per agent.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import subprocess
import time
from collections.abc import Iterator
from pathlib import Path

from sakura import interpreter, repository
from sakura.flow import agent_feed, agent_messages, budget_guard, conversations, logread, run_log, resume_account, resumed_work, roles, run_records, run_view, surface, task_repository
from sakura.flow.config import FlowConfig
from sakura.flow.render import strip_controls
from sakura.flow.runner import ResumeOf, RunRequest
from sakura.flow.store import TaskStore

WAIT_POLL_SECONDS = 2.0
MAX_WAIT_SECONDS = 6 * 60 * 60
JOINER = "\n\n"
ANOTHER_AGENT = "The lane now runs this role on another agent, so this conversation cannot be continued."


class ResumeRefused(RuntimeError):
    """Raised when a conversation cannot be continued now; the message is a sentence for the person."""


@contextlib.contextmanager
def chain_lock(store: TaskStore, run: str) -> Iterator[None]:
    """Hold the lock of the chain `run` belongs to: one run of a chain is decided and recorded at a time."""
    with _lock_file(store, run, "chain") as descriptor:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield


@contextlib.contextmanager
def queue_guard(store: TaskStore, run: str) -> Iterator[None]:
    """Hold the guard of the chain's queue: a message is held and a worker woken, or a worker decides it has nothing left
    and lets go of its place, one at a time, so the two can never miss each other."""
    with _lock_file(store, run, "queue") as descriptor:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield


@contextlib.contextmanager
def workspace_guard(store: TaskStore, workspace: str) -> Iterator[None]:
    """Hold the queue guard of every chain that ran in the folder `workspace`, so a decision about the folder (keep it for a
    message that waits, or remove it) cannot interleave with a message being held for those chains."""
    runs = store.read_state().get("runs", {})
    roots = sorted({_root(runs, name) for name, record in runs.items()
                    if os.path.realpath(record["cwd"]) == os.path.realpath(workspace)})
    with contextlib.ExitStack() as stack:
        for root in roots:
            stack.enter_context(queue_guard(store, root))
        yield


def reconcile_retained(store: TaskStore) -> None:
    """Remove every retained review copy that nothing needs any more: no message waits for it, no continuation is starting or
    running in it and no run is alive in it. A worker that dies after delivering its last message would leave it."""
    state = store.read_state()
    kept = {os.path.realpath(folder) for folder in state.get(task_repository.RETAINED_KEY, [])}
    for entry in (state.get(task_repository.ROLE_REPOSITORIES_KEY) or {}).values():
        if os.path.realpath(entry["path"]) not in kept:
            continue
        with workspace_guard(store, entry["workspace"]):
            current = store.read_state()
            alive = any(os.path.realpath(record["cwd"]) == os.path.realpath(entry["workspace"])
                        and record["state"] in run_records.LIVE_STATES for record in current.get("runs", {}).values())
            if not alive and not agent_messages.needs_workspace(current, entry["workspace"]):
                task_repository.remove_role_repository(store, Path(entry["path"]))


def _root(runs: dict, name: str) -> str:
    return agent_feed._root_of(runs, name)


class _Place:
    """A worker's place as the one that waits for its agent: held until `release`, which is idempotent."""

    def __init__(self, descriptor: int | None) -> None:
        self._descriptor = descriptor

    def __bool__(self) -> bool:
        return self._descriptor is not None

    def release(self) -> None:
        """Let go of the place (another worker may take it)."""
        if self._descriptor is not None:
            os.close(self._descriptor)
            self._descriptor = None


@contextlib.contextmanager
def worker_lock(store: TaskStore, run: str) -> Iterator[_Place]:
    """Hold the right to wait for the chain of `run`; the place is false when another worker already has it."""
    chain = agent_feed.chain_of(store, run)[0]["name"]
    store.runs_dir.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(store.runs_dir / f"{chain}.worker.lock", os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(descriptor)
        descriptor = None
    place = _Place(descriptor)
    try:
        yield place
    finally:
        place.release()


def hold_for(store: TaskStore, run: str, text: str, actor: str, kind: str = "steer", command: str = "") -> None:
    """Hold `text` for `run` and see that a worker delivers it. Raises agent_messages.HoldRefused (the queue is full, or
    the folder the run worked in is gone)."""
    with queue_guard(store, run):
        if not Path(agent_feed.chain_of(store, run)[-1]["cwd"]).is_dir():
            raise agent_messages.HoldRefused(conversations.folder_gone(store.read_state()))  # the reviewer's copy was removed meanwhile
        agent_messages.hold(store, run, text, actor, kind, command)
        _spawn_or_note(store, run)


def wake(store: TaskStore, run: str) -> None:
    """A message was just held for `run` by another path: see that a worker will deliver it."""
    try:
        with queue_guard(store, run):
            _spawn_or_note(store, run)
    except agent_feed.FeedRefused:
        return  # the run is not recorded: there is nothing to continue


def _spawn_or_note(store: TaskStore, run: str) -> None:
    """Start the worker that waits for `run`; when no process can be started the message stays held, and the ledger says so
    (the next message or wake tries again)."""
    try:
        spawn_worker(store, run)
    except OSError as error:
        store.append_ledger("runner", strip_controls(f"{run}: a worker for its held messages could not be started: {error}"))


def start(store: TaskStore, cfg: FlowConfig, common_dir: Path, run: str, text: str) -> str:
    """Begin the run that continues the chain of `run` and return its name. The record carries the session it continues and
    the message it starts with (`text`, after what is held for the agent and what an earlier start never launched with),
    which stay in it until the run launches. The caller holds `chain_lock`. Raises ResumeRefused."""
    chain = agent_feed.chain_of(store, run)
    newest = chain[-1]
    how, problem = run_view.listening(store, store.read_state(), newest, common_dir)
    if how != run_view.RESUME:
        raise ResumeRefused(problem or "It is running now.")
    account = _require_launch(store, cfg, newest)
    state = store.read_state()
    held = agent_messages.held_for(state, {record["name"] for record in chain})
    unlaunched = [record for record in chain if record.get("pending_text")
                  and not run_records.message_taken(store, record)]
    pending_names = [record["name"] for record in chain if record.get("pending_text")]
    first = JOINER.join([*(record["pending_text"] for record in unlaunched), *(entry["text"] for entry in held),
                         *([text] if text else [])])

    def consume(current: dict) -> None:
        agent_messages.consume_in(current, {entry["id"] for entry in held})
        for name in pending_names:
            current["runs"][name]["pending_text"] = None

    return _begin(store, newest, chain[0]["name"], account, first, consume)



def fresh(ctx, run: str, text: str):
    """Start an independent Goose conversation using the old route and an explicit new prompt."""
    state = ctx.store.read_state()
    record = state.get("runs", {}).get(run)
    if (not record or record.get("provider") != "goose" or not record.get("acp_binding")
            or record.get("state") not in run_records.RESUMABLE_STATES):
        raise ResumeRefused("A fresh attempt needs an ended, bound Goose run.")
    if not text.strip() or ctx.store.stop_file.exists():
        raise ResumeRefused("A fresh attempt needs a message and a lane that is ready to run.")
    cwd = Path(record["cwd"])
    request = RunRequest(record["role"], record["phase"], text, {}, cwd, record.get("env") or {},
                         repository=_role_repository(state, record["cwd"]), step=record.get("step", ""),
                         fresh_from=run)
    return ctx.runner.run(request)

def spawn_worker(store: TaskStore, run: str, begun: bool = False) -> None:
    """Start a worker process that outlives this one: for the begun run `run`, which it launches with the message its
    record holds, or (not `begun`) to wait for the run `run` to end and continue it with what is held."""
    argv = [str(surface.KIT_BIN), "flow", "agent-worker", store.task_id, run, *(["--begun"] if begun else [])]
    env = {**surface.driver_environment(os.environ), **interpreter.environment()}
    store.runs_dir.mkdir(parents=True, exist_ok=True)
    with (store.runs_dir / f"{run}.worker.log").open("ab") as log:
        subprocess.Popen(argv, cwd=store.repo, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                         start_new_session=True)


def run_begun(ctx, name: str) -> bool:
    """Launch the begun run `name` with the message its record holds and supervise it to its end; False when it could not
    be launched (the message is then held again for the agent, which stays resumable)."""
    state = ctx.store.read_state()
    record = state["runs"][name]
    text = record.get("pending_text")
    if not text:
        return True  # launched already
    try:
        resume = ResumeOf(name, record["session_id"], record["model"], record["effort"],
                          resume_account.resolve_record(record))
        is_goose = record["provider"] == "goose"
        env = (record.get("env") or {}) if is_goose else {}
        outcome = ctx.runner.run(RunRequest(record["role"], record["phase"], text, env, Path(record["cwd"]),
                                             step=record.get("step", "") if is_goose else "", resume=resume,
                                             repository=_role_repository(state, record["cwd"])))
    except Exception as refusal:  # whatever stops the launch: the message is held again and the person is told in the ledger
        failure = str(refusal) if record["provider"] == "goose" and isinstance(refusal, roles.RoleRefused) else ""
        abandon(ctx.store, name, failure=failure)
        ctx.store.append_ledger(record["phase"], strip_controls(f"{name} could not continue: {refusal}"))
        return False
    answered = outcome.exit_code == 0 if record["provider"] == "goose" else _answered(ctx.store, name, record, text)
    resumed_work.flag_late_commits(ctx.store)
    return answered


def _answered(store: TaskStore, name: str, record: dict, text: str) -> bool:
    """Whether the run that ended answered: a provider that refused it for a missing login did not, so the run is marked
    stopped, the message is held again and the ledger says what to do."""
    spec = roles.RoleSpec(record["provider"])
    try:
        data = logread.read_log(run_records.log_path(store, name)).encode()
    except (OSError, logread.UnreadableLog):
        return True
    if not run_log.login_failed(spec, data):
        return True
    run_records.mark_failed(store, name, lambda state: agent_messages.hold_back_in(state, record["resumes"], text, "cli:person"))
    store.append_ledger(record["phase"], f"{name} could not continue: Claude is not logged in on this account: run claude "
                                         "/login in a terminal, then write to it again. Your message is kept.")
    return False


def abandon(store: TaskStore, name: str, *, failure: str = "") -> None:
    """The begun run `name` will not launch: end its record (it keeps the session it continues) and hold its message again
    for the agent, in one write."""
    def move(state: dict, record: dict) -> None:
        text, record["pending_text"] = record.get("pending_text"), None
        if text:
            agent_messages.hold_back_in(state, record["resumes"], text, "cli:person")

    run_records.finish(store, name, None, aborted=True, also=move, acp_failure=failure)


def work(ctx, run: str) -> None:
    """Wait for the agent of `run` to end, then continue it with the messages held for it, again for each message held
    meanwhile; ends when none is held, or when a continuation cannot be launched. Only one worker waits per agent."""
    with worker_lock(ctx.store, run) as place:
        try:
            while place:
                _wait_until_ended(ctx.store, run)
                name = _begin_held(ctx, run, place)
                if name is None or not run_begun(ctx, name):
                    return
        finally:
            reconcile_retained(ctx.store)


def _begin_held(ctx, run: str, place: _Place) -> str | None:
    """Begin the run that continues the chain with what is held for it. With nothing left, the worker lets go of its
    place in the same breath it decides so (under the queue's guard); None too when the agent cannot be continued (the
    messages then stay held, and the ledger says why)."""
    store = ctx.store
    with queue_guard(store, run):
        chain = agent_feed.chain_of(store, run)
        names = {record["name"] for record in chain}
        if not agent_messages.held_for(store.read_state(), names) and not any(
                r.get("pending_text") and r["state"] not in run_records.LIVE_STATES for r in chain):
            task_repository.release_retained(store, chain[-1]["cwd"])
            place.release()
            return None
    with chain_lock(store, run):
        try:
            return start(store, ctx.config, repository.common_dir(ctx.repo), run, "")
        except ResumeRefused as refusal:
            store.append_ledger("runner", strip_controls(f"{run}: a message for it is held and cannot be delivered: {refusal}"))
            return None


def _wait_until_ended(store: TaskStore, run: str) -> None:
    deadline = time.monotonic() + MAX_WAIT_SECONDS
    while agent_feed.chain_of(store, run)[-1]["state"] in run_records.LIVE_STATES and time.monotonic() < deadline:
        time.sleep(WAIT_POLL_SECONDS)


def _role_repository(state: dict, cwd: str) -> Path | None:
    """The repository of its own an isolated role ran in, recovered from the state by the workspace it ran in; None for a
    role that works in a workspace of the task's own repository."""
    for entry in (state.get(task_repository.ROLE_REPOSITORIES_KEY) or {}).values():
        if os.path.realpath(entry["workspace"]) == os.path.realpath(cwd):
            return Path(entry["path"])
    return None


def _require_launch(store: TaskStore, cfg: FlowConfig, record: dict) -> resume_account.ResumeAccount:
    """The account the conversation continues on, once the role still runs on the record's provider and the guard, judging
    that account, lets it launch. Raises ResumeRefused."""
    try:
        provider = roles.parse_role(cfg.values["roles"][record["role"]]).provider
    except (KeyError, ValueError):
        provider = ""
    if provider != record["provider"]:
        raise ResumeRefused(ANOTHER_AGENT)
    try:
        account = resume_account.resolve_record(record)
        if provider == "goose":
            if store.stop_file.exists():
                raise ResumeRefused("The lane is stopped. Resume it explicitly before continuing the Goose conversation.")
        else:
            budget_guard.require(store, cfg, provider=provider, role=record["role"], on_account=account.account)
    except resume_account.AccountProblem as problem:
        raise ResumeRefused(str(problem)) from None
    except budget_guard.BudgetWait as wait:
        raise ResumeRefused(strip_controls(str(wait))) from None
    return account


def _begin(store: TaskStore, newest: dict, root: str, account: resume_account.ResumeAccount, first: str, consume) -> str:
    """Record the continuation in one write that also takes over the queue entries and pending messages it carries."""
    name = run_records.resume_name(store.read_state(), root)
    run_records.begin(store, name, newest["role"], newest["phase"], newest["provider"], newest["model"], newest["effort"],
                      newest["host"], Path(newest["cwd"]), newest.get("env") or {}, route=newest.get("route", ""),
                      reason=newest.get("reason", ""), account=newest.get("account"), resumes=newest["name"],
                      account_ref=account.ref, stored_in=str(account.root), step=newest.get("step", ""),
                      session_id=newest["session_id"], pending_text=first, also=consume,
                      acp_binding=newest.get("acp_binding"))
    return name


@contextlib.contextmanager
def _lock_file(store: TaskStore, run: str, kind: str) -> Iterator[int]:
    chain = agent_feed.chain_of(store, run)[0]["name"]
    store.runs_dir.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(store.runs_dir / f"{chain}.{kind}.lock", os.O_CREAT | os.O_RDWR, 0o600)
    try:
        yield descriptor
    finally:
        os.close(descriptor)
