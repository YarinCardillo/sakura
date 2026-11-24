import shlex
import sys

from sakura.flow import engine, exits, ports, task_repository, workspaces
from sakura.flow.gitutil import git
from sakura.flow.phases import cleanup
from flow_support import make_context


def test_cleanup_refuses_open_pr(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": "https://example/pull/1"}})
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "OPEN")

    result = cleanup.run(ctx)

    assert result.kind == "blocked"
    assert result.reason == "pull request not merged"


def test_cleanup_removes_worktrees_and_ports_keeps_task_dir(repo, tmp_path, monkeypatch):
    path = tmp_path / "demo-worktree"
    git(repo, "worktree", "add", "-qb", "feature/demo", str(path), "HEAD")
    git(repo, "branch", "feature/demo-t1")
    ctx = make_context(repo, state={
        "phase": "cleanup", "pr": {"url": "https://example/pull/1"},
        "branch": "feature/demo",
        "workspaces": {"0": {"path": str(path), "branch": "feature/demo"}},
        "build": {"t1": {"branch": "feature/demo-t1"}},
    })
    ports.allocate("repo/demo/0")
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")
    killed = []
    monkeypatch.setattr(cleanup.process, "is_alive", lambda _pid: True)
    monkeypatch.setattr(cleanup.process, "kill_tree", lambda pid, command: killed.append((pid, command)) or True)
    ctx.store.update_state(lambda state: state.update(active_stacks=[{"pid": 123, "command": "app"}]))

    result = cleanup.run(ctx)

    assert result.kind == "next"
    assert killed == [(123, "app")]
    assert not path.exists()
    assert not ports.owners().get("repo/demo/0")
    assert not cleanup._branch_exists(repo, "feature/demo-t1")
    assert ctx.store.root.is_dir()
    assert ctx.store.read_state()["status"] == "cleaned"


def test_cleanup_is_idempotent(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "status": "cleaned", "pr": {"url": ""}})
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")
    monkeypatch.setattr(cleanup.workspaces, "remove", lambda *_args: (_ for _ in ()).throw(AssertionError()))

    result = cleanup.run(ctx)

    assert result.kind == "next"


def _stop_marker_command(marker):
    code = (f"import os,pathlib; pathlib.Path({str(marker)!r}).write_text("
            "os.getenv('SAKURA_FLOW_PORT','')+','+os.getenv('SAKURA_FLOW_STACK',''))")
    return f"{sys.executable} -c {shlex.quote(code)}"


def _registered_daemon(ctx, port, workspace=None):
    workspace = workspace or ctx.repo / "demo-worktree"
    workspace.mkdir(parents=True, exist_ok=True)
    ctx.store.update_state(lambda state: state.update(
        active_stacks=[{"pid": 123, "command": "daemon launcher"}],
        stacks={"demo:0": {"pid": 123, "command": "daemon launcher", "workspace": str(workspace),
                            "log": str(ctx.store.operations_dir / "app-0.log"), "last_used": 1, "port": port}},
        workspaces={"0": {"path": str(workspace), "branch": "feature/demo"}},
    ))


def _task_workspace(repo, overrides):
    """A context whose workspace 0 is a worktree of the task's own repository, as the engine makes it."""
    base = git(repo, "rev-parse", "HEAD").strip()
    ctx = make_context(repo, overrides=overrides, state={
        "phase": "cleanup", "pr": {"url": "https://example/pull/1"}, "branch": "feature/demo", "base_commit": base,
        "workspaces": {},
    })
    return ctx, workspaces.create(ctx, "0", "feature/demo", base)


def test_cleanup_stops_daemon_with_recorded_stack_environment(repo, tmp_path, monkeypatch):
    # `app.stop` runs in the cage, which writes only in the workspace (removed by cleanup), the task repository's Git
    # folder (kept here so the marker can be read) and its tmp.
    marker = tmp_path / "trees" / "demo.git" / "stop-environment"
    ctx, workspace = _task_workspace(repo, {"app": {"stop": _stop_marker_command(marker)},
                                            "workspace": {"root": str(tmp_path / "trees")}})
    assert task_repository.path(ctx.store.read_state()) == marker.parent
    owner = f"{repo.name}/demo/0"
    port = workspace.ports[0]
    _registered_daemon(ctx, port, workspace.path)
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")
    monkeypatch.setattr(cleanup.process, "is_alive", lambda _pid: False)
    monkeypatch.setattr(cleanup.task_repository, "remove", lambda _store: None)

    result = cleanup.run(ctx)

    assert result.kind == "next", result.reason
    assert marker.read_text() == f"{port},demo:0"
    assert owner not in ports.owners()


def test_cleanup_keeps_daemon_registration_and_port_when_stop_fails(repo, tmp_path, monkeypatch):
    stop = f"{sys.executable} -c {shlex.quote('raise SystemExit(7)')}"
    ctx, workspace = _task_workspace(repo, {"app": {"stop": stop}, "workspace": {"root": str(tmp_path / "trees")}})
    owner = f"{repo.name}/demo/0"
    port = workspace.ports[0]
    _registered_daemon(ctx, port, workspace.path)
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")
    monkeypatch.setattr(cleanup.process, "is_alive", lambda _pid: False)
    removed = []
    monkeypatch.setattr(cleanup.workspaces, "remove", lambda *_args: removed.append(True))

    result = cleanup.run(ctx)

    assert result.kind == "blocked"
    assert "stop" in result.reason.lower()
    assert removed == []
    assert ctx.store.read_state()["stacks"]["demo:0"]["port"] == port
    assert owner in ports.owners()


def test_resume_runs_cleanup_after_report_ready(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "status": "report_ready"})
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")

    assert engine.advance(ctx) == exits.OK
    assert ctx.store.read_state()["status"] == "cleaned"


# -- hook marks are pruned with the task ------------------------------------------------------------


def _leave_marks(task_store, *runs):
    for run in runs:
        mark = task_store.hook_mark(run)
        mark.parent.mkdir(parents=True, exist_ok=True)
        mark.touch()


def test_cleanup_removes_the_hook_marks_of_the_task_and_keeps_other_tasks(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": "https://example/pull/1"}})
    other = make_context(repo, task="other").store
    _leave_marks(ctx.store, "build-1", "evaluate-1")
    _leave_marks(other, "build-1")
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")

    result = cleanup.run(ctx)

    assert result.kind == "next"
    assert not any(ctx.store.hook_mark(run).exists() for run in ("build-1", "evaluate-1"))
    assert other.hook_mark("build-1").exists()


def test_cleanup_keeps_the_hook_marks_while_the_pull_request_is_open(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": "https://example/pull/1"}})
    _leave_marks(ctx.store, "build-1")
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "OPEN")

    assert cleanup.run(ctx).kind == "blocked"
    assert ctx.store.hook_mark("build-1").exists()


def test_cleanup_of_a_task_without_marks_succeeds(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": "https://example/pull/1"}})
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")

    assert cleanup.run(ctx).kind == "next"


def test_Cleanup_RemovesTheTasksOwnRepositoryWithItsWorkspaces(repo, tmp_path, monkeypatch):
    ctx, workspace = _task_workspace(repo, {"workspace": {"root": str(tmp_path / "trees")}})
    folder = task_repository.path(ctx.store.read_state())
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")

    assert cleanup.run(ctx).kind == "next"

    assert not folder.exists() and not workspace.path.exists()
    assert task_repository.STATE_KEY not in ctx.store.read_state()
    assert git(repo, "rev-parse", "HEAD").strip()


def test_cleanup_removes_a_role_repository_a_crash_left(repo, tmp_path, monkeypatch):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": "https://example/pull/1"}},
                       overrides={"workspace": {"root": str(tmp_path / "trees" / "{repo}")}})
    folder, worktree = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, "report")
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")

    assert cleanup.run(ctx).kind == "next"

    assert not folder.exists() and not worktree.exists()
    assert not ctx.store.read_state().get(task_repository.ROLE_REPOSITORIES_KEY)
