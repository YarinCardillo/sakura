"""Remove task workspaces, local build branches, the task's own repository and hook marks after a merged pull
request."""

from __future__ import annotations

from pathlib import Path

from sakura.flow import gitutil, process, stacks as stack_tracker, task_repository, workspaces
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git
from sakura.flow.pr_status import read as _pr_state

DISCARDABLE = ("failed", "stopped")


def run(ctx: FlowContext) -> PhaseOutcome:
    """Clean task resources only after GitHub reports its pull request merged."""
    state = ctx.store.read_state()
    if state["status"] == "cleaned":
        return PhaseOutcome("next")
    pr_url = state.get("pr", {}).get("url", "")
    if _pr_state(pr_url) != "MERGED":
        return PhaseOutcome("blocked", "pull request not merged")
    failure = _remove_task(ctx, state)
    if failure:
        return failure
    _close(ctx, "removed workspaces, ports, local build branches and the task's repository")
    return PhaseOutcome("next")


def discard(ctx: FlowContext) -> str:
    """Remove a failed or stopped lane the way a merged one is removed, with no pull request to wait for; the work in its
    workspaces is gone. Returns why it could not (empty when done). The caller holds the task lock and checked the status."""
    failure = _remove_task(ctx, ctx.store.read_state())
    if failure:
        return failure.reason
    _close(ctx, "discarded: removed workspaces, ports, local build branches and the task's repository")
    return ""


def _close(ctx: FlowContext, note: str) -> None:
    ctx.store.update_state(lambda current: current.update(active_stacks=[], status="cleaned", reason=""))
    ctx.store.append_ledger("cleanup", note)
    ctx.surface.task_closed(ctx.store)


def _remove_task(ctx: FlowContext, state: dict) -> PhaseOutcome | None:
    """Stop the task's stacks and remove its workspaces, branches and repository; the outcome that blocks when something
    cannot be removed, else None."""
    failure = _stop_stacks(ctx, state.get("active_stacks", []))
    if failure:
        return failure
    try:
        for name in list(state.get("workspaces", {})):
            workspaces.remove(ctx, name)
        _remove_build_branches(ctx, state)
        # A role's own repository goes when its run ends; one is left only by a crash.
        for entry in list((state.get(task_repository.ROLE_REPOSITORIES_KEY) or {}).values()):
            task_repository.remove_role_repository(ctx.store, Path(entry["path"]))
        task_repository.remove(ctx.store)
    except GitError as error:
        return PhaseOutcome("blocked", str(error))
    ctx.store.prune_hook_marks()
    return None


def _stop_stacks(ctx: FlowContext, stacks: list[dict]) -> PhaseOutcome | None:
    registered = ctx.store.read_state().get("stacks", {})
    try:
        for stack_id in registered:
            stack_tracker.stop_registered(ctx, stack_id)
    except RuntimeError as error:
        return PhaseOutcome("blocked", str(error))
    for stack in stacks:
        pid, command = stack["pid"], stack["command"]
        if process.is_alive(pid) and not process.kill_tree(pid, command):
            return PhaseOutcome("blocked", f"stack process {pid} no longer matches its recorded command")
    return None


def _remove_build_branches(ctx: FlowContext, state: dict) -> None:
    """The build branches a task made in the repository itself, before tasks had their own repository."""
    branches = {entry.get("branch") for entry in state.get("build", {}).values()}
    for branch in sorted(name for name in branches if name and _branch_exists(ctx.repo, name)):
        git(ctx.repo, "branch", "-D", branch)


def _branch_exists(repo, branch: str) -> bool:
    return gitutil.run(repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}").returncode == 0
