"""Timed resume of the flows waiting on the budget: `sakura flow wake`.

For every registered repository, a task waiting on `budget` whose `until` has passed gets the guard again. If it
may run it is resumed through `resume` (detached, as the CLI resumes a flow after an approval); if not, its wait
moves to the new `until`; if the quota is unknown (a stale snapshot, a probe that failed) the wait stays and is
looked at again one wake interval later. A wait that never had an `until` (an ask) is the person's to release.

The wake may run from a background job (the usage archive agent), whose priority and environment a tmux server
would inherit for good: it only resumes flows when a server is already up, and says so once when it does not. Each
resume names, in the flow's ledger, the checkout whose code resumed it.
"""

from __future__ import annotations

from collections.abc import Callable
from datetime import datetime, timedelta
from pathlib import Path

from sakura.budget import guard
from sakura.flow import budget_gate, budget_guard, config, engine, registry, store, surface, tasks

Resume = Callable[[Path, store.TaskStore], None]
AWAITING = "awaiting_approval"
WAKE_INTERVAL = timedelta(minutes=15)  # how often the usage archive agent runs
NO_SERVER = "wake: no tmux server, {count} flows due"


def wake(resume: Resume, now: datetime | None = None, repo_list: Callable[[], list[Path]] = registry.repos,
         server_running: Callable[[], bool] = lambda: True) -> list[str]:
    """Look at every task of every registered repository; return one line per task that was due.

    `now` defaults to the local clock, as the engine reads the `night` window in local time. A task or a repository
    that cannot be woken (broken config, unreadable directory, no tmux for the detached resume) is reported, and the
    others still get their turn. With flows due but no tmux server running (`server_running` is False) nothing is
    resumed or ledgered: one line says how many are waiting for one.
    """
    now = now or budget_guard.local_now()
    due, lines = [], []
    for repo in repo_list():
        found, problems = _due_tasks(repo, now)
        due += found
        lines += problems
    if due and not server_running():
        return [*lines, NO_SERVER.format(count=len(due))]
    return lines + [line for repo, task_id in due if (line := _wake_task(repo, task_id, now, resume))]


def _due_tasks(repo: Path, now: datetime) -> tuple[list[tuple[Path, str]], list[str]]:
    """The tasks of `repo` whose budget wait is over, and a line for what could not be read."""
    try:
        task_ids = list(tasks.known_tasks(repo))
    except OSError as error:
        return [], [f"{repo}: wake failed: {type(error).__name__}: {error}"]
    due, problems = [], []
    for task_id in task_ids:
        try:
            if _is_due(store.TaskStore(repo, task_id).read_state(), now):
                due.append((repo, task_id))
        except Exception as error:  # noqa: BLE001 (one task must not stop the others)
            problems.append(f"{task_id}: wake failed: {type(error).__name__}: {error}")
    return due, problems


def _is_due(state: dict, now: datetime) -> bool:
    until = budget_guard.waiting_until(state)
    return state.get("status") == AWAITING and until is not None and until <= now


def _wake_task(repo: Path, task_id: str, now: datetime, resume: Resume) -> str | None:
    task_store = None
    try:
        task_store = store.TaskStore(repo, task_id)
        if not _is_due(task_store.read_state(), now):
            return None
        return _decide(repo, task_store, now, resume)
    except Exception as error:  # noqa: BLE001 (one task must not stop the others; the failure is ledgered and returned)
        if task_store is not None:
            task_store.append_ledger("budget", f"wake failed: {type(error).__name__}: {error}")
        return f"{task_id}: wake failed: {type(error).__name__}: {error}"


def _decide(repo: Path, task_store: store.TaskStore, now: datetime, resume: Resume) -> str:
    state = task_store.read_state()
    cfg = config.load(repo, overrides=state.get("overrides") or None)
    decision = budget_guard.evaluate(task_store, cfg, now)
    if decision.action == guard.RUN:
        task_store.append_ledger("budget", f"wake resumes the flow with {surface.KIT_BIN}")
        resume(repo, task_store)
        return f"{task_store.task_id}: resumed"
    if decision.action == guard.ASK:
        budget_guard.postpone(task_store, now + WAKE_INTERVAL)
        return f"{task_store.task_id}: quota unknown, waiting on ({decision.reason})"
    budget_gate.record_decision(engine.open_context(repo, task_store), decision, now)
    return f"{task_store.task_id}: still waiting ({budget_guard.ruling_line(decision)})"
