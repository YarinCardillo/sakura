"""The `accounts` array of the cockpit snapshot: labels and settings for the dashboard, never paths or credentials."""

import json
import sqlite3
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path

import pytest

from sakura.cockpit import accounts, api, pages, snapshot
from sakura.cockpit.gate import Identity
from sakura.usage.archive import settings, store

NOW = datetime(2026, 10, 29, 12, 0).astimezone()
GLOBAL_RESERVE = 25.0


class NoTmux:
    def __getattr__(self, name):
        raise AssertionError(f"tmux was asked for {name}")


@pytest.fixture
def home(tmp_path, monkeypatch):
    path = tmp_path / "isolated-home"
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    return path


def write_accounts(home: Path, body: str) -> None:
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def entry(provider, account_id, directory, **extra) -> str:
    lines = [f'provider = "{provider}"', f'id = "{account_id}"', f'config_dir = "{directory}"']
    lines += [f"{key} = {value}" for key, value in extra.items()]
    return "[[accounts]]\n" + "\n".join(lines) + "\n"


def archive(home: Path):
    return store.open_db(settings.load(env={"HOME": str(home)}).db)


def add_quota(conn, key, ts, week, five_hour=10.0, resets="2026-11-02T00:00:00Z"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES (?, ?, 'max', 'allowed', ?,"
                 " '2026-10-29T15:00:00Z', ?, ?)", (key, ts, five_hour, week, resets))
    conn.commit()


def add_alert(conn, alert_id, key, resolved_at=None):
    conn.execute("INSERT INTO budget_alerts (id, account, kind, title, reason, since, resolved_at) VALUES"
                 " (?, ?, 'week', 't', 'r', '2026-10-28T00:00:00Z', ?)", (alert_id, key, resolved_at))
    conn.commit()


CLAUDE_PLAN_NAMES = {"pro": "Claude Pro", "max5x": "Claude Max 5x", "max20x": "Claude Max 20x"}


def collect():
    return snapshot.collect([], NOW)


def test_snapshot_lists_accounts_without_paths(home):
    (home / ".claude-work").mkdir(parents=True)
    (home / ".codex").mkdir()
    write_accounts(home, entry("codex", "main", home / ".codex")
                   + entry("claude", "work", home / ".claude-work", label='"Work"', reserve_week_pct=40)
                   + '[[accounts.plans]]\nplan = "pro"\nsince = 2026-01-15\n'
                   + '[[accounts.plans]]\nplan = "max5x"\nsince = 2026-10-20T08:30:00Z\n'
                   + entry("claude", "gone", home / "absent", enabled="false"))
    conn = archive(home)
    add_quota(conn, "work", "2026-10-29T10:00:00Z", 12.0)
    add_quota(conn, "work", "2026-10-29T11:00:00Z", 61.5, five_hour=33.0)
    add_quota(conn, "codex:main", "2026-10-29T11:30:00Z", 7.0)
    add_alert(conn, "a1", "work")
    add_alert(conn, "a2", "work")
    add_alert(conn, "a3", "work", resolved_at="2026-10-29T09:00:00Z")
    conn.close()

    document = api.snapshot_document(collect())

    assert [{k: v for k, v in row.items() if k not in ("target", "actions")} for row in document["accounts"]] == [
        {"provider": "claude", "id": "gone", "key": "gone", "label": "Claude, gone", "state": "disabled", "enabled": False,
         "week_pct": None, "week_resets_at": None, "five_hour_pct": None, "reserve_week_pct": GLOBAL_RESERVE,
         "reserve_week_pct_own": None, "stop_at_pct": 75.0, "plans": [], "plan_choices": ["pro", "max5x", "max20x"],
         "plan_label": "", "plan_choice_labels": CLAUDE_PLAN_NAMES, "open_alerts": 0},
        {"provider": "claude", "id": "work", "key": "work", "label": "Work", "state": "ok", "enabled": True, "week_pct": 61.5,
         "week_resets_at": "2026-11-02T00:00:00Z", "five_hour_pct": 33.0, "reserve_week_pct": 40.0,
         "reserve_week_pct_own": 40.0, "stop_at_pct": 60.0, "plan_choices": ["pro", "max5x", "max20x"],
         "plans": [{"plan": "max5x", "since": "2026-10-20T08:30:00Z", "label": "Claude Max 5x"},
                   {"plan": "pro", "since": "2026-01-15", "label": "Claude Pro"}],
         "plan_label": "", "plan_choice_labels": CLAUDE_PLAN_NAMES, "open_alerts": 2},
        {"provider": "codex", "id": "main", "key": "codex:main", "label": "Codex, main", "state": "ok", "enabled": True, "week_pct": 7.0,
         "week_resets_at": "2026-11-02T00:00:00Z", "five_hour_pct": 10.0, "reserve_week_pct": GLOBAL_RESERVE,
         "reserve_week_pct_own": None, "stop_at_pct": 75.0, "plans": [], "plan_choices": ["plus", "prolite", "pro"],
         "plan_label": "", "plan_choice_labels": {"plus": "Codex Plus", "prolite": "Codex Pro Lite", "pro": "Codex Pro"},
         "open_alerts": 0}]
    assert all(row["actions"] for row in document["accounts"])
    assert str(home) not in api.dumps(document)


def test_snapshot_accounts_without_a_database_have_null_numbers(home):
    (home / ".claude" / "projects").mkdir(parents=True)

    [account] = api.snapshot_document(collect())["accounts"]

    assert (account["week_pct"], account["week_resets_at"], account["five_hour_pct"], account["open_alerts"]) == (
        None, None, None, 0)


def test_snapshot_accounts_survive_a_database_without_tables(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    db = settings.load(env={"HOME": str(home)}).db
    db.parent.mkdir(parents=True)
    sqlite3.connect(db).close()

    [account] = api.snapshot_document(collect())["accounts"]

    assert (account["week_pct"], account["open_alerts"]) == (None, 0)


def test_snapshot_does_not_write_the_database(home):
    (home / ".claude" / "projects").mkdir(parents=True)

    collect()

    assert not settings.load(env={"HOME": str(home)}).db.exists()


def test_snapshot_accounts_on_zero_config(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    (home / ".codex" / "sessions").mkdir(parents=True)

    accounts = api.snapshot_document(collect())["accounts"]

    assert [(a["provider"], a["id"], a["label"], a["state"], a["enabled"]) for a in accounts] == [
        ("claude", "default", "Claude, personal", "ok", True), ("codex", "default", "Codex, personal", "ok", True)]


def test_snapshot_accounts_empty_on_a_bare_machine(home):
    assert api.snapshot_document(collect())["accounts"] == []


def test_broken_accounts_file_is_a_problem_line_not_a_failed_snapshot(home):
    write_accounts(home, "[[accounts]\nprovider = ")

    found = collect()
    document = api.snapshot_document(found)

    assert document["accounts"] == []
    assert found.problems == ["accounts.toml could not be read, no accounts listed; run sakura accounts"]
    assert document["problems"] == [*found.problems, "Usage is unavailable: the leeway helper was not found"]


def test_accounts_in_the_document_are_json_serializable(home):
    (home / ".claude" / "projects").mkdir(parents=True)

    assert json.loads(api.dumps(api.snapshot_document(collect())))["accounts"][0]["id"] == "default"


def test_cockpit_page_shows_one_line_per_account(home):
    (home / ".claude-work").mkdir(parents=True)
    write_accounts(home, entry("claude", "work", home / ".claude-work", label='"Work"'))
    conn = archive(home)
    add_quota(conn, "work", "2026-10-29T11:00:00Z", 61.5)
    conn.close()
    found = collect()

    page = pages.index(found, Identity("owner@example.com", "nMAC", "mac"), "csrf")

    assert "<p>Work (claude): week 61.5%, ok</p>" in page


def test_unreadable_accounts_problem_line_leaks_no_path_for_a_syntax_error(home):
    write_accounts(home, "[[accounts]\nprovider = ")

    assert all(str(home) not in line for line in collect().problems)


def test_unreadable_accounts_problem_line_leaks_no_path_for_a_shared_config_dir(home):
    shared = home / ".claude-shared"
    write_accounts(home, entry("claude", "one", shared) + entry("claude", "two", shared))

    found = collect()

    assert found.accounts == [] and len(found.problems) == 1
    assert all(str(home) not in line for line in found.problems)


def test_snapshot_picks_the_latest_row_across_mixed_time_forms(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    conn = archive(home)
    add_quota(conn, "default", "2026-10-29T09:00:00Z", 10.0)
    add_quota(conn, "default", "2026-10-29T11:00:00+00:00", 40.0)
    add_quota(conn, "default", "2026-10-29T10:00:00Z", 20.0)
    conn.close()

    [account] = api.snapshot_document(collect())["accounts"]

    assert account["week_pct"] == 40.0


def test_snapshot_ignores_rows_older_than_the_window(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    conn = archive(home)
    add_quota(conn, "default", "2026-10-20T11:00:00Z", 55.0)
    conn.close()

    [account] = api.snapshot_document(collect())["accounts"]

    assert (account["week_pct"], account["week_resets_at"], account["five_hour_pct"]) == (None, None, None)


def test_snapshot_shows_the_older_value_when_the_newest_row_has_no_week(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    conn = archive(home)
    add_quota(conn, "default", "2026-10-29T09:00:00Z", 30.0, five_hour=5.0)
    conn.execute("INSERT INTO quota_snapshots (account, ts, status) VALUES ('default', '2026-10-29T11:00:00Z', 'error')")
    conn.commit()
    conn.close()

    [account] = api.snapshot_document(collect())["accounts"]

    assert (account["week_pct"], account["five_hour_pct"]) == (30.0, 5.0)


def test_snapshot_with_non_utf8_accounts_toml_is_a_problem_line(home):
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b'[[accounts]]\nlabel = "caf\xe9"\n')

    found = collect()

    assert api.snapshot_document(found)["accounts"] == []
    assert found.problems == ["accounts.toml could not be read, no accounts listed; run sakura accounts"]


def test_account_encoding_ignores_fields_outside_the_contract(home):
    @dataclass(frozen=True)
    class WiderRow(accounts.AccountRow):
        config_dir: str = "/secret/path"

    (home / ".claude" / "projects").mkdir(parents=True)
    [row] = collect().accounts

    encoded = api._account(WiderRow(**asdict(row)))

    assert "config_dir" not in encoded
    assert set(encoded) == {"provider", "id", "key", "label", "state", "enabled", "week_pct", "week_resets_at",
                            "five_hour_pct", "reserve_week_pct", "reserve_week_pct_own", "open_alerts", "stop_at_pct", "plans",
                            "plan_choices", "target", "actions"}


def test_rows_of_a_host_without_accounts_toml_carry_no_actions(home):
    (home / ".claude" / "projects").mkdir(parents=True)

    [row] = api.snapshot_document(collect())["accounts"]

    assert row["actions"] == [] and row["target"] == '["account","claude","default"]'


def test_rows_of_a_legacy_host_carry_no_actions(home):
    (home / ".claude" / "projects").mkdir(parents=True)
    legacy = home / ".config" / "sakura" / "usage.toml"
    legacy.parent.mkdir(parents=True)
    legacy.write_text('[[sources]]\nprovider = "claude"\naccount = "alpha"\nroot = "~/.claude/projects"\n',
                      encoding="utf-8")

    rows = api.snapshot_document(collect())["accounts"]

    assert [row["id"] for row in rows] == ["alpha"] and rows[0]["actions"] == []
