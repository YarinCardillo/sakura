import os
import stat

import pytest

from sakura import engine_home
from sakura.cockpit import settings
from sakura.flow import config
from cockpit_support import settings_text, write_tailscale


def _config_file(tmp_path, text):
    path = tmp_path / "cockpit.toml"
    path.write_text(text)
    return path


@pytest.fixture
def helper(tmp_path):
    directory = tmp_path / "bin"
    directory.mkdir()
    return write_tailscale(directory, marker=tmp_path / "helper-ran")


@pytest.mark.parametrize("layer", ["personal", "shared", "local", "flags"])
def test_repo_layer_cockpit_table_rejected_before_any_helper_runs(repo, tmp_path, layer, helper):
    ran = tmp_path / "helper-ran"
    attacker = f'[cockpit]\nport = 9\nallow_logins = ["attacker@example.com"]\ntailscale = "{helper}"\n'
    home = tmp_path / "home"
    overrides = None
    if layer == "personal":
        engine_home.config_file().write_text(attacker)
    elif layer == "shared":
        (repo / ".sakura").mkdir()
        (repo / ".sakura/flow.toml").write_text(attacker)
    elif layer == "local":
        (repo / ".sakura").mkdir()
        (repo / ".sakura/local.toml").write_text(attacker.replace("[cockpit]", "[flow.cockpit]"))
    else:
        overrides = {"cockpit": {"port": 9}}
    with pytest.raises(config.FlowConfigError, match="cockpit settings belong in ~/.config/sakura/cockpit.toml only"):
        config.load(repo, home=home, overrides=overrides)
    assert not ran.exists()


def test_empty_cockpit_table_is_rejected_too(repo, tmp_path):
    (repo / ".sakura").mkdir()
    (repo / ".sakura/flow.toml").write_text("[cockpit]\n")
    with pytest.raises(config.FlowConfigError, match="cockpit settings belong"):
        config.load(repo, home=tmp_path / "home")


def test_trusted_user_config_starts(tmp_path, helper):
    path = _config_file(tmp_path, settings_text(helper, logins=["owner@example.com"], nodes=["nMAC"], port=8788))
    loaded = settings.load(path, registered_repos=[])
    assert loaded.port == 8788
    assert loaded.allow_logins == frozenset({"owner@example.com"})
    assert loaded.allow_nodes == frozenset({"nMAC"})
    assert loaded.tailscale == helper


def test_default_path_lives_under_the_home_the_process_sees(tmp_path, helper, monkeypatch):
    home = tmp_path / "other-home"
    (home / ".config/sakura").mkdir(parents=True)
    (home / ".config/sakura/cockpit.toml").write_text(settings_text(helper))
    monkeypatch.setenv("HOME", str(home))
    assert settings.load(registered_repos=[]).port == 8787


def test_missing_file_refuses_to_start(tmp_path):
    with pytest.raises(settings.CockpitRefused, match="cannot read"):
        settings.load(tmp_path / "absent.toml", registered_repos=[])


def test_empty_lists_load_and_mean_nobody(tmp_path, helper):
    loaded = settings.load(_config_file(tmp_path, settings_text(helper, logins=[], nodes=[])), registered_repos=[])
    assert loaded.allow_logins == frozenset() and loaded.allow_nodes == frozenset()


@pytest.mark.parametrize("line", [
    'port = 0', 'port = 70000', 'port = "8787"', 'port = true',
    'allow_logins = "owner@example.com"', 'allow_logins = [1]', 'allow_logins = ["a b"]',
    'allow_nodes = ["n\\nMAC"]', 'allow_nodes = [""]', 'surprise = 1',
])
def test_malformed_values_refuse_to_start(tmp_path, helper, line):
    key = line.split("=")[0].strip()
    body = "\n".join(row for row in settings_text(helper).splitlines() if not row.startswith(key)) + "\n" + line + "\n"
    with pytest.raises(settings.CockpitRefused):
        settings.load(_config_file(tmp_path, body), registered_repos=[])


def test_tailscale_path_must_be_absolute_and_not_writable_by_others(tmp_path, helper):
    relative = _config_file(tmp_path, settings_text(helper).replace(str(helper), "tailscale"))
    with pytest.raises(settings.CockpitRefused, match="absolute"):
        settings.load(relative, registered_repos=[])

    absent = _config_file(tmp_path, settings_text(tmp_path / "nowhere/tailscale"))
    with pytest.raises(settings.CockpitRefused, match="not an executable file"):
        settings.load(absent, registered_repos=[])

    for mode, word in ((0o775, "writable"), (0o757, "writable")):
        helper.chmod(mode)
        with pytest.raises(settings.CockpitRefused, match=word):
            settings.load(_config_file(tmp_path, settings_text(helper)), registered_repos=[])

    helper.chmod(stat.S_IRUSR | stat.S_IWUSR)
    with pytest.raises(settings.CockpitRefused, match="not an executable file"):
        settings.load(_config_file(tmp_path, settings_text(helper)), registered_repos=[])


def test_tailscale_under_a_registered_repository_is_refused(tmp_path, repo):
    inside = repo / "tools"
    inside.mkdir()
    helper = write_tailscale(inside)
    with pytest.raises(settings.CockpitRefused, match="registered repository"):
        settings.load(_config_file(tmp_path, settings_text(helper)), registered_repos=[repo])


def test_symlink_into_a_registered_repository_is_refused(tmp_path, repo):
    real = write_tailscale(repo)
    link = tmp_path / "linked-tailscale"
    link.symlink_to(real)
    with pytest.raises(settings.CockpitRefused, match="registered repository"):
        settings.load(_config_file(tmp_path, settings_text(link)), registered_repos=[repo])


def test_tailscale_owned_by_someone_else_is_refused(tmp_path, helper, monkeypatch):
    monkeypatch.setattr(os, "geteuid", lambda: os.stat(helper).st_uid + 4242)
    with pytest.raises(settings.CockpitRefused, match="owned by"):
        settings.load(_config_file(tmp_path, settings_text(helper)), registered_repos=[])


def test_registered_repositories_are_a_required_argument(tmp_path, helper):
    path = _config_file(tmp_path, settings_text(helper))
    with pytest.raises(TypeError, match="registered_repos"):
        settings.load(path)
    with pytest.raises(TypeError, match="registered_repos"):
        settings.load()
