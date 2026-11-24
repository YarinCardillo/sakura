from __future__ import annotations

import json
from pathlib import Path

import pytest

from sakura import cli
from sakura.accounts import registry, write
from sakura.accounts.registry import Account

FIELDS = {"provider", "id", "key", "label", "enabled", "state"}


@pytest.fixture
def home(tmp_path, monkeypatch) -> Path:
    path = tmp_path / "home"
    (path / ".config" / "sakura").mkdir(parents=True)
    monkeypatch.setenv("HOME", str(path))
    return path


def register(home: Path, provider: str, account_id: str, label: str, enabled: bool = True, exists: bool = True) -> Path:
    directory = home / f".{provider}-{account_id}"
    if exists:
        directory.mkdir()
    write.add(registry.default_path({"HOME": str(home)}),
              Account(provider, account_id, label, directory, enabled, None, (), ()))
    return directory


def list_json(capsys) -> list[dict]:
    assert cli.main(["accounts", "list", "--json"]) == 0
    out = capsys.readouterr().out
    assert out.count("\n") == 1 and out.endswith("\n")
    return json.loads(out)


def test_list_json_has_exactly_the_fields_and_no_paths(home, capsys):
    directory = register(home, "claude", "work", "Zoë Work")
    register(home, "claude", "off", "Off", enabled=False)
    register(home, "claude", "gone", "Gone", exists=False)
    rows = list_json(capsys)
    assert [row["id"] for row in rows] == ["gone", "off", "work"]
    assert all(set(row) == FIELDS for row in rows)
    assert rows[2] == {"provider": "claude", "id": "work", "key": "work", "label": "Zoë Work", "enabled": True, "state": "ok"}
    assert [(row["enabled"], row["state"]) for row in rows[:2]] == [(True, "missing"), (False, "disabled")]
    assert str(directory) not in json.dumps(rows) and str(home) not in json.dumps(rows)


def test_list_json_prints_non_ascii_labels_unescaped(home, capsys):
    register(home, "claude", "work", "Zoë 日本")
    assert cli.main(["accounts", "list", "--json"]) == 0
    assert "Zoë 日本" in capsys.readouterr().out


def test_list_json_legacy_usage_toml_lists_its_accounts(home, capsys):
    (home / ".config" / "sakura" / "usage.toml").write_text(
        '[[sources]]\nprovider = "codex"\naccount = "beta"\nroot = "~/.codex/sessions"\n\n'
        '[[sources]]\nprovider = "claude"\naccount = "alpha"\nroot = "~/.claude/projects"\n', encoding="utf-8")
    assert not registry.default_path({"HOME": str(home)}).exists()
    rows = list_json(capsys)
    assert [(row["provider"], row["id"], row["key"]) for row in rows] == [("claude", "alpha", "alpha"), ("codex", "beta", "codex:beta")]


def test_list_json_without_any_accounts_is_an_empty_array(home, capsys):
    assert list_json(capsys) == []


def test_list_json_includes_zero_config_implicit_accounts(home, capsys):
    (home / ".claude" / "projects").mkdir(parents=True)
    rows = list_json(capsys)
    assert len(rows) == 1
    assert (rows[0]["provider"], rows[0]["key"], rows[0]["state"]) == ("claude", rows[0]["id"], "ok")


def test_list_json_key_is_the_snapshot_key(home, capsys):
    register(home, "codex", "personal", "Codex")
    register(home, "claude", "personal", "Claude")
    rows = list_json(capsys)
    assert [(row["provider"], row["id"], row["key"]) for row in rows] == [
        ("claude", "personal", "personal"), ("codex", "personal", "codex:personal")]


def test_list_json_broken_file_exits_2_with_empty_stdout(home, capsys):
    registry.default_path({"HOME": str(home)}).write_text("[[accounts]\nbroken", encoding="utf-8")
    with pytest.raises(SystemExit) as raised:
        cli.main(["accounts", "list", "--json"])
    captured = capsys.readouterr()
    assert raised.value.code == 2 and captured.out == "" and "accounts.toml" in captured.err


def test_list_without_json_equals_bare_accounts(home, capsys):
    register(home, "claude", "work", "Work")
    cli.main(["accounts"])
    bare = capsys.readouterr().out
    cli.main(["accounts", "list"])
    assert capsys.readouterr().out == bare != ""
