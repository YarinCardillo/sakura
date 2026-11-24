"""The guard that fails a test which writes under the real account root, and the trust root the tests use instead."""

import pytest

from conftest import _real_account_entries, watching_the_real_account_root
from sakura import engine_home


@pytest.fixture
def fake_account_root(tmp_path):
    root = tmp_path / "account" / "engine"
    (root / "repo-old" / "kit").mkdir(parents=True)
    return root


def test_Guard_Entries_NamesTheFoldersAndOneLevelBelow(fake_account_root):
    assert _real_account_entries(fake_account_root) == {"repo-old", "repo-old/kit"}


def test_Guard_AnEntryAddedMeanwhile_FailsNamingIt(fake_account_root):
    with pytest.raises(pytest.fail.Exception, match="repo-abc12345"):
        with watching_the_real_account_root(fake_account_root):
            (fake_account_root / "repo-abc12345" / "kit").mkdir(parents=True)


def test_Guard_AFileAddedInsideAnExistingFolder_FailsToo(fake_account_root):
    with pytest.raises(pytest.fail.Exception, match="repo-old/stamps"):
        with watching_the_real_account_root(fake_account_root):
            (fake_account_root / "repo-old" / "stamps").mkdir()


def test_Guard_NothingAdded_Passes(fake_account_root):
    with watching_the_real_account_root(fake_account_root):
        pass


def test_TrustRoot_InATest_IsTheTestsOwnFolderNeverTheAccountRoot(repo, tmp_path):
    assert engine_home.root_for_trust() == tmp_path / "engine-home"
    assert engine_home.kit_dir(repo).parent.parent == tmp_path / "engine-home"


def test_Guard_WatchesTheInjectedRootEvenWhenATestPatchesAccountRoot(tmp_path, monkeypatch):
    expected = tmp_path / "default-account" / engine_home.DEFAULT_RELATIVE
    assert engine_home.account_root() == expected
    unexpected = expected / "unexpected"
    try:
        with pytest.raises(pytest.fail.Exception, match="unexpected"), watching_the_real_account_root(expected):
            monkeypatch.setattr(engine_home, "account_root", lambda: tmp_path)
            unexpected.mkdir(parents=True)
    finally:
        if unexpected.is_dir():
            unexpected.rmdir()


def test_GitFixtureIgnoresHostToolsAndConfiguration(tmp_path, monkeypatch):
    from conftest import git

    executable = tmp_path / "git"
    executable.write_text("#!/bin/sh\nexit 99\n")
    executable.chmod(0o700)
    config = tmp_path / "unusable-config"
    config.write_text("invalid configuration\n")
    monkeypatch.setenv("PATH", str(tmp_path))
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    monkeypatch.setenv("GIT_CONFIG_COUNT", "1")
    assert git(tmp_path, "--version").startswith("git version ")
