"""The snapshot reads every registered repository in one process, in any order; a bad entry is one problem line."""

import itertools

import pytest

from sakura import engine_home
from sakura.flow import gitutil

from test_cockpit_snapshot import collect, create_task, other_repo


@pytest.fixture
def fresh_process(monkeypatch):
    """What `sakura cockpit snapshot` sees: tasks exist on disk, no repository is trusted yet."""
    return lambda: monkeypatch.setattr(gitutil, "_trusted_repositories", [])


def flow_keys(found):
    return sorted(item.key for item in found.items if item.kind == "flow")


def test_Snapshot_TwoRepositories_BothOrdersReadBoth(repo, tmp_path, fresh_process):
    second = other_repo(tmp_path)
    create_task(repo, "alpha")
    create_task(second, "beta")
    wanted = sorted([("flow", str(repo), "alpha"), ("flow", str(second), "beta")])
    for order in ([repo, second], [second, repo]):
        fresh_process()
        found = collect(order)
        assert flow_keys(found) == wanted
        assert found.problems == []


def test_Snapshot_ThreeRepositories_EveryOrderReadsAll(repo, tmp_path, fresh_process):
    others = [other_repo(tmp_path, "second"), other_repo(tmp_path, "third")]
    repos = [repo, *others]
    for number, folder in enumerate(repos):
        create_task(folder, f"task-{number}")
    wanted = sorted(("flow", str(folder), f"task-{number}") for number, folder in enumerate(repos))
    for order in itertools.permutations(repos):
        fresh_process()
        found = collect(list(order))
        assert flow_keys(found) == wanted
        assert found.problems == []


def test_Snapshot_RepositoryWithBrokenRecord_OneProblemLineRestStands(repo, tmp_path, fresh_process):
    broken = other_repo(tmp_path, "broken")
    create_task(repo, "alpha")
    (broken / ".git" / engine_home.TASK_REPOSITORY_RECORD).write_text("relative/path\n")
    for order in ([repo, broken], [broken, repo]):
        fresh_process()
        found = collect(order)
        assert flow_keys(found) == [("flow", str(repo), "alpha")]
        assert [line for line in found.problems if str(broken) in line] != []
        assert len([line for line in found.problems if str(broken) in line]) == 1
        assert not [line for line in found.problems if str(repo) in line]


def test_Snapshot_RegisteredFolderGitCannotOpen_OneProblemLineRestStands(repo, tmp_path, fresh_process):
    not_a_repository = tmp_path / "plain"
    not_a_repository.mkdir()
    create_task(repo, "alpha")
    fresh_process()

    found = collect([not_a_repository, repo])

    assert flow_keys(found) == [("flow", str(repo), "alpha")]
    assert len([line for line in found.problems if str(not_a_repository) in line]) == 1


def test_Snapshot_CollectedTwiceInOneProcess_StillReadsEveryRepository(repo, tmp_path, fresh_process):
    second = other_repo(tmp_path)
    create_task(repo, "alpha")
    create_task(second, "beta")
    fresh_process()

    first, again = collect([repo, second]), collect([repo, second])

    assert flow_keys(first) == flow_keys(again) and len(flow_keys(again)) == 2
    assert again.problems == []


def test_snapshot_reads_each_git_ref_once_and_refreshes_moved_heads(repo, tmp_path, monkeypatch):
    from sakura.cockpit import items
    from test_lanes import git

    linked = tmp_path / "linked"
    git(repo, "worktree", "add", "-qb", "linked", str(linked))
    git(linked, "commit", "-q", "--allow-empty", "-m", "linked head")
    git(repo, "pack-refs", "--all")
    for name in ("first", "second"):
        create_task(repo, name, branch="main")
        create_task(repo, f"{name}-linked", branch="main", workspaces={"0": {"path": str(linked)}})
    requests = []
    original = items.gitutil.git

    def record(path, *args, **kwargs):
        if args[:2] == ("log", "-1"):
            requests.append((path, args[-2]))
        return original(path, *args, **kwargs)

    monkeypatch.setattr(items.gitutil, "git", record)
    first = collect([repo])
    assert requests == [(repo, "refs/heads/main"), (linked, "HEAD")]
    assert [item.last_commit.split(" ", 1)[1] for item in first.items] == [
        "initial", "linked head", "initial", "linked head",
    ]

    requests.clear()
    git(repo, "commit", "-q", "--allow-empty", "-m", "new main")
    git(linked, "commit", "-q", "--allow-empty", "-m", "new linked head")
    again = collect([repo])
    assert requests == [(repo, "refs/heads/main"), (linked, "HEAD")]
    assert [item.last_commit.split(" ", 1)[1] for item in again.items] == [
        "new main", "new linked head", "new main", "new linked head",
    ]
