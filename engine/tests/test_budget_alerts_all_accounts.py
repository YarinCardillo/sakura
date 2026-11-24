"""Alerts for every enabled account: each gets its own pace alert, Claude accounts also overage and outside use."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura.accounts.registry import Account, Registry
from sakura.budget import alerts
from sakura.budget.settings import BudgetSettings
from sakura.flow import notify
from sakura.usage.archive import command, settings as archive_settings, store

UTC = timezone.utc
NOW = datetime(2026, 10, 28, 14, 0, tzinfo=UTC)
WEEK_RESET = NOW + timedelta(days=3)
SETTINGS = BudgetSettings(account="", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
                          ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00",
                          snapshot_max_age_minutes=30)
STEEP = ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0))
GENTLE = ((4, 40.0), (3, 40.25), (2, 40.5), (1, 40.75), (0.05, 41.0))


class FakeTransport:
    def __init__(self):
        self.posts = []

    def post(self, event, title, message):
        self.posts.append((event, title, message))


@pytest.fixture
def conn(tmp_path):
    connection = store.open_db(tmp_path / "usage.db")
    yield connection
    connection.close()


def claude(key, reserve=None):
    return alerts.AlertAccount(key, "claude", reserve)


def codex(key, reserve=None):
    return alerts.AlertAccount(f"codex:{key}", "codex", reserve)


def snapshot(conn, account, week, at, overage=None):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES (?, ?, 'max', 'allowed', 10.0, NULL, ?, ?, ?)",
                 (account, at.isoformat(), week, WEEK_RESET.isoformat(), overage))
    conn.commit()


def week_of(conn, account, series=STEEP):
    for hours_ago, pct in series:
        snapshot(conn, account, pct, NOW - timedelta(hours=hours_ago))


def outside_day(conn, account):
    """Oct 27: four 2% rises explained by 1000 weighted tokens each; Oct 28: +12% on 1000 tokens."""
    base = datetime(2026, 10, 27, 8, 0, tzinfo=UTC)
    for step in range(5):
        snapshot(conn, account, 10.0 + 2 * step, base + timedelta(hours=step))
        if step < 4:
            _turn(conn, account, f"{account}-k{step}", base + timedelta(hours=step, minutes=5))
    snapshot(conn, account, 18.0, datetime(2026, 10, 28, 13, 0, tzinfo=UTC))
    _turn(conn, account, f"{account}-today", datetime(2026, 10, 28, 13, 40, tzinfo=UTC))
    snapshot(conn, account, 30.0, datetime(2026, 10, 28, 13, 50, tzinfo=UTC))


def _turn(conn, account, key, at):
    conn.execute("INSERT INTO turns VALUES ('claude', ?, ?, 's', NULL, ?, NULL, NULL, 'claude-opus-5-5',"
                 " 0, 0, 0, 200, NULL)", (key, account, at.isoformat()))


def pace(found):
    """The pace alerts (a steep test week also rises faster than local turns explain, so it opens outside ones)."""
    return [alert for alert in found if alert.kind == "pace"]


def kinds(found):
    return sorted((alert.account, alert.kind) for alert in found)


def test_every_enabled_account_gets_its_pace_alert(conn):
    for key in ("a", "b", "codex:c"):
        week_of(conn, key)

    found = alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a"), claude("b"), codex("c")])

    assert {alert.id for alert in pace(found)} == {
        f"pace:a:{WEEK_RESET.isoformat()}", f"pace:b:{WEEK_RESET.isoformat()}",
        f"pace:codex:c:{WEEK_RESET.isoformat()}"}


def test_account_ceiling_overrides_the_global_reserve(conn):
    for key in ("a", "b"):
        week_of(conn, key, GENTLE)

    found = alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a"), claude("b", reserve=50.0)])

    assert kinds(found) == [("b", "pace")], "about 59% passes b's 50% ceiling but not the global 75%"
    assert "50% ceiling (reserve 50%)" in found[0].reason


def test_overage_and_outside_only_for_claude_accounts(conn):
    outside_day(conn, "a")
    outside_day(conn, "b")
    snapshot(conn, "a", 50.0, NOW, overage=1)
    snapshot(conn, "codex:c", 50.0, NOW, overage=1)
    outside_day(conn, "codex:c")

    found = alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a"), claude("b"), codex("c")])

    assert kinds([alert for alert in found if alert.kind != "pace"]) == [
        ("a", "outside"), ("a", "overage"), ("b", "outside")]


def test_existing_alert_ids_unchanged(conn):
    week_of(conn, "personal")
    old_id = f"pace:personal:{WEEK_RESET.isoformat()}"
    conn.execute("INSERT INTO budget_alerts (id, account, kind, title, reason, since, resolved_at, url, pushed_at)"
                 " VALUES (?, 'personal', 'pace', 'old title', 'old reason', '2026-10-27T09:00:00+00:00', NULL, NULL,"
                 " '2026-10-27T09:00:05+00:00')", (old_id,))
    transport = FakeTransport()

    found = pace(alerts.evaluate(conn, SETTINGS, NOW, (), [claude("personal")]))
    result = alerts.sync(conn, found, NOW, notify.AlertPusher(transport, lambda: "day", Path("/nonexistent")),
                         evaluated=["personal"])

    assert [alert.id for alert in found] == [old_id]
    assert result.opened == () and transport.posts == []
    assert conn.execute("SELECT id, since, pushed_at, resolved_at FROM budget_alerts").fetchall() == [
        (old_id, "2026-10-27T09:00:00+00:00", "2026-10-27T09:00:05+00:00", None)]


def test_disabled_account_is_not_evaluated_and_its_alerts_stay(conn):
    week_of(conn, "a")
    week_of(conn, "off")
    both = pace(alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a"), claude("off")]))
    alerts.sync(conn, both, NOW, evaluated=["a", "off"])
    conn.execute("DELETE FROM quota_snapshots WHERE account = 'a'")
    conn.commit()
    week_of(conn, "a", ((0.05, 20.0), (0.04, 20.0)))

    found = pace(alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a")]))
    result = alerts.sync(conn, found, NOW, evaluated=["a"])

    assert found == [] and result.resolved == (f"pace:a:{WEEK_RESET.isoformat()}",)
    assert conn.execute("SELECT id FROM budget_alerts WHERE kind = 'pace' AND resolved_at IS NULL").fetchall() == [
        (f"pace:off:{WEEK_RESET.isoformat()}",)]


def test_one_push_per_new_alert_across_accounts(conn, tmp_path):
    for key in ("a", "b", "codex:c"):
        week_of(conn, key)
    accounts = (Account("claude", "a", "Alpha", tmp_path, True, None, (), ()),
                Account("claude", "b", "Bravo", tmp_path, True, None, (), ()),
                Account("codex", "c", "Charlie", tmp_path, True, None, (), ()))
    transport = FakeTransport()
    found = pace(alerts.evaluate(conn, SETTINGS, NOW, (), [claude("a"), claude("b"), codex("c")]))

    pusher = notify.AlertPusher(transport, lambda: "day", tmp_path / "deferred")
    alerts.sync(conn, found, NOW, pusher, Registry(accounts, implicit=False), evaluated=["a", "b", "codex:c"])
    alerts.sync(conn, found, NOW, pusher, Registry(accounts, implicit=False), evaluated=["a", "b", "codex:c"])

    assert sorted(title for _, title, _ in transport.posts) == [
        "Alpha: Week on pace for 117% at its reset", "Bravo: Week on pace for 117% at its reset",
        "Charlie (Codex): Week on pace for 117% at its reset"]


def test_the_archive_alert_step_evaluates_every_enabled_registry_account(tmp_path):
    home = tmp_path / "isolated-home" / ".config" / "sakura"
    home.mkdir(parents=True)
    entries = [("claude", "a", "true"), ("claude", "b", "true"), ("codex", "c", "true"), ("claude", "off", "false")]
    (home / "accounts.toml").write_text("".join(
        f'[[accounts]]\nprovider = "{provider}"\nid = "{key}"\nlabel = "{key}"\nenabled = {enabled}\n'
        f'config_dir = "{tmp_path / provider / key}"\n' for provider, key, enabled in entries))
    config = archive_settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7)
    conn = store.open_db(config.db)
    for key in ("a", "b", "codex:c", "off"):
        week_of(conn, key)

    command._alert_lines(conn, config, NOW)

    assert conn.execute("SELECT account FROM budget_alerts WHERE kind = 'pace' AND resolved_at IS NULL ORDER BY account").fetchall() == [
        ("a",), ("b",), ("codex:c",)]
