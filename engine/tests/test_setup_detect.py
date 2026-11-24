"""First-launch detection uses only supported programs and folder structure."""

import builtins
import io
import json
import os
import subprocess
from pathlib import Path

import pytest
from sakura import cli, programs
from sakura.flow import cage
from sakura.setup import detect
from sakura.usage import leeway
from test_acp_qualification import install


@pytest.fixture
def host(tmp_path, monkeypatch):
    home = Path.home()
    commands = tmp_path / "commands"
    commands.mkdir()
    monkeypatch.setenv("PATH", str(commands))
    for name in ("CLAUDE_CODE_EXECUTABLE", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "GOOSE_PATH_ROOT"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(programs, "install_folders", list)
    monkeypatch.setattr(cage, "account_home", lambda: home)
    return home, commands


def _executable(path):
    path.write_text("#!/bin/sh\nexit 1\n")
    path.chmod(0o700)
    return path


def _detect(capsys):
    assert cli.main(["setup", "detect", "--json"]) == 0
    captured = capsys.readouterr()
    assert captured.err == ""
    return json.loads(captured.out)


def _claude_folder(home, name=".claude"):
    folder = home / name
    (folder / "projects").mkdir(parents=True)
    (folder / "history.jsonl").touch()
    return folder


def _goose_folder(home, name=".goose-work"):
    folder = home / name
    (folder / "config").mkdir(parents=True)
    (folder / "config" / "config.yaml").touch()
    return folder


def test_direct_scan_needs_consent(host, monkeypatch):
    monkeypatch.setattr(cage, "account_home", lambda: pytest.fail("looked at home before consent"))
    with pytest.raises(detect.ConsentRequired):
        detect.detect()


def test_only_supported_harnesses_and_structural_candidates_listed(host, capsys, monkeypatch):
    home, commands = host
    for name in ("claude", "claude-agent-acp", "codex", "goose", "opencode"):
        _executable(commands / name)
    _claude_folder(home)
    (home / ".codex" / "sessions").mkdir(parents=True)
    _goose_folder(home)
    (home / ".opencode" / "sessions").mkdir(parents=True)
    monkeypatch.setattr(subprocess, "run", lambda *args, **kwargs: pytest.fail("ran a program during discovery"))
    monkeypatch.setattr(leeway, "accounts", lambda *args, **kwargs: pytest.fail("queried usage accounts during discovery"))
    found = _detect(capsys)
    assert [row["id"] for row in found["harnesses"]] == ["claude", "codex", "goose"]
    assert [row["reason"] for row in found["harnesses"]] == [
        "qualification_required", "install_adapter", "qualification_required",
    ]
    assert [row["harness"] for row in found["logins"]] == ["claude", "codex", "goose"]
    assert found["logins"][2]["readiness"] == "unknown"
    assert all(row["logged_in"] is None for row in found["logins"])


def test_candidate_files_are_never_opened(host, monkeypatch, capsys):
    home, commands = host
    folders = [_claude_folder(home), _goose_folder(home)]
    _executable(commands / "claude")

    def guarded(opener):
        def open_(file, *args, **kwargs):
            if not isinstance(file, int):
                assert not any(Path(os.fsdecode(file)).is_relative_to(folder) for folder in folders)
            return opener(file, *args, **kwargs)
        return open_

    for module in (builtins, io, os):
        monkeypatch.setattr(module, "open", guarded(module.open))
    assert [row["folder"] for row in _detect(capsys)["logins"]] == list(map(str, folders))


def test_missing_programs_have_actionable_reasons(host, capsys):
    rows = _detect(capsys)["harnesses"]
    assert len(rows) == 3
    for row in rows:
        assert row["program_path"] is None and row["found"] is False and row["version"] is None
        assert row["readiness"] == "unavailable" and row["reason"] == "install_program"


def test_incompatible_install_is_detected_without_a_probe(host, capsys, monkeypatch):
    _, commands = host
    real = install(commands / "fixture-adapter")
    (commands / "goose").symlink_to(real)
    monkeypatch.setattr(subprocess, "run", lambda *args, **kwargs: pytest.fail("ran a program during discovery"))
    first = _detect(capsys)["harnesses"][-1]
    assert first["adapter_version"] == first["version"] == "1.60.2"
    with real.open("a") as handle:
        handle.write("\n")
    row = _detect(capsys)["harnesses"][-1]
    assert row["reason"] == "incompatible_install" and row["readiness"] == "unavailable"


def test_projects_alone_do_not_identify_a_claude_login(host, capsys):
    home, _ = host
    real = _claude_folder(home, ".claude-work")
    other = home / ".claude-mem"
    (other / "projects").mkdir(parents=True)
    (other / "settings.json").mkdir()
    assert [row["folder"] for row in _detect(capsys)["logins"]] == [str(real)]


def test_default_folders_first_even_with_host_overrides(host, monkeypatch, capsys):
    home, _ = host
    claude_extra = _claude_folder(home, ".a-claude")
    claude_default = _claude_folder(home)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(claude_extra))
    rows = _detect(capsys)["logins"]
    assert [row["folder"] for row in rows] == list(map(str, [claude_default, claude_extra]))


def test_default_goose_config_is_a_structural_candidate(host, capsys):
    home, commands = host
    _executable(commands / "goose")
    native = home / ".config" / "goose"
    native.mkdir(parents=True)
    (native / "config.yaml").touch()
    [login] = _detect(capsys)["logins"]
    assert login["harness"] == "goose" and login["folder"] == str(native)
    assert login["readiness"] == "unknown"
