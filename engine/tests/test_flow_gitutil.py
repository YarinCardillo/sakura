"""Shared git helper: failures and timeouts become GitError."""

import os
import shlex
import subprocess

from pathlib import Path

import pytest

from conftest import git
from sakura import git_program, repository
from sakura.flow import gitutil


def test_git_failure_raises_with_stderr(repo):
    with pytest.raises(gitutil.GitError, match="rev-parse"):
        gitutil.git(repo, "rev-parse", "--verify", "nope")


def test_git_timeout_raises_clear_error(repo, monkeypatch):
    def slow(*_args, **_kwargs):
        raise subprocess.TimeoutExpired("git", 1)

    monkeypatch.setattr(subprocess, "run", slow)
    with pytest.raises(gitutil.GitError, match="timed out"):
        gitutil.git(repo, "status")


def test_tail_returns_last_lines_and_tolerates_missing_file(tmp_path):
    log = tmp_path / "x.log"
    assert gitutil.tail(log) == ""
    log.write_text("\n".join(str(n) for n in range(100)))
    assert gitutil.tail(log).splitlines()[-1] == "99"
    assert len(gitutil.tail(log).splitlines()) == gitutil.LOG_TAIL_LINES


def test_branch_exists_distinguishes_absent_from_timeout(repo, monkeypatch):
    assert gitutil.branch_exists(repo, "main") is True
    assert gitutil.branch_exists(repo, "nope") is False

    def slow(*_args, **_kwargs):
        raise subprocess.TimeoutExpired("git", 1)

    monkeypatch.setattr(subprocess, "run", slow)
    with pytest.raises(gitutil.GitError, match="timed out"):
        gitutil.branch_exists(repo, "main")


# -- review 4: one pinned Git, never one a repository controls -------------------------------------------------


def _planted_git(folder, marker):
    folder.mkdir(parents=True, exist_ok=True)
    program = folder / "git"
    program.write_text(f"#!/bin/sh\ntouch {marker}\nexec /usr/bin/git \"$@\"\n")
    program.chmod(0o755)
    return program


@pytest.fixture
def unpinned(monkeypatch):
    """Each test resolves Git afresh from its own PATH."""
    monkeypatch.setattr(git_program, "_resolved", {})


def test_PinnedGit_APlantedGitInAWorkspaceVenvEarlierOnPath_IsRefusedAndNeverRuns(repo, tmp_path, monkeypatch,
                                                                                   unpinned):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    marker = tmp_path / "planted-ran"
    venv = _planted_git(lane / ".venv" / "bin", marker).parent
    monkeypatch.setenv("PATH", f"{venv}{os.pathsep}{os.environ['PATH']}")
    for call in (lambda: gitutil.trust_repository(repo), lambda: gitutil.git(repo, "status"),
                 lambda: gitutil.git(lane, "status"), lambda: repository.common_dir(repo)):
        with pytest.raises((gitutil.GitError, repository.RepositoryError), match="repository controls"):
            call()
    assert not marker.exists()


def test_PinnedGit_ARelativePathEntry_IsRefused(repo, tmp_path, monkeypatch, unpinned):
    marker = tmp_path / "planted-ran"
    _planted_git(tmp_path / "tools", marker)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PATH", f"tools{os.pathsep}{os.environ['PATH']}")
    with pytest.raises(git_program.GitProgramError, match="relative"):
        git_program.path()
    assert not marker.exists()


def test_PinnedGit_ResolvedOnce_IsAbsoluteAndUsedForEveryEngineCall(repo, tmp_path, monkeypatch, unpinned):
    pinned = git_program.path()
    assert os.path.isabs(pinned) and os.access(pinned, os.X_OK)
    marker = tmp_path / "later"
    monkeypatch.setenv("PATH", f"{_planted_git(tmp_path / 'later-bin', marker).parent}{os.pathsep}{os.environ['PATH']}")
    assert git_program.path() == pinned, "resolved once: a later PATH changes nothing"
    assert gitutil.command(repo, "status")[0][0] == pinned
    gitutil.git(repo, "status")
    assert not marker.exists()


def _planted(folder, name, marker):
    folder.mkdir(parents=True, exist_ok=True)
    program = folder / name
    program.write_text(f"#!/bin/sh\ntouch {marker}\nexit 1\n")
    program.chmod(0o755)
    return program


def test_PinnedGit_EngineGitCalls_RunWithAFixedSafePathAndThePinnedSsh(repo, unpinned):
    _, env = gitutil.command(repo, "status")
    folders = [os.path.dirname(git_program.path()), "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
    assert env["PATH"] == os.pathsep.join(dict.fromkeys(folders))
    assert env["GIT_SSH_COMMAND"] == shlex.quote(git_program.path("ssh")) and os.path.isabs(git_program.path("ssh"))
    for call_env in (gitutil.program_environment(repo), git_program.environment(repo, {"PATH": "/elsewhere"})):
        assert call_env["PATH"] == env["PATH"] and call_env["GIT_SSH_COMMAND"] == env["GIT_SSH_COMMAND"]


def test_PinnedGit_APlantedSshInAWorkspaceVenvEarlierOnPath_IsRefusedAndNeverRuns(repo, tmp_path, monkeypatch,
                                                                                   unpinned):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    marker = tmp_path / "planted-ssh-ran"
    venv = _planted(lane / ".venv" / "bin", "ssh", marker).parent
    git_program.path()  # the host git, resolved before the venv joins PATH
    monkeypatch.setenv("PATH", f"{venv}{os.pathsep}{os.environ['PATH']}")
    with pytest.raises(gitutil.GitError, match="ssh on PATH .* repository controls"):
        gitutil.git(repo, "ls-remote", "ssh://git@127.0.0.1:1/nothing.git")
    assert not marker.exists()


def test_PinnedGit_AGitChildSearchingPath_NeverSeesTheInheritedPath(repo, tmp_path, monkeypatch, unpinned):
    marker = tmp_path / "helper-ran"
    tools = _planted(tmp_path / "tools", "git-credential-planted", marker).parent
    monkeypatch.setenv("PATH", f"{os.environ['PATH']}{os.pathsep}{tools}")
    argv, env = gitutil.command(repo, "credential", "fill")
    done = subprocess.run([*argv[:1], "-c", "credential.helper=planted", *argv[1:]], env=env, capture_output=True,
                          text=True, input="protocol=https\nhost=example.invalid\n\n", timeout=30)
    assert not marker.exists(), done.stderr


@pytest.mark.skipif(not Path("/PRIVATE").exists(), reason="needs a case-insensitive volume")
def test_PinnedGit_ACaseVariantPathIntoTheWorkspace_IsStillRefused(repo, tmp_path, monkeypatch, unpinned):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    marker = tmp_path / "planted-ran"
    _planted_git(lane / ".venv" / "bin", marker)
    variant = tmp_path / "LANE" / ".venv" / "bin"
    assert variant.is_dir() and str(variant) != str(lane / ".venv" / "bin")
    monkeypatch.setenv("PATH", f"{variant}{os.pathsep}{os.environ['PATH']}")
    with pytest.raises(gitutil.GitError, match="repository controls"):
        gitutil.git(repo, "status")
    assert not marker.exists()


@pytest.mark.parametrize("planted", ["fifo", "link"])
def test_PinnedGit_AnUnreadableGitMarker_FailsClosed(tmp_path, unpinned, planted):
    folder = tmp_path / "work"
    folder.mkdir()
    if planted == "fifo":
        os.mkfifo(folder / ".git")
    else:
        (tmp_path / "elsewhere").write_text("gitdir: /nowhere\n")
        (folder / ".git").symlink_to(tmp_path / "elsewhere")
    with pytest.raises(git_program.GitProgramError, match="cannot be read"):
        git_program.for_repository(folder)


def test_PinnedGit_AnUnreadableWorktreePointer_FailsClosed(repo, tmp_path, unpinned):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    pointer = repo / ".git" / "worktrees" / "lane" / "gitdir"
    target = tmp_path / "pointer-target"
    target.write_text(pointer.read_text())
    pointer.unlink()
    pointer.symlink_to(target)
    with pytest.raises(git_program.GitProgramError, match="cannot be read"):
        git_program.for_repository(repo)


def test_EngineGit_TheObjectDirectoryVariablesOfTheEnvironment_NeverReachGit(repo, monkeypatch, tmp_path):
    monkeypatch.setenv("GIT_ALTERNATE_OBJECT_DIRECTORIES", str(tmp_path / "elsewhere" / "objects"))
    monkeypatch.setenv("GIT_OBJECT_DIRECTORY", str(tmp_path / "elsewhere" / "objects"))
    for env in (gitutil.command(repo, "status")[1], gitutil.program_environment(repo),
                git_program.environment(repo, dict(os.environ))):
        assert "GIT_ALTERNATE_OBJECT_DIRECTORIES" not in env and "GIT_OBJECT_DIRECTORY" not in env
    assert gitutil.git(repo, "rev-parse", "HEAD").strip()
