"""Where the engine keeps its state and config: outside every repository."""

import os
import pwd
import subprocess
from pathlib import Path

import pytest

from sakura import engine_home, repository
from sakura.flow import store


def test_root_is_the_environment_override_when_set_and_private(tmp_path, monkeypatch):
    (tmp_path / "engine").mkdir(mode=0o700)
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(tmp_path / "engine"))
    assert engine_home.root() == tmp_path / "engine"


def test_root_defaults_to_application_support_under_home(tmp_path, monkeypatch):
    monkeypatch.delenv("SAKURA_ENGINE_HOME", raising=False)
    assert engine_home.root(tmp_path) == tmp_path / "Library" / "Application Support" / "Sakura" / "engine"


def test_tests_never_use_the_real_engine_home():
    assert engine_home.root() != Path("~/Library/Application Support/Sakura/engine").expanduser()


def test_project_id_is_the_repository_name_and_a_short_hash_of_its_path(repo):
    project = engine_home.project_id(repo)
    name, digest = project.rsplit("-", 1)
    assert name == repo.name
    assert len(digest) == engine_home.HASH_LENGTH and int(digest, 16) >= 0
    assert engine_home.project_id(repo) == project


def test_project_id_is_the_same_from_a_linked_worktree(repo, tmp_path):
    linked = tmp_path / "linked"
    subprocess.run(["git", "-C", str(repo), "worktree", "add", "-q", "-b", "side", str(linked)], check=True)
    assert engine_home.project_id(linked) == engine_home.project_id(repo)


def test_project_id_differs_for_two_repositories_with_the_same_name(repo, tmp_path):
    other = tmp_path / "elsewhere" / repo.name
    subprocess.run(["git", "init", "-q", str(other)], check=True)
    assert engine_home.project_id(other) != engine_home.project_id(repo)


def test_task_state_lives_under_the_engine_home_not_in_git(repo):
    task = store.TaskStore(repo, "demo")
    assert task.root == engine_home.root() / engine_home.project_id(repo) / "flow" / "demo"
    assert repository.common_dir(repo) not in task.root.parents


def test_project_dir_is_created_private_on_first_use(repo):
    path = engine_home.ensure_project_dir(repo)
    assert path.is_dir()
    assert path.stat().st_mode & 0o077 == 0


def test_personal_config_file_is_at_the_engine_home():
    assert engine_home.config_file() == engine_home.root() / "config.toml"


def test_project_config_file_is_in_the_project_dir(repo):
    assert engine_home.project_config_file(repo) == engine_home.root() / engine_home.project_id(repo) / "config.toml"


# -- trust: the environment cannot move what hooks, brakes and stamp checks trust ----------------------------

ORIGINAL_ROOT_FOR_TRUST = engine_home.root_for_trust


def _private_dir(path: Path, mode: int = 0o700) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(mode)
    return path


def test_the_account_root_comes_from_the_password_database_not_from_HOME(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "elsewhere"))
    account = Path(pwd.getpwuid(os.getuid()).pw_dir)
    assert engine_home.account_root() == account / "Library" / "Application Support" / "Sakura" / "engine"


def test_root_for_trust_ignores_the_environment(tmp_path, monkeypatch):
    forged = _private_dir(tmp_path / "forged")
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(forged))
    monkeypatch.setenv("HOME", str(tmp_path / "forged-home"))
    assert ORIGINAL_ROOT_FOR_TRUST() == engine_home.account_root()
    assert engine_home.root() == forged


@pytest.mark.parametrize("mode", [0o755, 0o750, 0o701])
def test_an_override_that_is_not_private_is_refused(tmp_path, monkeypatch, mode):
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(_private_dir(tmp_path / "open", mode)))
    with pytest.raises(engine_home.EngineHomeError, match="0700"):
        engine_home.root()


def test_an_override_that_does_not_exist_is_refused(tmp_path, monkeypatch):
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(tmp_path / "missing"))
    with pytest.raises(engine_home.EngineHomeError, match="does not exist"):
        engine_home.root()


def test_an_override_that_is_a_symlink_is_refused(tmp_path, monkeypatch):
    real = _private_dir(tmp_path / "real")
    (tmp_path / "link").symlink_to(real)
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(tmp_path / "link"))
    with pytest.raises(engine_home.EngineHomeError, match="symlink"):
        engine_home.root()


def test_kit_records_stay_in_the_trusted_root_whatever_the_override(repo, tmp_path, monkeypatch):
    trusted = engine_home.root_for_trust()
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(_private_dir(tmp_path / "other")))
    assert trusted in engine_home.kit_dir(repo).parents
    assert tmp_path / "other" in engine_home.flow_dir(repo).parents
