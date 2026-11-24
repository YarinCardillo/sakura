"""Labels are display only: alert titles carry no account, every surface renders the label at read time."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura.accounts import labels, registry, write
from sakura.budget import alerts, weekly, weekly_render
from sakura.budget.settings import BudgetSettings
from sakura.flow import notify
from sakura.usage.archive import quota_report, settings, store

UTC = timezone.utc
NOW = datetime(2026, 10, 28, 14, 0, tzinfo=UTC)
WEEK_RESET = NOW + timedelta(days=3)
START = datetime(2026, 10, 20, 8, 0, tzinfo=UTC)
END = datetime(2026, 10, 27, 8, 0, tzinfo=UTC)
BUDGET = BudgetSettings(account="alpha", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
                        ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00",
                        snapshot_max_age_minutes=30)
ACCOUNTS = """
[[accounts]]
provider = "claude"
id = "alpha"
label = "Studio"

[[accounts]]
provider = "codex"
id = "alpha"
label = "Studio"
"""


class FakeTransport:
    def __init__(self):
        self.posts = []

    def post(self, event, title, message):
        self.posts.append((event, title, message))


BOTH = [alerts.AlertAccount("alpha", "claude"), alerts.AlertAccount("codex:alpha", "codex")]


@pytest.fixture
def home(tmp_path):
    return tmp_path / "home"


@pytest.fixture
def accounts_file(home):
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True)
    path.write_text(ACCOUNTS, encoding="utf-8")
    return path


@pytest.fixture
def names(accounts_file, home):
    return registry.load(env={"HOME": str(home)})


@pytest.fixture
def conn(tmp_path):
    connection = store.open_db(tmp_path / "usage.db")
    yield connection
    connection.close()


def overage_snapshot(conn, account):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES (?, ?, 'max', 'allowed', 10.0, NULL, 50.0, ?, 1)",
                 (account, NOW.isoformat(), WEEK_RESET.isoformat()))
    conn.commit()


def rising_week(conn, account):
    for hours_ago, pct in ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0)):
        conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                     " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                     " VALUES (?, ?, 'max', 'allowed', 10.0, NULL, ?, ?, NULL)",
                     (account, (NOW - timedelta(hours=hours_ago)).isoformat(), pct, WEEK_RESET.isoformat()))
    conn.commit()


def pusher(tmp_path, transport):
    return notify.AlertPusher(transport, lambda: "day", tmp_path / "deferred")


def test_alert_title_has_no_account_name(conn):
    overage_snapshot(conn, "alpha")
    for account in ("alpha", "codex:alpha"):
        rising_week(conn, account)

    found = alerts.evaluate(conn, BUDGET, NOW, (), BOTH)

    assert {alert.account for alert in found} == {"alpha", "codex:alpha"}
    for alert in found:
        assert "alpha" not in alert.title and "(" not in alert.title
    assert {alert.id for alert in found if alert.account == "codex:alpha"} == {
        f"pace:codex:alpha:{WEEK_RESET.isoformat()}"}


def test_pushed_title_names_the_account_by_display_name(conn, tmp_path, names):
    overage_snapshot(conn, "alpha")
    rising_week(conn, "codex:alpha")
    transport = FakeTransport()

    found = alerts.evaluate(conn, BUDGET, NOW, (), BOTH)
    alerts.sync(conn, found, NOW, pusher(tmp_path, transport), names)

    assert sorted(title for _, title, _ in transport.posts) == [
        "Studio (Codex): Week on pace for 117% at its reset", "Studio: Extra usage is being billed"]
    assert {row[0] for row in conn.execute("SELECT title FROM budget_alerts")} == {
        "Extra usage is being billed", "Week on pace for 117% at its reset"}


def test_weekly_page_uses_the_label(conn, tmp_path, names):
    usage = settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7)
    report = weekly.build(conn, usage, START, END, account="alpha")

    page = weekly_render.page(report, 43, names)
    text = weekly_render.text(report, registry=names)

    assert "\naccount: alpha\n" in page
    assert page.count("Studio") == 1 and "Account Studio," in page
    assert text.startswith("account Studio, plan ")


def test_weekly_page_without_a_registry_shows_the_id(conn, tmp_path):
    usage = settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7)
    report = weekly.build(conn, usage, START, END, account="alpha")

    assert "Account alpha," in weekly_render.page(report, 43)


def test_quota_view_shows_the_label_in_the_account_column(names):
    data = {"snapshots": [{"account": "alpha", "ts": "t", "subscription_type": "max", "status": "allowed",
                           "seven_day_pct": 3, "seven_day_resets_at": None, "five_hour_pct": 1,
                           "five_hour_resets_at": None},
                          {"account": "codex:alpha", "ts": "t", "subscription_type": "pro", "status": None,
                           "seven_day_pct": 4, "seven_day_resets_at": None, "five_hour_pct": None,
                           "five_hour_resets_at": None}],
            "plans": [], "ratios": [], "unexplained": [], "limit_events": [], "extra_usage": []}

    rendered = quota_report.render(data, names)

    assert "Studio (Codex)" in rendered and "Studio " in rendered
    assert "alpha" not in rendered
    assert "alpha" in quota_report.render(data)


def test_renamed_label_shows_on_old_alert(conn, tmp_path, accounts_file, home):
    old_title = "Extra usage is being billed (codex:alpha)"
    old = alerts.Alert(f"overage:codex:alpha:{WEEK_RESET.isoformat()}", "codex:alpha", alerts.OVERAGE, old_title,
                       "reason", NOW.isoformat(timespec="seconds"))
    conn.execute("INSERT INTO budget_alerts (id, account, kind, title, reason, since, resolved_at, url, pushed_at)"
                 " VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, NULL)",
                 (old.id, old.account, old.kind, old.title, old.reason, old.since))
    conn.commit()
    write.relabel(accounts_file, "codex", "alpha", "Renamed")
    transport = FakeTransport()

    alerts.sync(conn, [old], NOW, pusher(tmp_path, transport), registry.load(env={"HOME": str(home)}))

    assert [title for _, title, _ in transport.posts] == [f"Renamed (Codex): {old_title}"]
    assert conn.execute("SELECT title FROM budget_alerts").fetchone()[0] == old_title


def test_unknown_id_renders_as_id(names):
    assert labels.label_of("gone", names) == "gone"
    assert labels.label_of("codex:gone", names) == "codex:gone"
    assert labels.display_name("gone", names) == "gone"
    assert labels.display_name("codex:gone", names) == "codex:gone"
    assert labels.label_of("alpha", names) == "Studio"
    assert labels.label_of("codex:alpha", names) == "Studio"
    assert labels.display_name("alpha", names) == "Studio"
    assert labels.display_name("codex:alpha", names) == "Studio (Codex)"


def test_unreadable_registry_falls_back_to_bare_keys(home, accounts_file, capsys):
    accounts_file.write_text("[[accounts]\nbroken", encoding="utf-8")

    names = labels.load_or_empty({"HOME": str(home)})

    assert labels.display_name("alpha", names) == "alpha"
    assert capsys.readouterr() == ("", "")


def test_labels_fall_back_on_non_utf8_accounts_toml(home, accounts_file):
    accounts_file.write_bytes(b'[[accounts]]\nlabel = "caf\xe9"\n')

    names = labels.load_or_empty({"HOME": str(home)})

    assert labels.display_name("alpha", names) == "alpha"
