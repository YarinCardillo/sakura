"""What a person's message to a finished builder leaves in its lane.

A message to an agent that has ended continues its conversation in a resumed run (`flow/agent_resume.py`). That run works
in the task's own workspace and may commit there. The lane must not lose such a commit and must not deliver without it:

- while a resume of one of its runs is live the lane does not merge or move on (`wait_for_resumes`, `settle`);
- when a resume ended with commits that the lane's record of the task does not hold, the task goes through the same code
  review and merge as a fix round and the lane goes back to build (`settle`), so the final review, the checks and the
  delivery see them;
- when the lane had gone on (it pushed its branch or was delivered) before the resume committed, it says in plain words what
  is not in the branch, and resuming it reviews, merges and pushes the commits before it goes on to the pull request; a
  stopped or failed lane says it in its ask and does nothing else (`flag_late_commits`).

A resume of the final fix builder works in workspace 0: its commits are taken into the engine's head the way a final fix
round's are. Only commits of resumed runs count: a commit nobody asked for in a workspace stays ignored.
"""

from __future__ import annotations

import os
import time
from pathlib import Path

from sakura.flow import ask_words, delivery, engine, final_head, run_records, run_view, waits
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git
from sakura.flow.phases import build_task, integrate
from sakura.flow.store import TaskStore

POLL_SECONDS = 1.0
DELIVERED_STATUS = "report_ready"
BLOCKED_STATUS = "blocked"
NOTE_ONLY_STATUSES = waits.NOTE_ONLY_STATUSES
ZERO_WORKSPACE = "0"
ZERO_LABEL = "final-resume"
FIRST_PHASE = "build"
WAITING_NOTE = "a message you sent to an agent is still being worked on: the lane waits for it before it merges or moves on"
STOPPED_WHILE_WAITING = "stopped while waiting for a message to an agent to be worked on"


def settle(ctx: FlowContext) -> PhaseOutcome | None:
    """Before the lane runs its next phase: wait for live resumes, then take their commits into the lane.

    Returns a `stopped` outcome when the lane is stopped while it waits, a `blocked` one when a commit of workspace 0 cannot
    be integrated, else None. A lane that is before build has no workspaces to settle. Commits found send the lane back to
    build (tasks reopened for review, workspace 0's commits in the engine's head), as far back as the delivery."""
    if engine.PHASE_ORDER.index(ctx.store.read_state()["phase"]) < engine.PHASE_ORDER.index(FIRST_PHASE):
        return None
    if wait_for_resumes(ctx):
        return PhaseOutcome("stopped", STOPPED_WHILE_WAITING)
    state = ctx.store.read_state()
    late, zero = late_commits(ctx.store, state), late_in_zero(state)
    if zero:
        failure = _integrate_zero(ctx, zero[0])
        if failure:
            return failure
    if late or zero:
        _reopen(ctx, late, zero)
    return None


def wait_for_resumes(ctx: FlowContext) -> bool:
    """Block while a resumed run of the lane is live; True when the lane was stopped while it waited."""
    noted = False
    while _live_resumes(ctx.store):
        if ctx.store.stop_file.exists():
            return True
        if not noted:
            ctx.store.append_ledger("build", WAITING_NOTE)
            noted = True
        time.sleep(POLL_SECONDS)
    return False


def late_commits(store: TaskStore, state: dict) -> dict[str, tuple[str, int]]:
    """Merged task id -> (head of its workspace, commits on it the lane has not taken) for each task a resumed run worked
    in and moved on from the commit the lane recorded for it."""
    worked_in = _resumed_in(state)
    found = {}
    for task_id, entry in state.get("build", {}).items():
        path = _workspace_path(state, entry.get("workspace"))
        if entry.get("status") == build_task.MERGED and path and os.path.realpath(path) in worked_in:
            ahead = _commits_ahead(path, entry["last_head"])
            if ahead:
                found[task_id] = (git(path, "rev-parse", "HEAD").strip(), ahead)
    return found


def late_in_zero(state: dict) -> tuple[str, int] | None:
    """(head of workspace 0, commits on it past the engine's head) when a resumed run worked there (the final fix builder)
    and left some; None otherwise."""
    path = _workspace_path(state, ZERO_WORKSPACE)
    if not path or os.path.realpath(path) not in _resumed_in(state):
        return None
    ahead = _commits_ahead(path, final_head.recorded(state))
    return (git(path, "rev-parse", "HEAD").strip(), ahead) if ahead else None


def flag_late_commits(store: TaskStore) -> None:
    """After a resumed run ended: commits the lane has not taken are never dropped in silence.

    A lane that went on past them says what is not in the branch, in the ask the person reads: one that was delivered or that
    pushed its branch and is blocked is blocked with that sentence (and why it was stuck), so resuming it takes the commits;
    one that is stopped or failed keeps its status and says it in its ask (`late_work`). A lane that is running, waiting for an
    approval, or blocked before it pushed takes them when it next runs (`settle`)."""
    state = store.read_state()
    count = _count(late_commits(store, state), late_in_zero(state))
    if not count:
        return
    status, is_pushed = state.get("status"), delivery.of(state) is not None
    if status in NOTE_ONLY_STATUSES:
        _remember(store, count, is_pushed)
    elif status == DELIVERED_STATUS or (status == BLOCKED_STATUS and is_pushed):
        _block(store, state, count)


def _remember(store: TaskStore, count: int, is_pushed: bool) -> None:
    store.update_state(lambda current: current.update({waits.LATE_KEY: {"count": count, "pushed": is_pushed}}))
    store.append_ledger("report", ask_words.late_work(count, store.read_state()["status"], is_pushed))


def _block(store: TaskStore, state: dict, count: int) -> None:
    was = (state.get(waits.LATE_KEY) or {}).get("reason", "" if state["status"] == DELIVERED_STATUS else state.get("reason", ""))
    sentence = ask_words.late_work(count, state["status"] if state["status"] == DELIVERED_STATUS else BLOCKED_STATUS, True)
    plain = ask_words.blocked_words(was)[0] if was else ""
    reason = f"{sentence} {plain}".strip()

    def block(current: dict) -> None:
        if current.get("status") in (DELIVERED_STATUS, BLOCKED_STATUS):
            current.update(status=BLOCKED_STATUS, reason=reason, **{waits.LATE_KEY: {"count": count, "reason": was}})

    store.update_state(block)
    store.append_ledger("report", sentence)


def _count(late: dict, zero: tuple[str, int] | None) -> int:
    return sum(ahead for _, ahead in late.values()) + (zero[1] if zero else 0)


def _resumed_in(state: dict) -> set[str]:
    return {os.path.realpath(record["cwd"]) for record in state.get("runs", {}).values() if record.get("resumes")}


def _live_resumes(store: TaskStore) -> list[str]:
    records = (record for record in store.read_state().get("runs", {}).values() if record.get("resumes"))
    return [record["name"] for record in records
            if run_view.effective(store, record)["state"] in run_records.LIVE_STATES]


def _workspace_path(state: dict, name: object) -> str:
    recorded = state.get("workspaces", {}).get(name or "", {})
    path = recorded.get("path", "")
    return path if path and Path(path).is_dir() else ""


def _commits_ahead(path: str, recorded_head: str) -> int:
    """Commits of the workspace after `recorded_head`; 0 when the workspace does not hold that commit any more (a branch
    that was moved elsewhere is not work to take) or cannot be read."""
    try:
        if int(git(path, "rev-list", "--count", f"HEAD..{recorded_head}").strip()):
            return 0
        return int(git(path, "rev-list", "--count", f"{recorded_head}..HEAD").strip())
    except (GitError, ValueError):
        return 0


def _integrate_zero(ctx: FlowContext, head: str) -> PhaseOutcome | None:
    """Take the commits a resume of the final fix builder left in workspace 0 into the engine's head, as a final fix round's
    commit is taken (protected paths checked, built on the head); the final review and the checks then see them."""
    start = final_head.recorded(ctx.store.read_state())
    try:
        integrate.check_protected(ctx, "a message to the builder", start, head)
        final_head.fix(ctx, ZERO_LABEL, head)
    except final_head.MergeBlocked as error:
        return PhaseOutcome("blocked", str(error))
    return None


def _reopen(ctx: FlowContext, late: dict[str, tuple[str, int]], zero: tuple[str, int] | None) -> None:
    def reopen_tasks(state: dict) -> None:
        state.pop(waits.LATE_KEY, None)
        for task_id, (head, _) in late.items():
            state["build"][task_id].update(status=build_task.REVIEW, last_head=head, approved_head="", no_change="")

    ctx.store.update_state(reopen_tasks)
    for task_id, (_, ahead) in late.items():
        ctx.store.append_ledger("build", f"{task_id}: {ahead} commit(s) from a message to its builder go through review "
                                         "and merge like a fix round")
    if zero:
        ctx.store.append_ledger("build", f"{zero[1]} commit(s) from a message to the final builder join the engine's head "
                                         "and go through the final review")
    engine.reopen_phase(ctx, FIRST_PHASE, "reopened: work from a message to a builder is part of the lane")
