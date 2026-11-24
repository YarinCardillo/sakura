from __future__ import annotations

import pytest

from conftest import git

from sakura import profile


def test_load_merges_layers_and_adds_integration_branch(repo, tmp_path):
    home = tmp_path / "home"
    personal = home / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('require_signed_commits = true\n')
    shared = repo / ".sakura/profile.toml"
    shared.parent.mkdir()
    shared.write_text('[tests]\nfast = ["true"]\n')
    result = profile.load(repo, home)
    assert result.values["require_signed_commits"] is True
    assert result.values["tests"]["fast"] == ["true"]
    assert "main" in result.values["protected_branches"]
    assert result.origins["tests.fast"] == "shared"


def test_load_rejects_relaxation_without_explicit_key(repo):
    target = repo / ".sakura/profile.toml"
    target.parent.mkdir()
    target.write_text("require_draft_pr = false\n")
    with pytest.raises(profile.ProfileError, match="relaxes"):
        profile.load(repo)


def test_load_reports_explicit_relaxation_of_a_host_layer(repo, tmp_path):
    personal = tmp_path / "home" / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('relax = ["require_draft_pr"]\nrequire_draft_pr = false\n')
    result = profile.load(repo, tmp_path / "home")
    assert result.relaxations == ["require_draft_pr (defaults to personal)"]


@pytest.mark.parametrize("name, text", [
    ("profile.toml", 'relax = ["require_draft_pr"]\nrequire_draft_pr = false\n'),
    ("local.toml", 'relax = ["require_draft_pr"]\nrequire_draft_pr = false\n'),
    ("profile.toml", "relax = []\n")])
def test_RepositoryLayers_CannotCarryRelaxAtAll(repo, tmp_path, name, text):
    target = repo / ".sakura" / name
    target.parent.mkdir()
    target.write_text(text)
    with pytest.raises(profile.ProfileError, match="cannot carry relax"):
        profile.load(repo, tmp_path / "home")


def _shared_profile(repo, text):
    target = repo / ".sakura/profile.toml"
    target.parent.mkdir(exist_ok=True)
    target.write_text(text)


def test_IntegrationBranch_FromARepositoryLayer_IsNeutral(repo, tmp_path):
    _shared_profile(repo, 'integration_branch = "develop"\n')
    result = profile.load(repo, tmp_path / "home")
    assert result.values["integration_branch"] == "develop" and result.relaxations == []
    assert "develop" in result.values["protected_branches"]


def test_BranchPattern_FromARepositoryLayer_IsAllowedWhenItMatchesNoProtectedName(repo, tmp_path):
    _shared_profile(repo, 'branch_pattern = "^(feat|fix)/[a-z0-9-]+$"\n')
    assert profile.load(repo, tmp_path / "home").values["branch_pattern"] == "^(feat|fix)/[a-z0-9-]+$"


@pytest.mark.parametrize("pattern", ["^.*$", "^(main|feat/.*)$", "^(master)$", "^[a-z]+$"])
def test_BranchPattern_FromARepositoryLayer_ThatMatchesAProtectedName_IsRefused(repo, tmp_path, pattern):
    _shared_profile(repo, f"branch_pattern = '{pattern}'\n")
    with pytest.raises(profile.ProfileError, match="protected branch"):
        profile.load(repo, tmp_path / "home")


def _personal_profile(tmp_path, text):
    personal = tmp_path / "home" / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True, exist_ok=True)
    personal.write_text(text)


def test_BranchPattern_FromARepositoryLayer_MayNotMatchTheHostIntegrationBranch(repo, tmp_path):
    _personal_profile(tmp_path, 'integration_branch = "develop"\n')
    _shared_profile(repo, 'branch_pattern = "^(develop|feat/.*)$"\n')
    with pytest.raises(profile.ProfileError, match="'develop'"):
        profile.load(repo, tmp_path / "home")


def test_BranchPattern_FromARepositoryLayer_MayNotMatchAHostProtectedBranch(repo, tmp_path):
    _personal_profile(tmp_path, 'protected_branches = ["main", "master", "release"]\n')
    _shared_profile(repo, 'branch_pattern = "^(release|feat/.*)$"\n')
    with pytest.raises(profile.ProfileError, match="'release'"):
        profile.load(repo, tmp_path / "home")


def test_BranchPattern_FromARepositoryLayer_MayNotMatchTheRemoteDefaultBranch(repo, tmp_path):
    git(repo, "update-ref", "refs/remotes/origin/trunk", "HEAD")
    git(repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
    _shared_profile(repo, 'branch_pattern = "^(trunk|feat/.*)$"\n')
    with pytest.raises(profile.ProfileError, match="'trunk'"):
        profile.load(repo, tmp_path / "home")


def test_BranchPattern_MatchingOnlyTheIntegrationBranchTheSameLayerChose_IsNotWhatIsChecked(repo, tmp_path):
    _shared_profile(repo, 'integration_branch = "weird"\nbranch_pattern = "^(weird|feat/.*)$"\n')
    assert profile.load(repo, tmp_path / "home").values["integration_branch"] == "weird"


def test_BranchPattern_FromAHostLayer_IsNotChecked(repo, tmp_path):
    personal = tmp_path / "home" / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('branch_pattern = "^.*$"\n')
    assert profile.load(repo, tmp_path / "home").values["branch_pattern"] == "^.*$"
