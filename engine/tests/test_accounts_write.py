from __future__ import annotations

import os
import stat
import threading
from dataclasses import replace
from datetime import date, datetime, timezone
from pathlib import Path

import pytest

from sakura import cli
from sakura.accounts import registry, write
from sakura.accounts.registry import Account, AccountsConfigError, ExtraUsage, Plan


def account(tmp_path: Path, account_id: str = "personal", provider: str = "claude", **overrides) -> Account:
    fields = dict(provider=provider, id=account_id, label=account_id.title(), config_dir=tmp_path / f".{account_id}",
                  enabled=True, reserve_week_pct=None, plans=(), extra_usage=())
    return Account(**{**fields, **overrides})


@pytest.fixture
def config(tmp_path) -> Path:
    return tmp_path / "home" / ".config" / "sakura" / "accounts.toml"


@pytest.fixture
def home(tmp_path, monkeypatch) -> Path:
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    return tmp_path / "home"


def test_add_then_load_round_trips(tmp_path, config):
    rich = account(
        tmp_path, "work", "claude", enabled=False, reserve_week_pct=12.5,
        plans=(Plan("work", "max5", datetime(2026, 1, 2, 3, 4, 5, 678, tzinfo=timezone.utc), True),
               Plan("work", "max20", datetime(2026, 6, 1, tzinfo=timezone.utc))),
        extra_usage=(ExtraUsage("work", date(2026, 3, 4), 25.0, "EUR", "top up"),
                     ExtraUsage("work", date(2026, 4, 5), 0.5, "USD")))
    write.add(config, account(tmp_path, "personal"))
    write.add(config, rich)
    assert registry.load(config) == registry.Registry((account(tmp_path, "personal"), rich), implicit=False)


def test_label_with_quotes_and_accents_round_trips(tmp_path, config):
    label = 'Zoë "the \\ boss" \t\n café 日本 \x07'
    write.add(config, account(tmp_path, label=label))
    assert registry.load(config).accounts[0].label == label


def test_rename_changes_label_not_id(tmp_path, config):
    write.add(config, account(tmp_path))
    write.add(config, account(tmp_path, provider="codex"))
    write.relabel(config, "claude", "personal", "Work")
    labels = [(a.provider, a.id, a.label) for a in registry.load(config).accounts]
    assert labels == [("claude", "personal", "Work"), ("codex", "personal", "Personal")]


def test_add_refuses_a_dir_another_account_owns(tmp_path, config):
    write.add(config, account(tmp_path, "one"))
    before = config.read_bytes()
    with pytest.raises(AccountsConfigError, match="'one' and 'two'"):
        write.add(config, account(tmp_path, "two", config_dir=tmp_path / ".one"))
    assert config.read_bytes() == before


def test_add_refuses_a_duplicate_account(tmp_path, config):
    write.add(config, account(tmp_path))
    with pytest.raises(AccountsConfigError, match="duplicate account claude/personal"):
        write.add(config, account(tmp_path, config_dir=tmp_path / ".other"))


def test_failed_write_leaves_the_old_file(tmp_path, config, monkeypatch):
    write.add(config, account(tmp_path, "one"))
    before = config.read_bytes()

    def broken_replace(source, target):
        raise OSError("disk full")

    monkeypatch.setattr(os, "replace", broken_replace)
    with pytest.raises(OSError, match="disk full"):
        write.add(config, account(tmp_path, "two"))
    assert config.read_bytes() == before
    assert sorted(path.name for path in config.parent.iterdir()) == ["accounts.toml", "accounts.toml.lock"]


def test_invalid_content_is_refused_before_anything_is_written(tmp_path, config):
    write.add(config, account(tmp_path, "one"))
    before = config.read_bytes()
    with pytest.raises(AccountsConfigError, match="reserve_week_pct"):
        write.set_ceiling(config, "claude", "one", 101)
    with pytest.raises(AccountsConfigError, match="slug"):
        write.add(config, account(tmp_path, "Bad Id"))
    assert config.read_bytes() == before


def test_file_mode_is_0600_and_dir_is_0700(tmp_path, config):
    write.add(config, account(tmp_path))
    write.set_enabled(config, "claude", "personal", False)
    assert stat.S_IMODE(config.stat().st_mode) == 0o600
    assert stat.S_IMODE(config.parent.stat().st_mode) == 0o700


def test_set_enabled_and_ceiling_change_only_their_field(tmp_path, config):
    write.add(config, account(tmp_path))
    write.set_enabled(config, "claude", "personal", False)
    write.set_ceiling(config, "claude", "personal", 20)
    assert registry.load(config).accounts[0] == account(tmp_path, enabled=False, reserve_week_pct=20.0)
    write.set_ceiling(config, "claude", "personal", None)
    assert registry.load(config).accounts[0].reserve_week_pct is None


@pytest.mark.parametrize("change", [
    lambda path: write.relabel(path, "claude", "ghost", "x"),
    lambda path: write.set_enabled(path, "codex", "personal", False),
    lambda path: write.set_ceiling(path, "claude", "ghost", 5),
])
def test_unknown_account_is_an_error_naming_it(tmp_path, config, change):
    write.add(config, account(tmp_path))
    with pytest.raises(AccountsConfigError, match=r"no account (claude/ghost|codex/personal)"):
        change(config)


def test_concurrent_writers_both_land(tmp_path, config):
    names = [f"acct-{index}" for index in range(8)]
    start = threading.Barrier(len(names))
    errors: list[BaseException] = []

    def add_one(name: str) -> None:
        try:
            start.wait()
            write.add(config, account(tmp_path, name))
        except BaseException as error:
            errors.append(error)

    threads = [threading.Thread(target=add_one, args=(name,)) for name in names]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert errors == []
    assert sorted(a.id for a in registry.load(config).accounts) == sorted(names)


def test_list_shows_missing_dir(tmp_path, home, capsys):
    (tmp_path / ".personal").mkdir()
    path = registry.default_path({"HOME": str(home)})
    write.add(path, account(tmp_path, "personal"))
    write.add(path, account(tmp_path, "gone"))
    write.add(path, account(tmp_path, "off", config_dir=tmp_path / ".personal-2", enabled=False))
    assert cli.main(["accounts"]) == 0
    assert capsys.readouterr().out.splitlines() == [
        f"claude  personal  Personal  {tmp_path / '.personal'}  ok",
        f"claude  gone  Gone  {tmp_path / '.gone'}  missing",
        f"claude  off  Off  {tmp_path / '.personal-2'}  disabled",
    ]


def test_list_without_a_file_points_at_detect(home, capsys):
    assert cli.main(["accounts"]) == 0
    assert capsys.readouterr().out.strip() == "no accounts.toml (run sakura accounts detect)"


def test_cli_add_rename_disable_enable_set_ceiling(tmp_path, home):
    (tmp_path / "work").mkdir()
    assert cli.main(["accounts", "add", "--provider", "claude", "--id", "work", "--config-dir", str(tmp_path / "work")]) == 0
    assert cli.main(["accounts", "rename", "claude", "work", "Work Account"]) == 0
    assert cli.main(["accounts", "disable", "claude", "work"]) == 0
    assert cli.main(["accounts", "set-ceiling", "claude", "work", "30"]) == 0
    added = registry.load(registry.default_path({"HOME": str(home)})).accounts[0]
    assert (added.label, added.enabled, added.reserve_week_pct) == ("Work Account", False, 30.0)
    assert cli.main(["accounts", "enable", "claude", "work"]) == 0
    assert cli.main(["accounts", "set-ceiling", "claude", "work", "none"]) == 0
    added = registry.load(registry.default_path({"HOME": str(home)})).accounts[0]
    assert (added.enabled, added.reserve_week_pct) == (True, None)


def test_cli_add_refuses_a_missing_dir(tmp_path, home, capsys):
    with pytest.raises(SystemExit) as stopped:
        cli.main(["accounts", "add", "--provider", "claude", "--id", "work", "--config-dir", str(tmp_path / "nope")])
    assert stopped.value.code == 2
    assert "not a directory" in capsys.readouterr().err
    assert not registry.default_path({"HOME": str(home)}).exists()


def test_cli_add_refuses_a_dir_another_account_owns(tmp_path, home, capsys):
    (tmp_path / "work").mkdir()
    args = ["accounts", "add", "--provider", "claude", "--config-dir", str(tmp_path / "work")]
    assert cli.main([*args, "--id", "one"]) == 0
    with pytest.raises(SystemExit) as stopped:
        cli.main([*args, "--id", "two"])
    assert stopped.value.code == 2
    assert "share the config directory" in capsys.readouterr().err


@pytest.mark.parametrize("pct", ["abc", "101"])
def test_cli_set_ceiling_refuses_a_bad_percentage(tmp_path, home, capsys, pct):
    (tmp_path / "work").mkdir()
    cli.main(["accounts", "add", "--provider", "claude", "--id", "work", "--config-dir", str(tmp_path / "work")])
    with pytest.raises(SystemExit) as stopped:
        cli.main(["accounts", "set-ceiling", "claude", "work", pct])
    assert stopped.value.code == 2


def test_add_on_zero_config_starts_from_an_empty_file(tmp_path, home):
    for name in (".claude", ".claude-work"):
        (home / name / "projects").mkdir(parents=True)
    args = ["accounts", "add", "--provider", "claude", "--id", "work", "--config-dir", str(home / ".claude-work")]
    assert cli.main(args) == 0
    written = registry.load(registry.default_path({"HOME": str(home)}))
    assert [(a.provider, a.id, a.config_dir) for a in written.accounts] == [
        ("claude", "work", (home / ".claude-work").resolve())]


def test_write_expands_home_from_the_given_env(tmp_path, config, monkeypatch):
    elsewhere, given = tmp_path / "elsewhere", tmp_path / "given"
    monkeypatch.setenv("HOME", str(elsewhere))
    config.parent.mkdir(parents=True)
    config.write_text('[[accounts]]\nprovider = "claude"\nid = "work"\nconfig_dir = "~/.work"\n', encoding="utf-8")
    write.relabel(config, "claude", "work", "Work", env={"HOME": str(given)})
    assert registry.load(config, {"HOME": str(given)}).accounts[0].config_dir == given / ".work"
    assert str(given / ".work") in config.read_text(encoding="utf-8")
