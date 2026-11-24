"""Early warnings: pace, outside and overage alerts of the archive run, the request alerts of the flows, the pushes."""

import json
from datetime import datetime, timedelta, timezone

import pytest

from sakura.budget import alerts
from sakura.budget.settings import BudgetSettings
from sakura.flow import notify
from sakura.usage.archive import store

UTC = timezone.utc
NOW = datetime(2026, 10, 28, 14, 0, tzinfo=UTC)
WEEK_RESET = NOW + timedelta(days=3)
SETTINGS = BudgetSettings(account="a", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
                          ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00",
                          snapshot_max_age_minutes=30)


ACCOUNTS = [alerts.AlertAccount("a", "claude")]
WITH_CODEX = [*ACCOUNTS, alerts.AlertAccount("codex:a", "codex")]


@pytest.fixture
def conn(tmp_path):
    connection = store.open_db(tmp_path / "usage.db")
    yield connection
    connection.close()


class FakeTransport:
    """Records what would have gone to ntfy."""

    def __init__(self):
        self.posts = []

    def post(self, event, task, message):
        self.posts.append((event, task, message))


def pusher(tmp_path, mode="day", transport=None):
    return notify.AlertPusher(transport, lambda: mode, tmp_path / "deferred")


def snapshot(conn, week, at=NOW, reset=WEEK_RESET, overage=None, plan="max"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES ('a', ?, ?, 'allowed', 10.0, NULL, ?, ?, ?)", (at.isoformat(), plan, week, reset.isoformat(), overage))
    conn.commit()


def rising_week(conn):
    """Four hourly snapshots climbing one point an hour to 44%: about 117% by the reset three days away."""
    for hours_ago, pct in ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0)):
        snapshot(conn, pct, NOW - timedelta(hours=hours_ago))


def flat_week(conn):
    conn.execute("DELETE FROM quota_snapshots")
    for hours_ago in (3, 2, 1, 0.05):
        snapshot(conn, 44.0, NOW - timedelta(hours=hours_ago))


def only(kind, found):
    """The alerts of one kind (a steep test snapshot also projects the week over the ceiling)."""
    return [alert for alert in found if alert.kind == kind]


def rows(conn):
    return conn.execute("SELECT id, kind, resolved_at FROM budget_alerts ORDER BY id").fetchall()


def test_the_alerts_table_has_the_columns_notes_reads(conn):
    columns = conn.execute("PRAGMA table_info(budget_alerts)").fetchall()
    assert [(c[1], c[2], c[3], c[5]) for c in columns] == [
        ("id", "TEXT", 0, 1), ("account", "TEXT", 1, 0), ("kind", "TEXT", 1, 0), ("title", "TEXT", 1, 0),
        ("reason", "TEXT", 1, 0), ("since", "TEXT", 1, 0), ("resolved_at", "TEXT", 0, 0), ("url", "TEXT", 0, 0),
        ("pushed_at", "TEXT", 0, 0)]


def test_pace_alert_opens_once_and_resolves(conn, tmp_path):
    rising_week(conn)
    transport = FakeTransport()
    page = pusher(tmp_path, transport=transport)

    first = alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, page)
    again = alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, page)

    assert [a.kind for a in first.opened] == ["pace"] and again.opened == ()
    assert len(transport.posts) == 1
    assert rows(conn) == [(f"pace:a:{WEEK_RESET.isoformat()}", "pace", None)]
    flat_week(conn)
    later = NOW + timedelta(minutes=15)
    done = alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, later, (), ACCOUNTS)), later, page)
    assert done.resolved == (f"pace:a:{WEEK_RESET.isoformat()}",) and len(transport.posts) == 1
    assert rows(conn)[0][2] == later.isoformat(timespec="seconds")


def test_pace_alert_carries_the_numbers_and_their_basis(conn):
    rising_week(conn)
    (alert,) = only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))
    assert "117%" in alert.title
    assert "44%" in alert.reason and "117%" in alert.reason and "75%" in alert.reason and "25%" in alert.reason
    assert "snapshot trend" in alert.reason and "no token calibration" in alert.reason and "44-117%" in alert.reason


def test_pace_fires_on_the_snapshot_trend_not_on_local_tokens(conn):
    """Review Focus 3: no local turn exists at all, the week still climbs (use outside Claude Code)."""
    rising_week(conn)
    assert conn.execute("SELECT COUNT(*) FROM turns").fetchone() == (0,)
    assert [a.kind for a in only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))] == ["pace"]


def test_an_unknown_projection_leaves_an_open_pace_alert_open(conn, tmp_path):
    """Probe failing: the newest snapshot is over 6 h old, so nothing is known, which is never 'fine'."""
    rising_week(conn)
    page = pusher(tmp_path, transport=FakeTransport())
    alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, page)
    stale = NOW + timedelta(hours=7)

    found = only("pace", alerts.evaluate(conn, SETTINGS, stale, (), ACCOUNTS))
    result = alerts.sync(conn, found, stale, page)

    assert [a.kind for a in found] == ["pace"] and result.resolved == () and result.opened == ()
    assert rows(conn) == [(f"pace:a:{WEEK_RESET.isoformat()}", "pace", None)]


def test_an_open_pace_alert_is_resolved_once_its_week_is_over_even_if_unknown(conn, tmp_path):
    rising_week(conn)
    page = pusher(tmp_path, transport=FakeTransport())
    alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, page)
    after = WEEK_RESET + timedelta(hours=1)

    result = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, after, (), ACCOUNTS), after, page)

    assert result.resolved == (f"pace:a:{WEEK_RESET.isoformat()}",)


def test_a_week_on_pace_below_the_ceiling_has_no_alert(conn):
    for hours_ago, pct in ((3, 10.0), (2, 10.2), (1, 10.4), (0.05, 10.6)):
        snapshot(conn, pct, NOW - timedelta(hours=hours_ago))
    assert alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS) == []


def test_a_pace_alert_of_a_later_window_is_a_new_id_and_pushes_again(conn, tmp_path):
    transport = FakeTransport()
    page = pusher(tmp_path, transport=transport)
    rising_week(conn)
    alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, page)
    conn.execute("DELETE FROM quota_snapshots")
    later, next_reset = NOW + timedelta(days=4), WEEK_RESET + timedelta(days=7)
    for hours_ago, pct in ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0)):
        snapshot(conn, pct, later - timedelta(hours=hours_ago), next_reset)

    result = alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, later, (), ACCOUNTS)), later, page)

    assert [a.id for a in result.opened] == [f"pace:a:{next_reset.isoformat()}"] and len(transport.posts) == 2
    assert [(row[0][:4], row[2] is None) for row in rows(conn)] == [("pace", False), ("pace", True)]


def test_overage_alert(conn, tmp_path):
    snapshot(conn, 60.0, NOW - timedelta(minutes=20), overage=0)
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    transport = FakeTransport()

    (alert,) = only("overage", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))
    alerts.sync(conn, [alert], NOW, pusher(tmp_path, transport=transport))

    assert alert.kind == "overage" and alert.id == f"overage:a:{WEEK_RESET.isoformat()}"
    assert "extra usage" in alert.title.lower() and "61%" in alert.reason
    assert len(transport.posts) == 1
    snapshot(conn, 62.0, NOW + timedelta(minutes=10), overage=0)
    assert only("overage", alerts.evaluate(conn, SETTINGS, NOW + timedelta(minutes=15), (), ACCOUNTS)) == []


def test_overage_alert_without_a_weekly_percentage_still_opens(conn):
    """A probe that reported only the 5-hour window and extra usage in use: the figure is not invented."""
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES ('a', ?, 'max', 'allowed', 10.0, NULL, NULL, NULL, 1)", ((NOW - timedelta(minutes=5)).isoformat(),))

    (alert,) = only("overage", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))

    assert alert.id == "overage:a:-" and "no weekly figure" in alert.reason and "None" not in alert.reason


def test_a_snapshot_that_does_not_say_leaves_the_overage_alert_as_it_was(conn):
    snapshot(conn, 61.0, NOW - timedelta(minutes=20), overage=1)
    snapshot(conn, 62.0, NOW - timedelta(minutes=5), overage=None)
    assert [a.kind for a in only("overage", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))] == ["overage"]


def test_no_push_for_an_unchanged_alert(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    transport = FakeTransport()
    page = pusher(tmp_path, transport=transport)
    for minutes in (0, 15, 30):
        moment = NOW + timedelta(minutes=minutes)
        alerts.sync(conn, alerts.evaluate(conn, SETTINGS, moment, (), ACCOUNTS), moment, page)
    assert len(transport.posts) == 1 and len(rows(conn)) == 1
    assert conn.execute("SELECT since FROM budget_alerts").fetchone() == (NOW.isoformat(timespec="seconds"),)


def test_an_alert_that_reopens_in_its_window_is_not_pushed_twice(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    transport = FakeTransport()
    page = pusher(tmp_path, transport=transport)
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, page)
    alerts.sync(conn, [], NOW + timedelta(minutes=15), page)
    result = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW + timedelta(minutes=30), page)
    assert result.opened == () and len(transport.posts) == 1 and rows(conn)[0][2] is None


def test_night_mode_queues_the_push(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    transport = FakeTransport()

    result = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "night", transport))

    assert transport.posts == [] and "deferred" in result.pushes[0]
    (entry,) = [json.loads(path.read_text()) for path in (tmp_path / "deferred").glob("*.json")]
    assert entry["event"] == "budget_alert" and "Extra usage" in entry["task"] and "61%" in entry["message"]


def test_the_day_push_goes_to_the_transport_with_the_title_and_the_numbers(conn, tmp_path):
    rising_week(conn)
    transport = FakeTransport()
    result = alerts.sync(conn, only("pace", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, pusher(tmp_path, transport=transport))
    ((event, title, message),) = transport.posts
    assert event == "budget_alert" and "117%" in title and "75%" in message and "pushed" in result.pushes[0]


def test_without_an_ntfy_topic_the_alert_is_written_and_the_push_is_skipped(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    result = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "day", None))
    assert len(rows(conn)) == 1 and "skipped" in result.pushes[0] and "no ntfy topic" in result.pushes[0]
    assert not (tmp_path / "deferred").exists()


def test_a_failing_transport_is_reported_and_never_raises(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)

    class Down:
        def post(self, event, task, message):
            raise OSError("topic-secret-123 unreachable")

    result = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "day", Down()))
    assert result.pushes == ("failed",)


def calibrated_outside_day(conn):
    """Oct 27: four 2% rises explained by 1000 weighted tokens each (500 per point); Oct 28: +12% on 1000 tokens."""
    base = datetime(2026, 10, 27, 8, 0, tzinfo=UTC)
    for step in range(5):
        snapshot(conn, 10.0 + 2 * step, base + timedelta(hours=step))
        if step < 4:
            conn.execute("INSERT INTO turns VALUES ('claude', ?, 'a', 's', NULL, ?, NULL, NULL, 'claude-opus-5-5',"
                         " 0, 0, 0, 200, NULL)", (f"k{step}", (base + timedelta(hours=step, minutes=5)).isoformat()))
    snapshot(conn, 18.0, datetime(2026, 10, 28, 13, 0, tzinfo=UTC))
    conn.execute("INSERT INTO turns VALUES ('claude', 'today', 'a', 's', NULL, ?, NULL, NULL, 'claude-opus-5-5',"
                 " 0, 0, 0, 200, NULL)", (datetime(2026, 10, 28, 13, 40, tzinfo=UTC).isoformat(),))
    snapshot(conn, 30.0, datetime(2026, 10, 28, 13, 50, tzinfo=UTC))


def test_outside_share_alert_reports_its_cost(conn):
    calibrated_outside_day(conn)

    (alert,) = only("outside", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))

    assert (alert.kind, alert.id) == ("outside", "outside:a:2026-10-28")
    assert "8%" in alert.title and "2026-10-28" in alert.title
    for text in ("rose 12%", "explain 4%", "8% (67%)", "40%", "calibrated", "7-9%"):
        assert text in alert.reason


def test_outside_share_below_the_setting_has_no_alert(conn):
    calibrated_outside_day(conn)
    lenient = BudgetSettings(**{**SETTINGS.__dict__, "outside_share_alert": 0.8})
    assert only("outside", alerts.evaluate(conn, lenient, NOW, (), ACCOUNTS)) == []


def test_a_gap_is_named_in_the_outside_alert(conn):
    """The rise across a 10 hour hole is unattributed; the next interval is not explained, and says so."""
    base = datetime(2026, 10, 28, 1, 0, tzinfo=UTC)
    snapshot(conn, 10.0, base)
    snapshot(conn, 20.0, base + timedelta(hours=10))
    snapshot(conn, 25.0, base + timedelta(hours=10, minutes=10))

    (alert,) = only("outside", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS))

    assert "5%" in alert.title and "unattributed" in alert.reason and "10%" in alert.reason


def test_a_small_outside_rise_is_below_the_noise_floor(conn):
    snapshot(conn, 10.0, datetime(2026, 10, 28, 12, 0, tzinfo=UTC))
    snapshot(conn, 11.0, datetime(2026, 10, 28, 12, 10, tzinfo=UTC))
    assert only("outside", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)) == []


def test_an_outside_alert_resolves_when_its_day_leaves_the_lookback(conn, tmp_path):
    calibrated_outside_day(conn)
    alerts.sync(conn, only("outside", alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)), NOW, pusher(tmp_path, transport=FakeTransport()))
    later = NOW + timedelta(days=2)
    result = alerts.sync(conn, only("outside", alerts.evaluate(conn, SETTINGS, later, (), ACCOUNTS)), later, pusher(tmp_path))
    assert result.resolved == ("outside:a:2026-10-28",)


def test_sync_leaves_request_alerts_alone(conn, tmp_path):
    alerts.open_request(conn, "a", "repo", "demo", "demo asks: about 12%", "about 12% of your week", NOW)
    result = alerts.sync(conn, [], NOW, pusher(tmp_path))
    assert result.resolved == () and rows(conn)[0][2] is None


def test_a_request_alert_opens_per_flow_and_window_and_updates_its_numbers(conn):
    snapshot(conn, 30.0)
    alerts.open_request(conn, "a", "repo", "demo", "demo asks", "about 12% of your week", NOW)
    alerts.open_request(conn, "a", "repo", "demo", "demo paused", "week 80% reaches 75%", NOW + timedelta(hours=1))
    ((identifier, kind, resolved),) = rows(conn)
    assert identifier == f"request:a:repo:demo:{WEEK_RESET.isoformat()}"
    assert kind == "request" and resolved is None
    assert conn.execute("SELECT title, reason, since FROM budget_alerts").fetchone() == (
        "demo paused", "week 80% reaches 75%", NOW.isoformat(timespec="seconds"))


def test_resolving_requests_closes_the_flows_alerts_in_any_window(conn):
    snapshot(conn, 30.0)
    alerts.open_request(conn, "a", "repo", "demo", "t", "r", NOW)
    alerts.open_request(conn, "a", "repo", "other", "t", "r", NOW)
    snapshot(conn, 5.0, NOW + timedelta(days=4), WEEK_RESET + timedelta(days=7))

    closed = alerts.resolve_requests(conn, "a", "repo", "demo", NOW + timedelta(days=4))

    assert closed == 1
    assert [(row[0].split(":")[3], row[2] is None) for row in rows(conn)] == [("demo", False), ("other", True)]


def test_resolving_does_not_touch_a_flow_whose_name_starts_the_same(conn):
    alerts.open_request(conn, "a", "repo", "demo", "t", "r", NOW)
    alerts.open_request(conn, "a", "repo", "demo-2", "t", "r", NOW)
    assert alerts.resolve_requests(conn, "a", "repo", "demo", NOW) == 1


def codex_snapshot(conn, week, at, account="codex:a"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES (?, ?, 'prolite', 'allowed', NULL, NULL, ?, ?, NULL)",
                 (account, at.isoformat(), week, WEEK_RESET.isoformat()))
    conn.commit()


def rising_codex_week(conn, now=NOW):
    for hours_ago, pct in ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0)):
        codex_snapshot(conn, pct, now - timedelta(hours=hours_ago))


def test_codex_pace_alert_has_its_own_id(conn):
    rising_codex_week(conn)

    found = alerts.evaluate(conn, SETTINGS, NOW, (), WITH_CODEX)

    assert [(alert.id, alert.account, alert.kind) for alert in found] == [
        (f"pace:codex:a:{WEEK_RESET.isoformat()}", "codex:a", "pace")]
    assert "codex:a" not in found[0].title


def test_codex_alerts_are_not_evaluated_without_a_mapped_account(conn):
    rising_codex_week(conn)
    assert alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS) == []


def test_a_stale_codex_projection_keeps_its_open_pace_alert_and_opens_none(conn):
    rising_codex_week(conn)
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), WITH_CODEX), NOW)
    later = NOW + timedelta(hours=8)

    again = alerts.evaluate(conn, SETTINGS, later, (), WITH_CODEX)
    result = alerts.sync(conn, again, later)

    assert [alert.kind for alert in again] == ["pace"] and result.resolved == () and result.opened == ()


def test_a_codex_account_is_never_evaluated_for_overage(conn):
    codex_snapshot(conn, 50.0, NOW)
    conn.execute("UPDATE quota_snapshots SET overage_in_use = 1")
    conn.commit()

    assert alerts.evaluate(conn, SETTINGS, NOW, (), WITH_CODEX) == []


# -- a push that did not go out is retried -------------------------------------------------------------------------

class Down:
    def post(self, event, task, message):
        raise OSError("unreachable")


def pushed_at(conn):
    return conn.execute("SELECT pushed_at FROM budget_alerts ORDER BY id").fetchall()


def test_a_failed_push_is_retried_on_the_next_run_until_it_goes_out(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    found = alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS)
    first = alerts.sync(conn, found, NOW, pusher(tmp_path, "day", Down()))
    assert first.pushes == ("failed",) and pushed_at(conn) == [(None,)]

    later = NOW + timedelta(minutes=15)
    transport = FakeTransport()
    second = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, later, (), ACCOUNTS), later, pusher(tmp_path, "day", transport))

    assert second.opened == () and [a.kind for a in second.attempted] == ["overage"] and "pushed" in second.pushes[0]
    assert len(transport.posts) == 1 and pushed_at(conn) == [(later.isoformat(timespec="seconds"),)]
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, later, (), ACCOUNTS), later, pusher(tmp_path, "day", transport))
    assert len(transport.posts) == 1, "pushed once it went out"


def test_a_push_deferred_by_night_mode_counts_as_pushed(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "night", FakeTransport()))
    assert pushed_at(conn)[0][0] is not None
    again = alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "night", FakeTransport()))
    assert again.attempted == () and len(list((tmp_path / "deferred").glob("*.json"))) == 1


def test_an_alert_waits_for_an_ntfy_topic_and_goes_out_when_one_is_configured(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "day", None))
    assert pushed_at(conn) == [(None,)]
    transport = FakeTransport()
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "day", transport))
    assert len(transport.posts) == 1 and pushed_at(conn)[0][0] is not None


def test_a_resolved_alert_that_was_never_pushed_is_not_retried(conn, tmp_path):
    snapshot(conn, 61.0, NOW - timedelta(minutes=5), overage=1)
    alerts.sync(conn, alerts.evaluate(conn, SETTINGS, NOW, (), ACCOUNTS), NOW, pusher(tmp_path, "day", Down()))
    alerts.sync(conn, [], NOW + timedelta(minutes=15), pusher(tmp_path, "day", FakeTransport()))
    transport = FakeTransport()
    alerts.sync(conn, [], NOW + timedelta(minutes=30), pusher(tmp_path, "day", transport))
    assert transport.posts == []


def test_a_request_alert_is_marked_pushed_when_written_because_the_flow_notifier_pushes_it(conn, tmp_path):
    alerts.open_request(conn, "a", "repo", "demo", "demo asks", "about 12% of your week", NOW)
    assert pushed_at(conn) == [(NOW.isoformat(timespec="seconds"),)]
    transport = FakeTransport()
    result = alerts.sync(conn, [], NOW + timedelta(minutes=15), pusher(tmp_path, "day", transport))
    assert result.attempted == () and transport.posts == []
    alerts.open_request(conn, "a", "repo", "demo", "demo paused", "week 80%", NOW + timedelta(hours=1))
    assert pushed_at(conn) == [(NOW.isoformat(timespec="seconds"),)], "an update does not move the push time"
