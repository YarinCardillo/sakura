"""Shared builders for flow engine tests."""

from __future__ import annotations

import subprocess
from pathlib import Path
from types import SimpleNamespace

from sakura.flow import config, engine, notify, process, store, workspaces

# Kept at import: some tests replace `workspaces.create` with a double, and `task_workspace` needs the real one.
_CREATE_WORKSPACE = workspaces.create


class FakeRunner:
    """Runner double: calls `behaviour(request)` and records every request; a role named in `by_role` gets that
    result, every other role `result`."""

    def __init__(self, behaviour=None, by_role=None):
        self.requests = []
        self.behaviour = behaviour
        self.result = None
        self.final = ""  # what every run's last message says
        self.by_role = dict(by_role or {})

    def run(self, request):
        self.requests.append(request)
        if self.behaviour:
            self.behaviour(request)
        result = self.by_role.get(request.role, self.result)
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, final=self.final, result=result, log_path=Path("run.log"))


def make_context(repo: Path, runner=None, overrides: dict | None = None, task: str = "demo", state: dict | None = None,
                 own_repository: bool = False):
    """A stored task and its context. `own_repository`: the task's own repository is made, as `flow start` makes it,
    for a test whose commands run caged (every cage is given that repository's Git folder); workspaces the state
    names are recorded after it, whatever repository they belong to."""
    task_store = store.TaskStore(repo, task)
    initial = {"task": task, "ref": "specs/demo.md", "phase": "intake", "status": "running", "reason": "",
               "history": [], "approvals": {}, "completed": {}, "active_run": None, "overrides": overrides or {},
               "base_branch": "main"}
    initial.update(state or {})
    cfg = config.load(repo, home=repo / "nohome", overrides=overrides)
    if own_repository:
        _create_with_own_repository(task_store, cfg, repo, initial)
    else:
        task_store.create(initial)
    return engine.FlowContext(repo, task_store, cfg, runner or FakeRunner(), notify.LedgerNotifier(task_store),
                              config.steps(cfg.values))


def _create_with_own_repository(task_store, cfg, repo: Path, initial: dict) -> None:
    from sakura.flow import task_repository

    workspaces_named = initial.pop("workspaces", None)
    initial.setdefault("base_commit", _head(repo))
    task_store.create(initial)
    task_repository.ensure(task_store, cfg.values, repo)
    if workspaces_named is not None:
        task_store.update_state(lambda current: current.update(workspaces=workspaces_named))


def _head(repo: Path) -> str:
    return subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True,
                          check=True).stdout.strip()


def task_workspace(ctx, name: str = "0", branch: str = "") -> Path:
    """Workspace `name` of the task's own repository, made as the engine makes one but without hooks or setup: every
    role and caged command runs in one (`flow/task_repository.py`). The base is the repository's HEAD unless the
    task's state names one."""
    state = ctx.store.read_state()
    if not state.get("base_commit") or "workspaces" not in state:
        head = _head(ctx.repo)
        state = ctx.store.update_state(lambda current: (current.setdefault("base_commit", head),
                                                        current.setdefault("workspaces", {})))
    chosen = branch or state.get("branch") or f"feat/flow-{ctx.store.task_id}"
    return _CREATE_WORKSPACE(ctx, name, chosen, state["base_commit"], prepared=False).path


def engine_head(ctx, workspace: Path | None = None) -> str:
    """Record `workspace`'s HEAD (workspace 0's by default) as the engine's head, as the build phase leaves it once it
    integrated an approved commit built on the base (`flow/final_head.py`); every later phase proves and delivers
    that head, never a workspace's. Both commits are sealed from `workspace` first, so a test whose workspace 0 is the
    repository itself needs no task repository. Returns the head."""
    from sakura.flow import final_head, sealed

    state = ctx.store.read_state()
    workspace = workspace or Path(state["workspaces"]["0"]["path"])
    head = _head(workspace)
    for revision in dict.fromkeys((state["base_commit"], head)):
        sealed.seal(workspace, revision)
    if head != final_head.recorded(state):
        final_head.fix(ctx, "approved", head)
    return head


def engine_flows(ctx, files: dict, allowed: list[str], message: str = "test(flow): contract flows for demo") -> str:
    """Record `files` as the flows step of the engine's head, as the contract phase does once the base run passed and,
    with code review on, the code reviewer approved exactly that commit (`final_head.approve_flows`). Returns it."""
    from sakura.flow import final_head

    candidate = final_head.flows_candidate(ctx, files, allowed, message)
    if ctx.steps.code_review:
        final_head.approve_flows(ctx, candidate)
    return final_head.record_flows(ctx, candidate, allowed)


class MonitorLauncher:
    """Adapts an old-style `monitor(argv, **kwargs)` callable to the Launcher seam; tests only."""

    host = "local"

    def __init__(self, monitor):
        self._monitor = monitor

    def run(self, request, on_start):
        return self._monitor(
            request.argv, cwd=request.cwd, env=request.env, log_path=request.log_path,
            budget_seconds=request.budget_seconds, idle_seconds=request.idle_seconds, stop_file=request.stop_file,
            on_start=lambda pid: on_start({"pid": pid, "command": process.command_of(pid)}),
            stdin_text=request.stdin_text, on_gap=request.on_gap, on_poll=request.on_poll, nice=request.nice)

    def is_alive(self, identity):
        return False

    def kill(self, identity):
        return None


class RemoteStyleLauncher:
    """A launcher whose identity is an opaque token, never a PID; records what the engine asks of it."""

    def __init__(self, host="far", on_run=None):
        self.host = host
        self.on_run = on_run
        self.requests = []
        self.killed = []
        self.alive = True

    def run(self, request, on_start):
        self.requests.append(request)
        on_start({"host": self.host, "token": "x"})
        if self.on_run:
            self.on_run(request)
        request.log_path.write_text("remote output\n")
        return process.ProcessOutcome(0, False, False, False, 0)

    def is_alive(self, identity):
        return self.alive

    def kill(self, identity):
        self.killed.append(dict(identity))
