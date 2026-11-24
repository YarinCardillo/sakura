"""A lane's engine process reads the other registered projects without having been started on them."""

import pytest

from sakura import repository
from sakura.flow import gitutil, lane_ceiling, others, registry

from conftest import git
from test_cockpit_snapshot import create_task, other_repo


@pytest.fixture(params=["side_by_side", "different_parents"])
def second(request, repo, tmp_path):
    """A second repository, next to the first one or under a parent folder of its own."""
    parent = tmp_path if request.param == "side_by_side" else tmp_path / "elsewhere"
    parent.mkdir(exist_ok=True)
    return other_repo(parent)


def _lane_process_on(first, second_repo):
    """What `sakura flow run` on `first` knows: both projects registered, only `first` trusted."""
    registry.remember(first)
    registry.remember(second_repo)
    gitutil._trusted_repositories[:] = [first]
    repository._common_dirs.clear()  # a new process has resolved no folder yet


def test_Others_SecondRegisteredProject_ItsOpenLaneIsListed(repo, second):
    create_task(repo, "alpha")
    create_task(second, "beta")
    _lane_process_on(repo, second)
    text = others.summary(registry.repos(), exclude=lane_ceiling.lane_name(repo, "alpha"))
    assert f"{second.name}/beta" in text


def test_Others_SecondRegisteredProjectWithoutLanes_NothingIsListed(repo, second):
    create_task(repo, "alpha")
    _lane_process_on(repo, second)
    text = others.summary(registry.repos(), exclude=lane_ceiling.lane_name(repo, "alpha"))
    assert others.NO_LANES in text


def test_LaneCeiling_SecondRegisteredProject_ItsLaneCounts(repo, second):
    create_task(repo, "alpha")
    create_task(second, "beta")
    _lane_process_on(repo, second)
    assert sorted(lane_ceiling.open_lanes(registry.repos())) == sorted(
        [f"{repo.name}/alpha", f"{second.name}/beta"])


def test_GitHelper_FolderOfAnUnregisteredProject_IsStillRefused(repo, second):
    _lane_process_on(repo, second)
    with pytest.raises(gitutil.GitError, match="inside the workspace"):
        gitutil.git(second, "rev-parse", "HEAD")
    assert git(second, "rev-parse", "HEAD")
