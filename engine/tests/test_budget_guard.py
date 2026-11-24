"""The live budget guard: reserve floors, pacing to the night, holds, and the snapshot projection it uses."""

from datetime import datetime, timedelta, timezone

import pytest

from sakura.budget import guard
from sakura.budget.settings import BudgetSettings
from sakura.usage import projection
from sakura.usage.archive import store

UTC = timezone.utc
NOW = datetime(2026, 10, 28, 14, 0, tzinfo=UTC)
WEEK_RESET = NOW + timedelta(days=3)
FIVE_RESET = NOW + timedelta(hours=2)
SETTINGS = BudgetSettings(account="a", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
                          ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00",
                          snapshot_max_age_minutes=30)


@pytest.fixture
def conn(tmp_path):
    connection = store.open_db(tmp_path / "usage.db")
    yield connection
    connection.close()


def snapshot(conn, week, five=10.0, minutes_ago=5, week_reset=WEEK_RESET, five_reset=FIVE_RESET, account="a",
             at=None):
    """One quota snapshot `minutes_ago` before NOW (or at `at`)."""
    ts = at or NOW - timedelta(minutes=minutes_ago)
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES (?, ?, 'max', 'allowed', ?, ?, ?, ?)",
                 (account, ts.isoformat(), five, five_reset and five_reset.isoformat(), week,
                  week_reset and week_reset.isoformat()))
    conn.commit()


def check(conn, pending=0.0, urgent=True, now=NOW, settings=SETTINGS, task_key=None, tail=0.0):
    return guard.check("a", now, conn, settings, pending, urgent, task_key, tail)


def test_a_snapshot_with_room_runs(conn):
    snapshot(conn, week=40.0)
    assert check(conn) == guard.Decision("run", None, "")


def test_reserve_pauses_until_weekly_reset(conn):
    snapshot(conn, week=70.0)
    decision = check(conn, pending=5.0)
    assert decision.action == "pause" and decision.until == WEEK_RESET
    assert "75" in decision.reason and "70" in decision.reason


def test_the_pending_estimate_counts_against_the_reserve(conn):
    snapshot(conn, week=60.0)
    assert check(conn, pending=14.0).action == "run"
    assert check(conn, pending=15.0).action == "pause"


def test_five_hour_reserve_defers_to_its_reset(conn):
    snapshot(conn, week=10.0, five=70.0)
    decision = check(conn)
    assert decision.action == "defer" and decision.until == FIVE_RESET
    assert "70" in decision.reason


def test_the_week_reserve_wins_over_the_five_hour_one(conn):
    snapshot(conn, week=80.0, five=90.0)
    assert check(conn).action == "pause"


def test_stale_snapshot_asks(conn):
    snapshot(conn, week=1.0, minutes_ago=31)
    decision = check(conn)
    assert decision.action == "ask" and decision.until is None
    assert "31 min old" in decision.reason


def test_no_snapshot_asks(conn):
    decision = check(conn)
    assert decision.action == "ask" and "no snapshot of a" in decision.reason


def test_a_snapshot_without_a_weekly_figure_asks(conn):
    snapshot(conn, week=None)
    assert check(conn).action == "ask"


def test_a_window_whose_reset_has_passed_counts_as_zero(conn):
    snapshot(conn, week=90.0, five=95.0, week_reset=NOW - timedelta(minutes=1), five_reset=NOW - timedelta(minutes=1))
    assert check(conn).action == "run"


def test_a_missing_five_hour_figure_is_not_invented(conn):
    snapshot(conn, week=10.0, five=None, five_reset=None)
    assert check(conn).action == "run"


def test_a_pause_without_a_known_reset_retries_now(conn):
    snapshot(conn, week=90.0, week_reset=None)
    decision = check(conn)
    assert decision.action == "pause" and decision.until == NOW


def test_holds_count_for_the_second_flow(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "repo:first", "a", 20.0, NOW)
    assert check(conn, pending=10.0, task_key="repo:second").action == "pause"


def test_the_flows_own_hold_does_not_count_against_itself(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "repo:first", "a", 20.0, NOW)
    assert check(conn, pending=10.0, task_key="repo:first").action == "run"


def test_holds_of_another_account_do_not_count(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "repo:other", "work", 40.0, NOW)
    assert check(conn, pending=10.0).action == "run"


def test_release_removes_the_hold_and_is_idempotent(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "repo:first", "a", 20.0, NOW)
    guard.release(conn, "repo:first")
    guard.release(conn, "repo:first")
    assert check(conn, pending=10.0).action == "run"


def test_a_hold_from_before_the_week_began_is_ignored(conn):
    snapshot(conn, week=50.0)  # the week resets in 3 days, so it began 4 days ago
    guard.hold(conn, "/work/repo:old", "a", 20.0, NOW - timedelta(days=5))
    assert check(conn, pending=10.0).action == "run"
    guard.hold(conn, "/work/repo:recent", "a", 20.0, NOW - timedelta(days=1))
    assert check(conn, pending=10.0).action == "pause"


def test_a_hold_from_before_a_reset_that_has_passed_is_ignored(conn):
    reset = NOW - timedelta(hours=2)
    snapshot(conn, week=90.0, week_reset=reset, five_reset=reset)  # the snapshot predates the reset: the week is 0
    guard.hold(conn, "/work/repo:old", "a", 70.0, reset - timedelta(hours=1))
    guard.hold(conn, "/work/repo:new", "a", 10.0, reset + timedelta(hours=1))
    assert check(conn, pending=10.0).action == "run"
    guard.hold(conn, "/work/repo:newer", "a", 60.0, reset + timedelta(hours=1))
    assert check(conn, pending=10.0).action == "pause"


def test_the_pause_reason_names_the_flows_that_hold(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "/work/repo:first", "a", 20.0, NOW)
    decision = check(conn, pending=10.0, task_key="/work/repo:second")
    assert "holds: repo:first 20%" in decision.reason


def test_hold_twice_keeps_one_row_with_the_new_amount(conn):
    guard.hold(conn, "repo:first", "a", 20.0, NOW)
    guard.hold(conn, "repo:first", "a", 12.0, NOW + timedelta(hours=1))
    assert conn.execute("SELECT week_pct, created_at FROM budget_holds").fetchall() == [(12.0, NOW.isoformat())]


# -- an owner release lifts only the rule that stopped the flow ----------------------------------------------------

def test_decisions_name_the_rule_that_made_them(conn):
    snapshot(conn, week=80.0, five=90.0)
    assert check(conn).rule == "week"
    snapshot(conn, week=10.0, five=90.0, minutes_ago=1)
    assert check(conn).rule == "five_hour"
    assert check(conn, now=NOW + timedelta(minutes=40)).rule == "staleness"


def test_a_lifted_week_rule_leaves_the_five_hour_rule_in_force(conn):
    snapshot(conn, week=80.0, five=90.0)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "week").rule == "five_hour"
    snapshot(conn, week=80.0, five=10.0, minutes_ago=1)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "week").action == "run"


def test_a_lifted_five_hour_rule_leaves_the_week_rule_in_force(conn):
    snapshot(conn, week=80.0, five=90.0)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "five_hour").rule == "week"


def test_a_lifted_staleness_rule_judges_the_old_snapshot_by_the_other_rules(conn):
    snapshot(conn, week=80.0, minutes_ago=45)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "staleness").rule == "week"
    snapshot(conn, week=10.0, minutes_ago=44)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "staleness").action == "run"


def test_a_lifted_staleness_rule_does_not_invent_a_missing_snapshot(conn):
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True, None, 0.0, "staleness").action == "ask"


def test_a_lifted_pacing_rule_runs_a_when_cheap_flow_by_day(conn):
    rising_week(conn, start=60.0, per_hour=2.0)
    assert check(conn, urgent=False).rule == "pacing"
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, False, None, 0.0, "pacing").action == "run"


def test_a_lifted_rule_applies_to_a_codex_lower_bound_too(conn):
    snapshot(conn, week=80.0, five=90.0, account="codex:a")
    assert guard.check_lower_bound("codex:a", NOW, conn, SETTINGS, True, "week").rule == "five_hour"


# -- the flow's own share shrinks as it spends -------------------------------------------------------------------

def test_a_flow_that_spent_part_of_its_estimate_keeps_running(conn):
    snapshot(conn, week=50.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW, week_at_hold=50.0)
    snapshot(conn, week=56.0, minutes_ago=1)  # it spent 6 of its 20 points: 14 are still to come, 56 + 14 < 75
    assert check(conn, pending=20.0, task_key="repo:own").action == "run"


def test_a_flow_without_a_baseline_counts_its_whole_estimate(conn):
    snapshot(conn, week=56.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW)
    assert check(conn, pending=20.0, task_key="repo:own").action == "pause"


def test_the_spend_is_counted_from_the_week_figure_at_the_hold(conn):
    snapshot(conn, week=70.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW, week_at_hold=60.0)  # 10 spent, 10 to come: 70 + 10 reaches 75
    assert check(conn, pending=20.0, task_key="repo:own").action == "pause"
    guard.release(conn, "repo:own")
    guard.hold(conn, "repo:other", "a", 20.0, NOW, week_at_hold=50.0)  # 20 spent, nothing to come: 70 < 75
    assert check(conn, pending=20.0, task_key="repo:other").action == "run"


def test_what_the_flow_has_left_never_drops_below_its_post_build_tail(conn):
    snapshot(conn, week=72.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW, week_at_hold=50.0)  # it spent 22 of 20: nothing left but the tail
    assert check(conn, pending=20.0, task_key="repo:own").action == "run"
    assert check(conn, pending=20.0, task_key="repo:own", tail=4.0).action == "pause"


def test_a_baseline_from_before_a_reset_counts_no_spend(conn):
    snapshot(conn, week=3.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW, week_at_hold=90.0)
    assert check(conn, pending=20.0, task_key="repo:own").action == "run"


def test_holding_again_after_a_weekly_reset_refreshes_the_date_and_the_baseline(conn):
    week_start = NOW - timedelta(days=4)
    guard.hold(conn, "repo:own", "a", 20.0, NOW - timedelta(days=5), week_at_hold=80.0)
    guard.hold(conn, "repo:own", "a", 20.0, NOW, week_at_hold=2.0, week_start=week_start)
    assert conn.execute("SELECT created_at, week_at_hold FROM budget_holds").fetchall() == [(NOW.isoformat(), 2.0)]


def test_holding_again_in_the_same_week_keeps_the_date_and_the_baseline(conn):
    week_start = NOW - timedelta(days=4)
    guard.hold(conn, "repo:own", "a", 20.0, NOW - timedelta(days=1), week_at_hold=40.0)
    guard.hold(conn, "repo:own", "a", 18.0, NOW, week_at_hold=55.0, week_start=week_start)
    assert conn.execute("SELECT week_pct, created_at, week_at_hold FROM budget_holds").fetchall() == [
        (18.0, (NOW - timedelta(days=1)).isoformat(), 40.0)]


def test_week_position_is_the_live_figure_and_the_start_of_its_window(conn):
    assert guard.week_position(conn, "a", NOW) == (None, None)
    snapshot(conn, week=33.0)
    assert guard.week_position(conn, "a", NOW) == (33.0, WEEK_RESET - timedelta(days=7))


# -- pacing of a flow started with --when-cheap ------------------------------------------------------------------

def rising_week(conn, start=20.0, per_hour=1.0, hours=6):
    """Snapshots every hour for `hours` hours, rising `per_hour` points an hour, the last one 5 minutes old."""
    for step in range(hours):
        at = NOW - timedelta(minutes=5, hours=hours - 1 - step)
        snapshot(conn, week=start + per_hour * step, at=at)


def test_when_cheap_flow_defers_to_night_when_pace_is_high(conn):
    rising_week(conn, start=20.0, per_hour=1.0)  # 25 now, 1 point an hour, 72 hours to the reset: 97 projected
    decision = check(conn, urgent=False)
    assert decision.action == "defer"
    assert decision.until == datetime(2026, 10, 28, 23, 0, tzinfo=UTC)
    assert "projected" in decision.reason


def test_an_urgent_flow_ignores_the_pace(conn):
    rising_week(conn, start=20.0, per_hour=1.0)
    assert check(conn, urgent=True).action == "run"


def test_a_when_cheap_flow_runs_when_the_pace_is_low(conn):
    rising_week(conn, start=20.0, per_hour=0.1)
    assert check(conn, urgent=False).action == "run"


def test_a_when_cheap_flow_runs_inside_the_night_window(conn):
    night = datetime(2026, 10, 28, 23, 30, tzinfo=UTC)
    rising_week(conn, start=20.0, per_hour=1.0)
    snapshot(conn, week=26.0, at=night - timedelta(minutes=5))
    assert check(conn, urgent=False, now=night).action == "run"


def test_the_next_night_after_midnight_starts_that_evening(conn):
    after_midnight = datetime(2026, 10, 29, 8, 0, tzinfo=UTC)
    assert guard.next_night_start(after_midnight, "23:00-07:00") == datetime(2026, 10, 29, 23, 0, tzinfo=UTC)
    assert guard.in_night(datetime(2026, 10, 29, 6, 59, tzinfo=UTC), "23:00-07:00")
    assert not guard.in_night(after_midnight, "23:00-07:00")


def test_a_when_cheap_flow_without_a_trend_runs(conn):
    snapshot(conn, week=40.0)
    assert check(conn, urgent=False).action == "run"


# -- the snapshot-based weekly projection (shared with the pace alert) ------------------------------------------

def test_project_week_extrapolates_the_trend_to_the_reset(conn):
    rising_week(conn, start=20.0, per_hour=1.0)
    projected = projection.project_week(conn, "a", NOW)
    assert projected == pytest.approx(25.0 + 72 - 5 / 60, abs=1.0)


def test_project_week_needs_two_snapshots(conn):
    snapshot(conn, week=40.0)
    assert projection.project_week(conn, "a", NOW) is None


def test_project_week_ignores_snapshots_of_a_previous_week(conn):
    old_reset = NOW - timedelta(hours=1)
    for step in range(3):
        snapshot(conn, week=80.0 + step, week_reset=old_reset, at=NOW - timedelta(hours=10 - step))
    snapshot(conn, week=2.0, minutes_ago=5)
    assert projection.project_week(conn, "a", NOW) is None


def test_project_week_ignores_other_accounts(conn):
    rising_week(conn, start=20.0, per_hour=1.0)
    assert projection.project_week(conn, "work", NOW) is None


def test_the_archive_has_a_budget_holds_table(conn):
    columns = {row[1]: (row[2], row[3], row[5]) for row in conn.execute("PRAGMA table_info(budget_holds)")}
    assert columns == {"task_key": ("TEXT", 0, 1), "account": ("TEXT", 1, 0), "week_pct": ("REAL", 1, 0),
                       "created_at": ("TEXT", 1, 0), "week_at_hold": ("REAL", 0, 0)}


# -- Codex: the snapshot as a lower bound ------------------------------------------------------------------------

def codex_check(conn, urgent=True, now=NOW):
    return guard.check_lower_bound("codex:a", now, conn, SETTINGS, urgent)


def test_codex_lower_bound_pauses_at_the_reserve(conn):
    snapshot(conn, week=80.0, account="codex:a")
    decision = codex_check(conn)
    assert decision.action == "pause" and decision.until == WEEK_RESET and "lower bound" in decision.reason


def test_codex_lower_bound_of_a_stale_snapshot_still_counts(conn):
    snapshot(conn, week=80.0, minutes_ago=600, account="codex:a")
    assert codex_check(conn).action == "pause"
    assert check(conn).action == "ask", "the Claude check on the same age stays unknown"


def test_codex_lower_bound_runs_when_the_stale_figure_leaves_room(conn):
    snapshot(conn, week=40.0, minutes_ago=600, account="codex:a")
    assert codex_check(conn) == guard.Decision("run", None, "")


def test_codex_window_after_its_reset_counts_as_zero(conn):
    snapshot(conn, week=95.0, minutes_ago=3000, week_reset=NOW - timedelta(days=1), account="codex:a")
    assert codex_check(conn).action == "run"


def test_codex_null_windows_skip_their_checks_and_no_snapshot_runs(conn):
    assert codex_check(conn).action == "run"
    snapshot(conn, week=None, five=None, five_reset=None, minutes_ago=600, account="codex:a")
    assert codex_check(conn).action == "run"


def test_codex_five_hour_window_defers_until_its_reset(conn):
    snapshot(conn, week=10.0, five=75.0, account="codex:a")
    decision = codex_check(conn)
    assert decision.action == "defer" and decision.until == FIVE_RESET


# -- the daily pace is retired: pace is where an account lands at its reset (`flow/pace.py`) ---------------


def test_the_guard_has_no_daily_pace_any_more(conn):
    snapshot(conn, week=30.0, at=NOW.replace(hour=0, minute=0) - timedelta(minutes=10))
    snapshot(conn, week=45.0)
    assert guard.check("a", NOW, conn, SETTINGS, 0.0, True).action == "run"
    assert not hasattr(guard, "day_gain") and not hasattr(guard, "DAY_PACE_RULE")
    assert guard.PACE_RULE == "pace"
