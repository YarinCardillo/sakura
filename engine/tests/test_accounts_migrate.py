from __future__ import annotations

import hashlib
import os
import sqlite3
import stat
import tomllib
from pathlib import Path

import pytest

from sakura import cli
from sakura.accounts import detect, registry
from sakura.accounts.registry import AccountsConfigError
from sakura.usage.archive import settings

PLAN_PERIODS = (("alpha", "pro", "2026-01-01"), ("alpha", "max5", "2026-02-01"), ("alpha", "max20", "2026-03-01"),
                ("beta", "pro", "2026-01-15"), ("alpha", "max5", "2026-04-01"), ("beta", "max5", "2026-05-01"),
                ("alpha", "max20", "2026-06-01"), ("beta", "max20", "2026-07-01"), ("alpha", "max5", "2026-08-01"),
                ("beta", "pro", "2026-09-01"))
ALPHA_PLAN_COUNT = 6
EXTRA_USAGE_COUNT = 7


def usage_text(extra: str = "") -> str:
    head = '''db = "~/.local/state/sakura/usage.db"
claude_bin = "~/.local/bin/claude"
pr_repos = ["acme/app", "acme/site"]
pr_days = 45

[plan_weights]
pro = 1.0
max5 = 5
max20 = 20.0

[[sources]]
provider = "claude"
account = "alpha"
root = "~/.claude/projects"

[[sources]]
provider = "claude"
account = "beta"
root = "~/.altclaude/projects"

[[sources]]
provider = "codex"
account = "alpha"
root = "~/.codex/sessions"
'''
    plans = "".join(f'\n[[plans]]\naccount = "{who}"\nplan = "{plan}"\nsince = {since}\n' for who, plan, since in PLAN_PERIODS)
    extras = "".join(f'\n[[extra_usage]]\naccount = "alpha"\non = 2026-10-{day:02d}\namount = {day}.5\ncurrency = "EUR"\n'
                     for day in range(1, EXTRA_USAGE_COUNT + 1))
    return head + plans + extras + extra


@pytest.fixture
def home(tmp_path, monkeypatch) -> Path:
    path = tmp_path / "home"
    (path / ".config" / "sakura").mkdir(parents=True)
    monkeypatch.setenv("HOME", str(path))
    return path


def usage_file(home: Path) -> Path:
    return home / ".config" / "sakura" / "usage.toml"


def accounts_file(home: Path) -> Path:
    return home / ".config" / "sakura" / "accounts.toml"


def seed(home: Path, text: str | None = None) -> Path:
    path = usage_file(home)
    path.write_text(usage_text() if text is None else text, encoding="utf-8")
    return path


def keys(loaded: registry.Registry) -> list[tuple[str, str]]:
    return [(account.provider, account.id) for account in loaded.accounts]


def test_legacy_usage_toml_loads_without_detection(home, monkeypatch):
    seed(home)
    monkeypatch.setattr(detect, "detect", lambda *args, **kwargs: pytest.fail("detection must not run in legacy mode"))
    loaded = registry.load(env={"HOME": str(home)})
    assert (loaded.implicit, loaded.legacy) == (False, True)
    assert keys(loaded) == [("claude", "alpha"), ("claude", "beta"), ("codex", "alpha")]
    assert [account.config_dir for account in loaded.accounts] == [home / ".claude", home / ".altclaude", home / ".codex"]
    assert [account.label for account in loaded.accounts] == ["alpha", "beta", "alpha"]


def test_no_file_and_no_account_keys_stays_implicit(home):
    seed(home, 'pr_days = 5\n')
    assert registry.load(env={"HOME": str(home)}) == registry.Registry((), implicit=True)


def test_migration_keeps_every_id(home):
    seed(home)
    assert cli.main(["accounts", "migrate", "--write"]) == 0
    migrated = registry.load(env={"HOME": str(home)})
    assert (migrated.implicit, migrated.legacy) == (False, False)
    assert keys(migrated) == [("claude", "alpha"), ("claude", "beta"), ("codex", "alpha")]
    assert all(account.enabled and account.reserve_week_pct is None for account in migrated.accounts)


def test_plans_attach_to_the_claude_account(home):
    seed(home)
    cli.main(["accounts", "migrate", "--write"])
    alpha, beta, codex = registry.load(env={"HOME": str(home)}).accounts
    assert (len(alpha.plans), len(beta.plans), len(codex.plans)) == (ALPHA_PLAN_COUNT, 4, 0)
    assert (len(alpha.extra_usage), len(beta.extra_usage), len(codex.extra_usage)) == (EXTRA_USAGE_COUNT, 0, 0)
    assert all(plan.account == "alpha" for plan in alpha.plans)
    assert [plan.since for plan in alpha.plans] == sorted(plan.since for plan in alpha.plans)


def test_legacy_and_migrated_registries_are_equal_apart_from_the_flag(home):
    seed(home)
    legacy = registry.load(env={"HOME": str(home)})
    cli.main(["accounts", "migrate", "--write"])
    assert registry.load(env={"HOME": str(home)}).accounts == legacy.accounts


def test_dry_run_writes_nothing(home, capsys):
    original = seed(home).read_bytes()
    assert cli.main(["accounts", "migrate"]) == 0
    printed = capsys.readouterr().out
    assert printed.count("[[accounts]]") == 3
    assert "usage.toml keeps: db, claude_bin, pr_repos, pr_days, plan_weights" in printed
    assert usage_file(home).read_bytes() == original
    assert sorted(path.name for path in usage_file(home).parent.iterdir()) == ["usage.toml"]


def test_write_backs_up_and_keeps_only_the_other_keys(home):
    original = seed(home).read_bytes()
    cli.main(["accounts", "migrate", "--write"])
    (backup,) = usage_file(home).parent.glob("usage.toml.bak-*")
    assert backup.read_bytes() == original
    assert stat.S_IMODE(backup.stat().st_mode) == 0o600
    assert stat.S_IMODE(usage_file(home).stat().st_mode) == 0o600
    assert stat.S_IMODE(accounts_file(home).stat().st_mode) == 0o600
    kept = tomllib.loads(usage_file(home).read_text(encoding="utf-8"))
    assert kept == {"db": "~/.local/state/sakura/usage.db", "claude_bin": "~/.local/bin/claude",
                    "pr_repos": ["acme/app", "acme/site"], "pr_days": 45,
                    "plan_weights": {"pro": 1.0, "max5": 5, "max20": 20.0}}
    assert settings.load(env={"HOME": str(home)}).pr_days == 45


def test_second_run_is_a_no_op(home, capsys):
    seed(home)
    cli.main(["accounts", "migrate", "--write"])
    capsys.readouterr()
    before = {path.name: path.read_bytes() for path in usage_file(home).parent.iterdir() if path.suffix != ".lock"}
    assert cli.main(["accounts", "migrate", "--write"]) == 0
    assert "already migrated" in capsys.readouterr().out
    after = {path.name: path.read_bytes() for path in usage_file(home).parent.iterdir() if path.suffix != ".lock"}
    assert after == before


def test_usage_toml_with_sources_after_migration_is_an_error(home):
    seed(home)
    cli.main(["accounts", "migrate", "--write"])
    usage_file(home).write_text(usage_text(), encoding="utf-8")
    with pytest.raises(settings.UsageConfigError, match="run sakura accounts migrate"):
        settings.load(env={"HOME": str(home)})


def test_settings_without_accounts_toml_behaves_as_before(home):
    seed(home)
    loaded = settings.load(env={"HOME": str(home)})
    assert [(source.provider, source.account) for source in loaded.sources] == [
        ("claude", "alpha"), ("claude", "beta"), ("codex", "alpha")]


def test_archive_rows_untouched(home, tmp_path):
    database = tmp_path / "usage.db"
    connection = sqlite3.connect(database)
    connection.execute("CREATE TABLE turns (provider TEXT, account TEXT)")
    connection.execute("INSERT INTO turns VALUES ('claude', 'alpha'), ('codex', 'alpha')")
    connection.commit()
    connection.close()
    seed(home, usage_text().replace("~/.local/state/sakura/usage.db", str(database)))
    before = hashlib.sha256(database.read_bytes()).hexdigest()
    cli.main(["accounts", "migrate", "--write"])
    assert hashlib.sha256(database.read_bytes()).hexdigest() == before


def test_a_missing_config_dir_still_migrates(home):
    seed(home)
    assert not (home / ".altclaude").exists()
    cli.main(["accounts", "migrate", "--write"])
    assert ("claude", "beta") in keys(registry.load(env={"HOME": str(home)}))


def test_an_explicit_config_dir_equal_to_the_root_parent_converts(home):
    seed(home, '[[sources]]\nprovider = "claude"\naccount = "gamma"\nroot = "~/.gamma/projects"\nconfig_dir = "~/.gamma"\n')
    (gamma,) = registry.load(env={"HOME": str(home)}).accounts
    assert gamma.config_dir == home / ".gamma"


def test_an_explicit_config_dir_that_moves_the_root_is_refused_naming_the_source(home, capsys):
    original = seed(home, '[[sources]]\nprovider = "claude"\naccount = "gamma"\nroot = "/data/anywhere"\n'
                          'config_dir = "~/.gamma"\n').read_bytes()
    with pytest.raises(AccountsConfigError, match="claude/gamma"):
        registry.load(env={"HOME": str(home)})
    with pytest.raises(SystemExit):
        cli.main(["accounts", "migrate"])
    assert "claude/gamma" in capsys.readouterr().err
    assert usage_file(home).read_bytes() == original


def test_a_root_without_a_derivable_config_dir_is_refused_naming_the_source(home):
    seed(home, '[[sources]]\nprovider = "claude"\naccount = "gamma"\nroot = "/data/anywhere"\n')
    with pytest.raises(AccountsConfigError, match="claude/gamma"):
        registry.load(env={"HOME": str(home)})


def test_a_plan_for_an_unknown_account_is_refused_naming_it(home, capsys):
    original = seed(home, usage_text('\n[[plans]]\naccount = "ghost"\nplan = "pro"\nsince = 2026-01-01\n')).read_bytes()
    with pytest.raises(SystemExit):
        cli.main(["accounts", "migrate", "--write"])
    assert "ghost" in capsys.readouterr().err
    assert usage_file(home).read_bytes() == original
    assert not accounts_file(home).exists()


def test_a_plan_for_an_account_with_only_a_codex_source_is_refused(home):
    seed(home, '[[sources]]\nprovider = "codex"\naccount = "solo"\nroot = "~/.solo/sessions"\n'
               '[[plans]]\naccount = "solo"\nplan = "pro"\nsince = 2026-01-01\n')
    with pytest.raises(AccountsConfigError, match="solo"):
        registry.load(env={"HOME": str(home)})


def test_an_unknown_usage_key_refuses_migration_before_writing(home, capsys):
    original = seed(home, "mystery = 1\n" + usage_text()).read_bytes()
    with pytest.raises(SystemExit):
        cli.main(["accounts", "migrate", "--write"])
    assert "mystery" in capsys.readouterr().err
    assert usage_file(home).read_bytes() == original
    assert sorted(path.name for path in usage_file(home).parent.iterdir()) == ["accounts.toml.lock", "usage.toml"]


def test_a_failed_usage_rewrite_removes_the_new_accounts_file(home, monkeypatch):
    original = seed(home).read_bytes()
    real_replace = os.replace

    def failing_replace(source, target):
        if Path(target).name == "usage.toml":
            raise OSError("disk full")
        real_replace(source, target)

    monkeypatch.setattr(os, "replace", failing_replace)
    with pytest.raises(SystemExit):
        cli.main(["accounts", "migrate", "--write"])
    assert not accounts_file(home).exists()
    assert usage_file(home).read_bytes() == original


def test_nothing_to_migrate_without_account_keys(home, capsys):
    seed(home, 'pr_days = 5\n')
    assert cli.main(["accounts", "migrate", "--write"]) == 0
    assert "nothing to migrate" in capsys.readouterr().out
    assert not accounts_file(home).exists()


def test_a_non_string_account_is_refused_naming_the_key(home):
    seed(home, '[[sources]]\nprovider = "claude"\naccount = 7\nroot = "~/.claude/projects"\n')
    with pytest.raises(AccountsConfigError, match="sources.account"):
        registry.load(env={"HOME": str(home)})


def test_rename_on_a_legacy_host_is_refused_naming_migrate(home, capsys):
    original = seed(home).read_bytes()
    with pytest.raises(SystemExit) as stopped:
        cli.main(["accounts", "rename", "claude", "alpha", "Alpha"])
    assert stopped.value.code == 2
    assert "run sakura accounts migrate first" in capsys.readouterr().err
    assert not accounts_file(home).exists()
    assert usage_file(home).read_bytes() == original


def test_migrate_write_finishes_an_interrupted_migration(home, capsys):
    seed(home)
    cli.main(["accounts", "migrate", "--write"])
    migrated = accounts_file(home).read_bytes()
    usage_file(home).write_text(usage_text(), encoding="utf-8")
    capsys.readouterr()
    assert cli.main(["accounts", "migrate", "--write"]) == 0
    assert "finished an interrupted migration" in capsys.readouterr().out
    assert accounts_file(home).read_bytes() == migrated
    assert sorted(tomllib.loads(usage_file(home).read_text(encoding="utf-8"))) == [
        "claude_bin", "db", "plan_weights", "pr_days", "pr_repos"]
    assert settings.load(env={"HOME": str(home)}).pr_days == 45
    assert usage_text() in [path.read_text(encoding="utf-8") for path in usage_file(home).parent.glob("usage.toml.bak-*")]


def test_settings_with_non_utf8_usage_toml_is_a_usage_config_error(home):
    usage_file(home).write_bytes(b'pr_repos = ["caf\xe9"]\n')
    with pytest.raises(settings.UsageConfigError, match="not valid UTF-8"):
        settings.load(env={"HOME": str(home)})
