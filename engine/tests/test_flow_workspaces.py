"""Task worktrees: creation from the pinned base commit, reuse and removal."""

import subprocess
from pathlib import Path

from sakura import engine_home
from sakura.flow import workspaces

from conftest import git
from flow_support import make_context


def _context(repo, tmp_path, setup=None, task="demo"):
    base = git(repo, "rev-parse", "HEAD")
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": setup or []}}
    state = {"base_commit": base, "branch": f"feat/flow-{task}", "workspaces": {}}
    return make_context(repo, overrides=overrides, task=task, state=state), base


def test_task_workspace_ignores_dirty_owner_checkout(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    (repo / "README").write_text("dirty\n")
    (repo / "scratch.txt").write_text("untracked\n")
    git(repo, "commit", "-qam", "moved on")
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert workspace.path == tmp_path / "trees" / "repo" / "demo-0"
    assert (workspace.path / "README").read_text() == "initial\n"
    assert not (workspace.path / "scratch.txt").exists()
    assert git(workspace.path, "rev-parse", "HEAD") == base
    assert git(workspace.path, "rev-parse", "--abbrev-ref", "HEAD") == "feat/flow-demo"
    assert len(workspace.ports) == 10


def test_workspace_create_is_idempotent(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    first = workspaces.create(ctx, "0", "feat/flow-demo", base)
    (first.path / "kept.txt").write_text("work in progress\n")
    again = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert again == first
    assert (again.path / "kept.txt").exists()
    assert ctx.store.read_state()["workspaces"]["0"]["path"] == str(first.path)


def test_existing_workspace_port_change_is_recorded(repo, tmp_path, monkeypatch):
    from sakura.flow import ports

    monkeypatch.setattr(ports, "PORT_BASE", 47000)
    ctx, base = _context(repo, tmp_path)
    first = workspaces.create(ctx, "0", "feat/flow-demo", base)
    ctx.config.values["app"]["origin"] = "http://localhost:48000"
    moved = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert moved.ports.start == 48000
    assert ctx.store.read_state()["workspaces"]["0"]["ports"] == [48000, 48009]


def test_workspace_create_reuses_existing_branch_without_worktree(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    git(repo, "branch", "feat/flow-demo", base)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert git(workspace.path, "rev-parse", "--abbrev-ref", "HEAD") == "feat/flow-demo"


def test_workspace_installs_hooks_and_runs_setup(repo, tmp_path):
    ctx, base = _context(repo, tmp_path, setup=["echo ready > setup.out"])
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert (workspace.path / ".sakura/git-hooks/pre-commit").is_symlink()
    assert (workspace.path / "setup.out").read_text().strip() == "ready"
    operation_logs = ctx.store.runs_dir / "operations"
    assert (operation_logs / "workspace-0-install.log").is_file()
    assert (operation_logs / "workspace-0-setup-1.log").is_file()
    assert str(operation_logs / "workspace-0-setup-1.log") in ctx.store.ledger.read_text()


def test_failing_setup_command_blocks_with_its_log(repo, tmp_path):
    ctx, base = _context(repo, tmp_path, setup=["echo boom >&2; exit 3"])
    try:
        workspaces.create(ctx, "0", "feat/flow-demo", base)
    except workspaces.WorkspaceError as error:
        assert "exit 3" in str(error) and "boom" in str(error)
    else:
        raise AssertionError("setup failure must raise")


def test_remove_deletes_worktree_and_releases_ports(repo, tmp_path):
    from sakura.flow import ports

    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    workspaces.remove(ctx, "0")
    assert not workspace.path.exists()
    assert ports.owners() == {}
    assert "0" not in ctx.store.read_state()["workspaces"]
    # The branch stays, in the task's own repository: the person's repository never had it.
    task_repo = ctx.store.read_state()["task_repository"]["path"]
    assert git(Path(task_repo), "branch", "--list", "feat/flow-demo").strip() == "feat/flow-demo"
    assert git(repo, "branch", "--list", "feat/flow-demo") == ""


def test_default_workspace_root_is_next_to_the_engine_home_not_in_the_owners_folders(repo):
    ctx = make_context(repo)
    path = workspaces._path_for(ctx, "0")
    assert path == engine_home.root().with_name(engine_home.root().name + "-workspaces") / repo.name / "demo-0"
    assert "development" not in path.parts and "knowledge" not in path.parts


def test_workspace_create_never_writes_the_repositorys_git_config(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    config_before = (repo / ".git/config").read_bytes()
    exclude_before = (repo / ".git/info/exclude").read_bytes()

    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)

    assert (repo / ".git/config").read_bytes() == config_before
    assert (repo / ".git/info/exclude").read_bytes() == exclude_before
    assert not list((repo / ".git").glob("**/config.worktree"))
    assert (workspace.path / ".sakura/git-hooks/pre-commit").is_symlink()
