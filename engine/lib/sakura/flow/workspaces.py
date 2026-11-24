"""Git worktrees of a task's own repository (`flow/task_repository.py`), with their port blocks, hooks and setup
commands."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlparse

from sakura import interpreter, slots
from sakura.flow import cage, gitutil, pinned_roots, ports, process, repository_commands, task_repository
from sakura.flow.gitutil import branch_exists, git, tail
from sakura.flow.engine import FlowContext

KIT_BIN = Path(__file__).resolve().parents[3] / "bin" / "sakura"
SETUP_BUDGET_KEY = "contract"


class WorkspaceError(RuntimeError):
    """Raised when a workspace cannot be created or prepared; the message says why."""


@dataclass(frozen=True)
class Workspace:
    """A task worktree with the port block reserved for its app."""

    name: str
    path: Path
    branch: str
    ports: range


def create(ctx: FlowContext, name: str, branch: str, start_point: str, prepared: bool = True) -> Workspace:
    """Create (or reuse) the worktree `<root>/<task>-<name>` of the task's own repository on `branch` at `start_point`.

    Hooks are installed and `workspace.setup` runs once; a recorded workspace whose directory still exists is
    returned unchanged. `prepared=False` (intake, before the person approved any command) leaves both out, and a
    later `create` of the same workspace does them. Raises WorkspaceError when the task's repository cannot be made
    or found (`flow/task_repository.py`).
    """
    path = _path_for(ctx, name)
    owner = _owner(ctx, name)
    origin = ctx.config.values["app"]["origin"]
    block = ports.allocate(owner, preferred_start=urlparse(origin).port if origin else None)
    recorded = ctx.store.read_state()["workspaces"].get(name)
    if recorded and path.is_dir():
        if recorded["ports"] != [block.start, block.stop - 1]:
            ctx.store.update_state(lambda state: state["workspaces"][name].update(
                {"ports": [block.start, block.stop - 1]}))
        if prepared and not recorded.get("prepared", True):
            _prepare(ctx, name, path)
            ctx.store.update_state(lambda state: state["workspaces"][name].update({"prepared": True}))
        return Workspace(name, path, recorded["branch"], block)
    if not (path / ".git").exists():
        _add_worktree(_task_repository(ctx), path, branch, start_point)
    if prepared:
        _prepare(ctx, name, path)
    _record(ctx, name, path, branch, block, prepared)
    return Workspace(name, path, branch, block)


def remove(ctx: FlowContext, name: str) -> None:
    """Delete the worktree (the branch stays), free its ports and forget it."""
    state = ctx.store.read_state()
    recorded = state["workspaces"].get(name)
    path = Path(recorded["path"]) if recorded else _path_for(ctx, name)
    # A task started before tasks had their own repository has worktrees of the person's.
    owner = task_repository.path(state) if state.get(task_repository.STATE_KEY) else ctx.repo
    if path.exists():
        git(owner, "worktree", "remove", "--force", str(path))
    pinned_roots.release(path)
    ports.release(_owner(ctx, name))
    ports.release_prefix(f"{_owner(ctx, name)}/")  # the app sessions' ports, held until the workspace goes
    ctx.store.update_state(lambda state: state["workspaces"].pop(name, None))


def _path_for(ctx: FlowContext, name: str) -> Path:
    return task_repository.workspace_root(ctx.config.values, ctx.repo) / f"{ctx.store.task_id}-{name}"


def _task_repository(ctx: FlowContext) -> Path:
    try:
        return task_repository.ensure(ctx.store, ctx.config.values, ctx.repo)
    except task_repository.TaskRepositoryError as error:
        raise WorkspaceError(str(error)) from error


def _owner(ctx: FlowContext, name: str) -> str:
    return f"{ctx.repo.name}/{ctx.store.task_id}/{name}"


def _add_worktree(repo: Path, path: Path, branch: str, start_point: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if branch_exists(repo, branch):
        git(repo, "worktree", "add", str(path), branch)
    else:
        git(repo, "worktree", "add", "-b", branch, str(path), start_point)


def _prepare(ctx: FlowContext, name: str, path: Path) -> None:
    _require_own_workspace(ctx, path)
    env = {**process.child_environment(os.environ), **interpreter.environment()}
    _run_logged(ctx, name, "install", [str(KIT_BIN), "install", "--files-only"], path, env)
    run_setup(ctx, name, path)


def _require_own_workspace(ctx: FlowContext, path: Path) -> None:
    """Refuse to prepare `path` unless its `.git` names its own entry of the task's recorded repository: a role may
    have worked there already (intake's planner in workspace 0), and the install step runs outside every cage, in the
    folder, finding its repository there (review 10, finding 1)."""
    state = ctx.store.read_state()
    recorded = task_repository.path(state)
    task_repository.pin(state)
    try:
        common = gitutil.verified_common_dir(path)
    except gitutil.GitError as error:
        raise WorkspaceError(f"the Git folder of {path} is not the task repository's own: {error}") from None
    if os.path.realpath(common) != os.path.realpath(recorded):
        raise WorkspaceError(f"the Git folder of {path} is not the task repository's own")


def run_setup(ctx: FlowContext, name: str, path: Path, build_dir: Path | None = None) -> None:
    """Run every `workspace.setup` command in `path`, caged; with `build_dir` the source is read-only and the
    commands install into that folder (the completion checkout). Raises WorkspaceError when one fails."""
    for index, command in enumerate(ctx.config.values["workspace"]["setup"], start=1):
        launch = _launch(ctx, name, f"setup-{index}", command, path, build_dir)
        _run_logged(ctx, name, f"setup-{index}", launch.argv, path, launch.env, is_caged=True)


def _launch(ctx: FlowContext, name: str, step: str, command: str, path: Path,
            build_dir: Path | None) -> repository_commands.Launch:
    """How a `workspace.setup` command runs: a repository's one approved and caged, the host's as it is."""
    try:
        return repository_commands.plan(ctx.store, ctx.config, command, path, f"{name}-{step}", build_dir=build_dir)
    except (repository_commands.CommandNotApproved, cage.CageError) as error:
        raise WorkspaceError(f"workspace {name} {step} not run: {error}") from error


def _run_logged(ctx: FlowContext, name: str, step: str, argv: list[str], cwd: Path, env: dict[str, str],
                is_caged: bool = False) -> None:
    log = ctx.store.operations_dir / f"workspace-{name}-{step}.log"
    log.unlink(missing_ok=True)
    values = ctx.config.values
    try:
        with process.heavy_slot(ctx.config, f"workspace {step}", timeout_seconds=values["timeouts"][SETUP_BUDGET_KEY],
                                stop_file=ctx.store.stop_file):
            scratch = repository_commands.scratch_log(env, log) if is_caged else None
            try:
                outcome = process.tracked_run(
                    ctx,
                    argv, cwd=cwd, env=env, log_path=scratch.path if scratch else log,
                    budget_seconds=values["timeouts"][SETUP_BUDGET_KEY], idle_seconds=values["timeouts"]["idle_seconds"],
                    stop_file=ctx.store.stop_file, log_file=scratch.file if scratch else None)
            finally:
                if scratch:
                    repository_commands.keep_log(scratch, log)
    except slots.SlotTimeout as error:
        raise WorkspaceError(f"{name} {step} stalled waiting for a heavy-work slot") from error
    except slots.SlotStopped as error:
        raise WorkspaceError(f"{name} {step} stopped while waiting for a heavy-work slot") from error
    ctx.store.append_ledger("workspace", f"{name} {step}: exit {outcome.exit_code}, log {log}")
    if outcome.left_running:
        raise WorkspaceError(f"workspace {name} {step} {process.LEFT_RUNNING}; log {log}:\n{tail(log)}")
    if outcome.exit_code != 0:
        raise WorkspaceError(f"workspace {name} {step} failed (exit {outcome.exit_code}); log {log}:\n{tail(log)}")


def _record(ctx: FlowContext, name: str, path: Path, branch: str, block: range, prepared: bool) -> None:
    entry = {"path": str(path), "branch": branch, "ports": [block.start, block.stop - 1], "prepared": prepared}
    ctx.store.update_state(lambda state: state["workspaces"].update({name: entry}))
