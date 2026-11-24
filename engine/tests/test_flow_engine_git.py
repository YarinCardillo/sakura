"""The engine's own Git calls in a workspace an agent can write: no hook, no signing program, a verified Git folder."""

import os
import stat
import subprocess
from pathlib import Path

import pytest

from conftest import git
from flow_support import make_context
from sakura.flow import gitutil, workspaces
from sakura.flow.phases import contract as contract_phase


def _workspace(repo, tmp_path):
    base = git(repo, "rev-parse", "HEAD")
    ctx = make_context(repo, overrides={"workspace": {"root": str(tmp_path / "trees" / "{repo}")}},
                       state={"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}})
    return ctx, workspaces.create(ctx, "0", "feat/flow-demo", base).path


def _script(path: Path, marker: Path) -> Path:
    path.write_text(f"#!/bin/sh\ntouch {marker}\nexit 0\n")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def _hooks_folder(repo, tmp_path) -> Path:
    """A hooks folder of this test's own that the repository's config names: never a shared folder."""
    hooks = tmp_path / "planted-hooks"
    hooks.mkdir()
    git(repo, "config", "core.hooksPath", str(hooks))
    return hooks


def _stage_file(path: Path) -> None:
    (path / "new.txt").write_text("x\n")
    gitutil.git(path, "add", "new.txt")


def test_EngineCommit_PlantedHookInTheGitFolder_IsNotRun(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "hook-ran"
    hooks = _hooks_folder(repo, tmp_path)
    _script(hooks / "pre-commit", marker)
    _script(hooks / "post-commit", marker)
    _stage_file(path)
    gitutil.git(path, "commit", "-q", "-m", "engine commit")
    assert not marker.exists()
    assert gitutil.git(path, "log", "-1", "--format=%s").strip() == "engine commit"


def test_EngineCommit_HooksPathAndSigningProgramInConfig_AreNotRun(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "ran"
    evil_hooks = tmp_path / "evil-hooks"
    evil_hooks.mkdir()
    _script(evil_hooks / "pre-commit", marker)
    signer = _script(tmp_path / "signer", marker)
    git(repo, "config", "core.hooksPath", str(evil_hooks))
    git(repo, "config", "gpg.program", str(signer))
    git(repo, "config", "commit.gpgsign", "true")
    _stage_file(path)
    gitutil.git(path, "commit", "-q", "-m", "engine commit")
    assert not marker.exists()


def test_EngineGit_FsmonitorInConfig_IsNotRun(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "ran"
    git(repo, "config", "core.fsmonitor", str(_script(tmp_path / "monitor", marker)))
    gitutil.git(path, "status", "--porcelain")
    assert not marker.exists()


def test_EngineGit_GlobalAndSystemConfig_AreIgnoredButTheHostIdentityStillCommits(repo, tmp_path, monkeypatch):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "ran"
    (Path(os.environ["HOME"]) / ".gitconfig").write_text(f"[core]\n\thooksPath = {tmp_path}\n[gpg]\n\tprogram = {_script(tmp_path / 's', marker)}\n")
    _stage_file(path)
    gitutil.git(path, "commit", "-q", "-m", "engine commit")
    assert gitutil.git(path, "log", "-1", "--format=%an").strip() == "Test User"
    assert not marker.exists()


def test_EngineGit_TamperedGitFile_PointingAtAnAgentGitFolder_IsRefused(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "hook-ran"
    evil = tmp_path / "evil-clone"
    subprocess.run(["git", "clone", "-q", str(repo), str(evil)], check=True, capture_output=True)
    _script(evil / ".git" / "hooks" / "pre-commit", marker)
    (path / ".git").write_text(f"gitdir: {evil / '.git'}\n")
    with pytest.raises(gitutil.GitError, match="not .*registered|does not point"):
        gitutil.git(path, "commit", "--allow-empty", "-q", "-m", "x")
    assert not marker.exists()


def test_EngineGit_GitFileOfAnotherWorktreeEntry_IsRefusedWhenTheBackpointerDiffers(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    other_base = git(repo, "rev-parse", "HEAD")
    git(repo, "worktree", "add", "-q", "--detach", str(tmp_path / "other"), other_base)
    foreign = next(p for p in (repo / ".git" / "worktrees").iterdir() if (p / "gitdir").read_text().strip().startswith(str(tmp_path / "other")))
    (path / ".git").write_text(f"gitdir: {foreign}\n")
    with pytest.raises(gitutil.GitError):
        gitutil.git(path, "status")


def test_EngineGit_GitFolderReplacedByADirectory_IsRefused(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "hook-ran"
    (path / ".git").unlink()
    subprocess.run(["git", "init", "-q", str(path)], check=True, capture_output=True)
    _script(path / ".git" / "hooks" / "pre-commit", marker)
    with pytest.raises(gitutil.GitError):
        gitutil.git(path, "commit", "--allow-empty", "-q", "-m", "x")
    assert not marker.exists()


def test_EngineGit_SymlinkedGitFile_IsRefused(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    real = path / ".git"
    moved = tmp_path / "moved-git-file"
    real.rename(moved)
    real.symlink_to(moved)
    with pytest.raises(gitutil.GitError):
        gitutil.git(path, "status")


def test_EngineGit_AMainRepositoryAndItsWorktreesWork(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    assert gitutil.git(repo, "rev-parse", "--abbrev-ref", "HEAD").strip() == "main"
    assert gitutil.git(path, "rev-parse", "--abbrev-ref", "HEAD").strip() == "feat/flow-demo"


def test_ContractPhaseCommit_PlantedHook_IsNotRun(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    marker = tmp_path / "hook-ran"
    hooks = _hooks_folder(repo, tmp_path)
    _script(hooks / "pre-commit", marker)
    (path / "e2e").mkdir()
    (path / "e2e" / "flow.py").write_text("# flow\n")
    base = ctx.store.read_state()["base_commit"]
    gitutil.git(path, "add", "e2e/flow.py")
    contract_phase.check_staged(path, base, ["e2e/flow.py"])
    gitutil.git(path, "commit", "-q", "-m", "test(flow): contract flows for demo")
    assert not marker.exists()


def test_EngineGit_WorkspaceThatIsAFullClone_IsRefusedBecauseItsGitFolderIsInsideIt(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    clone = tmp_path / "clone-workspace"
    subprocess.run(["git", "clone", "-q", str(repo), str(clone)], check=True, capture_output=True)
    with pytest.raises(gitutil.GitError, match="inside the workspace"):
        gitutil.git(clone, "status")
    gitutil.trust_repository(clone)  # even registered, the main checkout is the only place a .git folder may sit
    gitutil.git(clone, "status")


def test_EngineGit_GitFileNamingAFolderInsideTheWorkspace_IsRefused(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    inner = path / "inner-git"
    inner.mkdir()
    (path / ".git").write_text(f"gitdir: {inner}\n")
    with pytest.raises(gitutil.GitError, match="inside the workspace"):
        gitutil.git(path, "status")


def test_EngineGit_AttributesAreNotReadFromTheSystemOrAGlobalFile(repo, tmp_path):
    ctx, path = _workspace(repo, tmp_path)
    argv, env = gitutil.command(path, "diff")
    assert env["GIT_ATTR_NOSYSTEM"] == "1" and "core.attributesFile=/dev/null" in argv
