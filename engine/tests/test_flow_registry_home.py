"""The registry follows the engine home; the old file under the account home is adopted once and left alone."""

import json

import pytest

from sakura import engine_home
from sakura.flow import registry


@pytest.fixture
def legacy_file(tmp_path):
    return tmp_path / "isolated-home" / ".local" / "state" / "sakura" / "flow" / "repos.json"


def write_legacy(legacy_file, entries):
    legacy_file.parent.mkdir(parents=True)
    legacy_file.write_text(json.dumps({"repos": [str(entry) for entry in entries]}))


def make_repos(tmp_path, *names):
    paths = [tmp_path / name for name in names]
    for path in paths:
        path.mkdir()
    return paths


@pytest.fixture
def default_engine_home(monkeypatch):
    """No SAKURA_ENGINE_HOME: the engine home is the default one (the conftest points it at a temporary folder)."""
    monkeypatch.delenv(engine_home.ENV_VARIABLE)
    return engine_home.root()


def test_DefaultPath_EngineHomeNamed_IsReposJsonInIt(tmp_path):
    assert registry.default_path() == tmp_path / "engine-home" / "repos.json"


def test_Remember_EngineHomeNamed_WritesThereAndNotUnderHome(tmp_path, legacy_file):
    (repo,) = make_repos(tmp_path, "one")
    registry.remember(repo)
    assert registry.repos() == [repo.resolve()]
    assert (tmp_path / "engine-home" / "repos.json").is_file()
    assert not legacy_file.exists()


def test_Repos_EngineHomeNamed_NeverReadsOrWritesOldFile(tmp_path, legacy_file):
    kept, new = make_repos(tmp_path, "old", "new")
    write_legacy(legacy_file, [kept])
    before = legacy_file.read_bytes()
    assert registry.repos() == []
    registry.remember(new)
    assert registry.repos() == [new.resolve()]
    assert legacy_file.read_bytes() == before


def test_Repos_DefaultHomeWithoutFile_AdoptsExistingEntriesOnce(tmp_path, legacy_file, default_engine_home):
    kept, gone, later = make_repos(tmp_path, "kept", "gone", "later")
    write_legacy(legacy_file, [kept, gone])
    gone.rmdir()

    assert registry.repos() == [kept.resolve()]
    rewritten = json.dumps({"repos": [str(later)]})
    legacy_file.write_text(rewritten)

    assert registry.repos() == [kept.resolve()]
    assert legacy_file.read_text() == rewritten


def test_Remember_DefaultHomeWithoutFile_AdoptsOldEntriesThenAdds(tmp_path, legacy_file, default_engine_home):
    old, new = make_repos(tmp_path, "old", "new")
    write_legacy(legacy_file, [old])
    before = legacy_file.read_bytes()

    registry.remember(new)

    assert registry.repos() == [old.resolve(), new.resolve()]
    assert legacy_file.read_bytes() == before


def test_Repos_DefaultHomeWithFile_IgnoresOldFile(tmp_path, legacy_file, default_engine_home):
    old, current = make_repos(tmp_path, "old", "current")
    registry.remember(current)
    write_legacy(legacy_file, [old])

    assert registry.repos() == [current.resolve()]


def test_Repos_DefaultHomeWithCorruptOldFile_StartsEmptyAndLeavesIt(tmp_path, legacy_file, default_engine_home):
    legacy_file.parent.mkdir(parents=True)
    legacy_file.write_text("{not json")

    assert registry.repos() == []
    assert legacy_file.read_text() == "{not json"


def test_DefaultPath_EngineHomeInvalid_RaisesRegistryError(tmp_path, monkeypatch):
    monkeypatch.setenv(engine_home.ENV_VARIABLE, str(tmp_path / "missing"))
    with pytest.raises(registry.RegistryError, match="SAKURA_ENGINE_HOME"):
        registry.default_path()
