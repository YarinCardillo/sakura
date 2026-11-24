"""Build phase: parallel builders in their own worktrees, code review, the engine's ordered merge of the approved
commits in its own store (`flow/final_head.py`), full tests on its head."""

from __future__ import annotations

import hashlib
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from datetime import datetime, timezone
from pathlib import Path

from sakura.flow import config, final_head, plan, resumed_work, workspaces
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git
from sakura.flow.phases import build_task, final_review, integrate

ZERO_WORKSPACE = "0"
UNSTARTED = (build_task.PENDING, build_task.BUILDING, build_task.REVIEW)


def worker_count(values: dict) -> int:
    """Builders that may run at once: the preset's builders clamped into `builders.min`..`builders.max`.

    Bare `values` without knobs (a dial and bounds only) take the dial's builders.
    """
    bounds = values["builders"]
    builders = values["knobs"]["builders"] if "knobs" in values else config.DIAL[values["dial"]].builders_cap
    return max(bounds["min"], min(bounds["max"], builders))


def inputs_hash(ctx: FlowContext) -> str:
    """Digest of the approved plan, the saved contract and the base commit."""
    state = ctx.store.read_state()
    material = f"{state['plan_sha256']}\0{state['contract_sha256']}\0{state['base_commit']}"
    return hashlib.sha256(material.encode()).hexdigest()


def run(ctx: FlowContext) -> PhaseOutcome:
    """Build every plan task, merge them in plan order and run the full tests on the task branch."""
    state = ctx.store.read_state()
    recorded = state["workspaces"].get(ZERO_WORKSPACE)
    if not recorded:
        return PhaseOutcome("blocked", f"workspace {ZERO_WORKSPACE} is missing; the contract phase must run first")
    _record_started(ctx)
    try:
        tasks = plan.load(ctx.store, state["plan_sha256"])
        _reset(ctx, tasks)
        failure = _build_and_merge(ctx, tasks, Path(recorded["path"])) or _protected_on_the_head(ctx)
        return failure or integrate.run_full_tests(ctx) or final_review.run(ctx) or PhaseOutcome("next")
    except (plan.PlanError, GitError, workspaces.WorkspaceError) as error:
        return PhaseOutcome("blocked", str(error))


def _protected_on_the_head(ctx: FlowContext) -> PhaseOutcome | None:
    """The protected-path check over everything the engine's head changes from the base, the contract flows the
    engine committed included (each must still be the blob it committed), read from its store."""
    state = ctx.store.read_state()
    try:
        integrate.check_protected(ctx, "the engine's head", state["base_commit"], final_head.recorded(state),
                                  exempt=final_head.flow_blobs(state))
    except integrate.MergeBlocked as error:
        return PhaseOutcome("blocked", str(error))
    return None


def _record_started(ctx: FlowContext) -> None:
    """Record the build phase before its code-review sub-steps enter history."""
    entry = {"phase": "build", "status": "started", "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
             "detail": "build tasks started"}
    ctx.store.update_state(lambda state: state["history"].append(entry))


def _reset(ctx: FlowContext, tasks: list[plan.BuildTask]) -> None:
    """Create the records of new tasks and give every unmerged task a fresh round budget.

    A task that was not merged when the plan changed keeps its id but is a new piece of work: it starts with no
    failures, so the failures of the old plan cannot route it to Opus.
    """
    def change(state: dict) -> None:
        state["full_green_head"] = None
        state["full_green_inputs_sha256"] = None
        state.pop("merge_conflict", None)
        for task in tasks:
            entry = state["build"].setdefault(task.id, build_task.new_entry())
            if entry["status"] != build_task.MERGED:
                entry["rounds"] = 0
                entry["conflict_rounds"] = 0
                if entry.setdefault("plan_sha256", state["plan_sha256"]) != state["plan_sha256"]:
                    entry.update(failures=0, plan_sha256=state["plan_sha256"])

    ctx.store.update_state(change)


def _build_and_merge(ctx: FlowContext, tasks: list[plan.BuildTask], zero: Path) -> PhaseOutcome | None:
    order = plan.order(tasks)
    by_id = {task.id: task for task in tasks}
    running: dict[Future, str] = {}
    failures: list[PhaseOutcome] = []
    limit = worker_count(ctx.config.values)
    ctx.store.append_ledger("build", f"{len(tasks)} build tasks on up to {limit} builders")
    with ThreadPoolExecutor(max_workers=limit) as pool:
        while True:
            if not failures:
                failures += _advance(ctx, zero, by_id, order, running, pool, limit)
            if not running:
                break
            done, _ = wait(running, return_when=FIRST_COMPLETED)
            failures += [outcome for outcome in (_finish(running, future) for future in done) if outcome]
    build = ctx.store.read_state()["build"]
    if not failures and any(build[task_id]["status"] != build_task.MERGED for task_id in by_id):
        return PhaseOutcome("blocked", "no build task is ready; the plan has a dependency cycle")
    return _first(failures)


def _advance(ctx, zero, by_id, order, running, pool, limit) -> list[PhaseOutcome]:
    """Merge what is ready in plan order, then start every ready task that fits the worker limit."""
    try:
        if resumed_work.wait_for_resumes(ctx):
            return [PhaseOutcome("stopped", resumed_work.STOPPED_WHILE_WAITING)]
        _merge_ready(ctx, zero, by_id, order)
        if ctx.store.stop_file.exists():
            return [PhaseOutcome("stopped", "stopped during build")]
        _submit_ready(ctx, zero, by_id, order, running, pool, limit)
    except (GitError, workspaces.WorkspaceError, integrate.MergeBlocked) as error:
        return [PhaseOutcome("blocked", str(error))]
    return []


def _merge_ready(ctx: FlowContext, zero: Path, by_id: dict, order: list[str]) -> None:
    """Merge every approved task, in plan order, into the engine's head: the exact commit the approval recorded,
    never its branch, which a role or a survivor can move after the approval."""
    build = ctx.store.read_state()["build"]
    for task_id in order:
        status = build[task_id]["status"]
        if status == build_task.MERGED:
            continue
        if status != build_task.APPROVED:
            return
        entry = build[task_id]
        integrate.check_protected(ctx, f"build task {task_id}", entry["start"], entry["approved_head"])
        try:
            head = final_head.merge(ctx, task_id, by_id[task_id].title, entry["approved_head"])
        except final_head.MergeConflict as conflict:
            _give_a_round_to_resolve(ctx, task_id, by_id, conflict)
            return
        build_task.patch(ctx, task_id, status=build_task.MERGED)
        ctx.store.append_ledger("build", f"merged {task_id} into the engine's head {head[:12]}")
        _show(ctx, zero)


def _give_a_round_to_resolve(ctx: FlowContext, task_id: str, by_id: dict, conflict: final_head.MergeConflict) -> None:
    """A task whose approved work conflicts with what is merged is work for its builder: the engine's head is merged into its
    workspace, the conflicts left for the builder to resolve keeping both tasks' intent, and the task goes through review and
    merge again. Once per run of the phase; a conflict that survives is recorded for the ask and raised."""
    state = ctx.store.read_state()
    entry = state["build"][task_id]
    titles = _titles_that_changed(state, by_id, task_id, conflict.files)
    if entry.get("conflict_rounds", 0) >= 1:
        ctx.store.update_state(lambda current: current.update(
            merge_conflict={"task": task_id, "files": conflict.files, "titles": titles}))
        raise conflict
    head = final_head.recorded(state)
    _start_merge_in(ctx, Path(state["workspaces"][entry["workspace"]]["path"]), head)
    build_task.patch(ctx, task_id, status=build_task.BUILDING, start=head, approved_head="", findings=[
        _conflict_finding(titles, conflict.files)], conflict_rounds=entry.get("conflict_rounds", 0) + 1)
    ctx.store.append_ledger("build", f"{task_id} conflicts with merged work in {', '.join(conflict.files)}: its builder "
                                     "gets a round to resolve it")


def _start_merge_in(ctx: FlowContext, workspace: Path, head: str) -> None:
    """Merge the engine's head into the task's workspace without committing, so the builder finds the conflicts to resolve."""
    final_head.publish(ctx, head)
    try:
        git(workspace, "merge", "--no-ff", "--no-commit", final_head.PUBLISHED_REF)
    except GitError:
        if not git(workspace, "ls-files", "--unmerged").strip():
            raise  # not a conflict: the merge itself failed


def _titles_that_changed(state: dict, by_id: dict, task_id: str, files: list[str]) -> list[str]:
    """The titles of the merged tasks that changed any of `files`, then the title of `task_id`."""
    touched = []
    for other, entry in state["build"].items():
        recorded = state["workspaces"].get(entry.get("workspace") or "", {})
        if other != task_id and entry["status"] == build_task.MERGED and recorded.get("path") and set(files) & set(
                git(Path(recorded["path"]), "diff", "--name-only", f"{entry['start']}..{entry['approved_head']}").split()):
            touched.append(by_id[other].title)
    return [*touched, by_id[task_id].title]


def _conflict_finding(titles: list[str], files: list[str]) -> str:
    others = ", ".join(f'"{title}"' for title in titles[:-1]) or "another task"
    return (f"Your work conflicts with work already merged into the lane: {others} also changed {', '.join(files)}. A merge "
            f"of the lane's current work is in progress in your folder, with the conflicts left in the files. Resolve every "
            f'conflict keeping the intent of both tasks (yours is "{titles[-1]}"), run the focused tests and commit the '
            "resolution. Leave nothing uncommitted.")


def _show(ctx: FlowContext, zero: Path) -> None:
    """Move workspace 0, the person's view of the task branch, to the engine's head; a view that cannot move (a
    change there) is noted and left, since nothing reads it back."""
    try:
        final_head.show(ctx, zero, ctx.store.read_state()["branch"])
    except GitError as error:
        ctx.store.append_ledger("build", f"workspace 0 still shows another revision than the engine's head: {error}")


def _submit_ready(ctx, zero, by_id, order, running, pool, limit) -> None:
    build = ctx.store.read_state()["build"]
    available = limit - len(running)
    ready = []
    for task_id in order:
        if len(ready) >= available:
            break
        if task_id in running.values() or build[task_id]["status"] not in UNSTARTED:
            continue
        task = by_id[task_id]
        if all(build[dep]["status"] == build_task.MERGED for dep in task.depends_on) \
                and not _waits_for_a_task_with_the_same_files(task, by_id, order, build):
            workspace = _open_workspace(ctx, zero, task, list(by_id).index(task_id))
            ready.append((task_id, task, workspace))
    for task_id, task, workspace in ready:
        running[pool.submit(_work, ctx, task, workspace)] = task_id


def _waits_for_a_task_with_the_same_files(task: plan.BuildTask, by_id: dict, order: list[str], build: dict) -> bool:
    """Whether a task before this one in the plan, which lists a file this one lists, is not merged yet: the two would each
    create or change that file on their own, so this one starts on top of the other's merged work."""
    earlier = order[:order.index(task.id)]
    return any(build[other]["status"] != build_task.MERGED and plan.shares_files(by_id[other], task) for other in earlier)


def _open_workspace(ctx: FlowContext, zero: Path, task: plan.BuildTask, index: int) -> workspaces.Workspace:
    state = ctx.store.read_state()
    branch = f"{state['branch']}-{task.id.lower()}"
    # The engine's own head, never workspace 0's: a survivor may have moved that branch after the last merge.
    start = state["build"][task.id]["start"] or final_head.recorded(state)
    final_head.publish(ctx, start)
    workspace = workspaces.create(ctx, str(index + 1), branch, start)
    entry = state["build"][task.id]
    build_task.patch(ctx, task.id, workspace=workspace.name, branch=workspace.branch, start=start,
                     last_head=entry["last_head"] or start)
    return workspace


def _work(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace) -> PhaseOutcome | None:
    try:
        return build_task.work(ctx, task, workspace)
    except GitError as error:
        return PhaseOutcome("blocked", f"{task.id}: {error}")


def _finish(running: dict[Future, str], future: Future) -> PhaseOutcome | None:
    running.pop(future)
    return future.result()


def _first(failures: list[PhaseOutcome]) -> PhaseOutcome | None:
    """A stop wins over a block; otherwise the first failure in the order it happened."""
    stopped = [failure for failure in failures if failure.kind == "stopped"]
    return (stopped or failures or [None])[0]
