"""Profile discovery and explicit hook installation, entirely in temporary homes."""

import json
import os
import shlex
import subprocess
from pathlib import Path

from sakura.quota_enforcement import install

KIT = Path(__file__).resolve().parents[1] / "bin/sakura"


def test_discovery_covers_default_alternate_env_registry_and_orca(tmp_path):
    directories = [tmp_path / ".claude", tmp_path / ".claude-work", tmp_path / "custom",
                   tmp_path / "registered", tmp_path / "Library/Application Support/orca/claude-profiles/one/home"]
    for directory in directories:
        directory.mkdir(parents=True)
    alias = tmp_path / "Library/Application Support/orca/claude-profiles/two/home"
    alias.parent.mkdir(parents=True)
    alias.symlink_to(directories[0], target_is_directory=True)
    config = tmp_path / ".config/sakura/accounts.toml"
    config.parent.mkdir(parents=True)
    config.write_text('[[accounts]]\nprovider="claude"\nid="extra"\nlabel="Extra"\n'
                      f'config_dir="{directories[3]}"\nenabled=true\n')
    env = {"HOME": str(tmp_path), "CLAUDE_CONFIG_DIR": str(directories[2])}
    assert set(install.discover(tmp_path, env)) == set(directories)


def test_install_is_idempotent_preserves_settings_and_binds_profile(tmp_path):
    settings = tmp_path / "settings.json"
    settings.write_text(json.dumps({"model": "opus", "hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "existing"}]}]}}))
    settings.chmod(0o640)
    install.profiles([tmp_path], KIT)
    first = settings.read_bytes()
    install.profiles([tmp_path], KIT)
    assert settings.read_bytes() == first
    value = json.loads(first)
    assert value["model"] == "opus"
    assert value["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == "existing"
    for event in ("SessionStart", "UserPromptSubmit", "PreToolUse"):
        command = value["hooks"][event][-1]["hooks"][0]["command"]
        tokens = shlex.split(command)
        assert tokens[:4] == [str(KIT), "quota-hooks", "run", event]
        assert tokens[tokens.index("--config-dir") + 1] == str(tmp_path)
    assert settings.stat().st_mode & 0o777 == 0o640


def test_install_reconfiguration_replaces_only_its_own_hooks(tmp_path):
    install.profiles([tmp_path], KIT, "http://127.0.0.1:8799")
    install.profiles([tmp_path], KIT, "http://127.0.0.1:8800")
    hooks = json.loads((tmp_path / "settings.json").read_text())["hooks"]
    assert len(hooks["SessionStart"]) == 1
    assert "8800" in hooks["SessionStart"][0]["hooks"][0]["command"]


def test_install_invalid_settings_leaves_original_untouched(tmp_path):
    settings = tmp_path / "settings.json"
    settings.write_text('{"hooks": []}')
    result = subprocess.run([str(KIT), "quota-hooks", "install", "--profile", str(tmp_path)],
                            env={**os.environ, "HOME": str(tmp_path)}, capture_output=True, text=True)
    assert result.returncode == 1
    assert "hooks" in result.stderr
    assert settings.read_text() == '{"hooks": []}'


def test_cli_install_and_offline_run_use_only_fixture_profile(tmp_path):
    result = subprocess.run([str(KIT), "quota-hooks", "install", "--profile", str(tmp_path),
                             "--advisor-url", "http://127.0.0.1:1"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    settings = json.loads((tmp_path / "settings.json").read_text())
    command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
    result = subprocess.run(shlex.split(command), input=json.dumps({"cwd": str(tmp_path),
                            "tool_name": "Agent", "tool_input": {"prompt": "Build"}}),
                            capture_output=True, text=True)
    assert result.returncode == 0
    assert result.stdout == ""
    assert "advisor unavailable" in result.stderr
