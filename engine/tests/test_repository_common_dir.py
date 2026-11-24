"""`repository.common_dir` asks Git once per folder and process, and again when the folder's `.git` is replaced."""

import shutil

import pytest

from sakura import repository
from sakura.flow import gitutil

from test_cockpit_snapshot import collect, create_task, other_repo
from conftest import git


@pytest.fixture
def git_calls(monkeypatch):
    """The `--git-common-dir` calls that reach Git."""
    calls = []
    real = gitutil.run

    def spy(path, *args, **kwargs):
        if "--git-common-dir" in args:
            calls.append(path)
        return real(path, *args, **kwargs)

    monkeypatch.setattr(gitutil, "run", spy)
    return calls


def test_CommonDir_SameFolderTwice_AsksGitOnce(repo, git_calls):
    first, second = repository.common_dir(repo), repository.common_dir(repo)

    assert first == second == repo.resolve() / ".git"
    assert len(git_calls) == 1


def test_CommonDir_FolderThroughALink_SharesTheResolvedPathsAnswer(repo, tmp_path, git_calls):
    link = tmp_path / "link"
    link.symlink_to(repo)

    assert repository.common_dir(repo) == repository.common_dir(link)
    assert len(git_calls) == 1


def test_CommonDir_GitFolderReplaced_AsksAgain(repo, git_calls):
    repository.common_dir(repo)
    shutil.rmtree(repo / ".git")
    git(repo.parent, "init", "-q", "-b", "main", str(repo))

    assert repository.common_dir(repo) == repo.resolve() / ".git"
    assert len(git_calls) == 2


def test_CommonDir_LinkedWorktree_IsItsOwnKey(repo, tmp_path, git_calls):
    linked = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "side", str(linked))

    assert repository.common_dir(linked) == repository.common_dir(repo)
    assert len(git_calls) == 2


def test_CommonDir_FolderWithNoRepository_RaisesEveryTime(tmp_path):
    plain = tmp_path / "plain"
    plain.mkdir()
    for _ in range(2):
        with pytest.raises(repository.RepositoryError):
            repository.common_dir(plain)


def test_Snapshot_TwoRepositoriesFourTasks_AsksGitOncePerRepository(repo, tmp_path, git_calls):
    second = other_repo(tmp_path)
    for folder in (repo, second):
        create_task(folder, "one")
        create_task(folder, "two")
    git_calls.clear()

    collect([repo, second])

    assert len(git_calls) <= 2
