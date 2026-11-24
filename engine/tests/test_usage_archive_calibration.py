from datetime import date, datetime, timedelta, timezone

import pytest

from sakura.usage.archive import calibration, settings, store
from sakura.usage.archive.calibration import OutsideShare, Ratio

START = datetime(2026, 10, 20, 8, 0, tzinfo=timezone.utc)
RESET_A, RESET_B = "2026-10-27T08:00:00Z", "2026-11-03T08:00:00Z"
FIVE_A, FIVE_B = "2026-10-20T13:00:00Z", "2026-10-20T18:00:00Z"
DAY = date(2026, 10, 20)
PLANS = (settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc)),)


def at(minutes):
    return START + timedelta(minutes=minutes)


@pytest.fixture
def conn(tmp_path):
    return store.open_db(tmp_path / "usage.db")


def snapshot(conn, minutes, pct, resets=RESET_A, account="a"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at)"
                 " VALUES (?, ?, 'max', 'allowed', 1.0, NULL, ?, ?)", (account, at(minutes).isoformat(), pct, resets))


def five_hour_snapshot(conn, minutes, pct, resets=FIVE_A, account="a"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at)"
                 " VALUES (?, ?, 'max', 'allowed', ?, ?, 50.0, ?)", (account, at(minutes).isoformat(), pct, resets, RESET_A))


def turn(conn, minutes, output, model="claude-opus-5-5", account="a", provider="claude"):
    key = f"{account}-{minutes}-{model}-{output}"
    conn.execute("INSERT INTO turns VALUES (?, ?, ?, 's', NULL, ?, NULL, NULL, ?, 0, 0, 0, ?, NULL)",
                 (provider, key, account, at(minutes).isoformat(), model, output))


def ratios(conn, plans=PLANS):
    return calibration.compute(conn, plans)


def test_model_class_names():
    assert calibration.model_class("claude-opus-5-5") == "opus"
    assert calibration.model_class("claude-sonnet-4-5-20250929") == "sonnet"
    assert calibration.model_class("claude-haiku-4-5") == "haiku"
    assert [calibration.model_class(value) for value in (None, "", "gpt-5", "<synthetic>", "claude-3-5")] == ["other"] * 5


def test_weighted_tokens_uses_the_price_ratios():
    assert calibration.weighted_tokens(10, 10, 100, 2) == 10 + 20 + 10 + 10


def test_compute_ratio_from_a_pure_interval(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    snapshot(conn, 10, 12.0)

    assert ratios(conn) == [Ratio("a", "max20x", "opus", 500.0, 1, 2.0, "seven_day", (500.0, 500.0))]


def test_turns_outside_the_interval_account_or_provider_are_ignored(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 0, 999)
    turn(conn, 5, 200)
    turn(conn, 5, 999, account="b")
    turn(conn, 5, 999, provider="codex")
    turn(conn, 11, 999)
    snapshot(conn, 10, 12.0)

    assert ratios(conn) == [Ratio("a", "max20x", "opus", 500.0, 1, 2.0, "seven_day", (500.0, 500.0))]


def test_zero_change_pairs_accumulate_into_the_next_rise(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 100)
    snapshot(conn, 10, 10.0)
    turn(conn, 15, 100)
    snapshot(conn, 20, 11.0)

    assert ratios(conn) == [Ratio("a", "max20x", "opus", 1000.0, 1, 1.0, "seven_day", (1000.0, 1000.0))]


def test_weekly_reset_starts_over(conn):
    snapshot(conn, 0, 90.0)
    turn(conn, 5, 999)
    snapshot(conn, 10, 5.0, RESET_B)
    turn(conn, 15, 200)
    snapshot(conn, 20, 7.0, RESET_B)

    assert ratios(conn) == [Ratio("a", "max20x", "opus", 500.0, 1, 2.0, "seven_day", (500.0, 500.0))]


def test_new_reset_time_with_a_higher_percentage_is_not_a_pair(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 999)
    snapshot(conn, 10, 20.0, RESET_B)

    assert ratios(conn) == []


def test_unexplained_rise_is_skipped_and_counted(conn):
    snapshot(conn, 0, 10.0)
    snapshot(conn, 10, 12.0)
    turn(conn, 15, 200)
    snapshot(conn, 20, 14.0)

    analysis = calibration.analyze(conn, PLANS)

    assert analysis.ratios == [Ratio("a", "max20x", "opus", 500.0, 1, 2.0, "seven_day", (500.0, 500.0))]
    assert dict(analysis.unexplained) == {("a", "max20x"): 1}


def test_mixed_interval_goes_to_mixed(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 120)
    turn(conn, 6, 80, model="claude-sonnet-4-5")
    snapshot(conn, 10, 12.0)
    turn(conn, 15, 160)
    turn(conn, 16, 40, model="claude-sonnet-4-5")
    snapshot(conn, 20, 14.0)

    assert ratios(conn) == [Ratio("a", "max20x", "mixed", 500.0, 1, 2.0, "seven_day", (500.0, 500.0)),
                            Ratio("a", "max20x", "opus", 500.0, 1, 2.0, "seven_day", (500.0, 500.0))]


def test_interval_plan_comes_from_the_period_at_its_start(conn):
    plans = (settings.Plan("a", "max20x", START - timedelta(days=9)),
             settings.Plan("a", "max5x", START + timedelta(minutes=30)))
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    snapshot(conn, 10, 12.0)
    turn(conn, 15, 400)
    snapshot(conn, 35, 13.0)
    turn(conn, 40, 100)
    snapshot(conn, 50, 14.0)

    assert [(r.plan, r.weighted_per_pct) for r in ratios(conn, plans)] == [("max20x", 1000.0), ("max5x", 500.0)]


def test_ratios_of_two_intervals_sum_tokens_over_sum_rise(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    snapshot(conn, 10, 12.0)
    turn(conn, 15, 400)
    snapshot(conn, 20, 13.0)

    assert ratios(conn) == [Ratio("a", "max20x", "opus", 1000.0, 2, 3.0, "seven_day", (800.0, 1700.0))]


def test_five_hour_ratio_from_intervals(conn):
    five_hour_snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    five_hour_snapshot(conn, 10, 12.0)
    five_hour_snapshot(conn, 400, 3.0, FIVE_B)
    turn(conn, 405, 100)
    five_hour_snapshot(conn, 410, 4.0, FIVE_B)

    result = calibration.compute(conn, PLANS, window="five_hour")

    assert result == [Ratio("a", "max20x", "opus", 500.0, 2, 3.0, "five_hour", (500.0, 500.0))]


def test_five_hour_window_ignores_weekly_percentages(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    snapshot(conn, 10, 12.0)

    assert calibration.compute(conn, PLANS, window="five_hour") == []
    assert [r.window for r in calibration.compute(conn, PLANS)] == ["seven_day"]


def test_unknown_window_is_rejected(conn):
    with pytest.raises(ValueError, match="weekly"):
        calibration.compute(conn, PLANS, window="weekly")


def test_ratio_spread_from_interval_percentiles(conn):
    for index, output in enumerate((20, 40, 60, 80, 100)):
        base = index * 20
        snapshot(conn, base, 10.0 + index)
        turn(conn, base + 5, output)
        snapshot(conn, base + 10, 11.0 + index)

    (ratio,) = ratios(conn)

    assert ratio.weighted_per_pct == pytest.approx(300.0)
    assert ratio.spread == pytest.approx((180.0, 420.0))


def test_spread_of_two_intervals_stays_within_them(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 20)
    snapshot(conn, 10, 11.0)
    turn(conn, 15, 100)
    snapshot(conn, 20, 12.0)

    (ratio,) = ratios(conn)

    assert ratio.spread == pytest.approx((180.0, 420.0))
    assert ratio.spread[0] >= 100.0 and ratio.spread[1] <= 500.0


def calibrated_day_one(conn):
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 200)
    snapshot(conn, 10, 12.0)


def test_outside_share_of_a_day_with_no_local_turns_is_one(conn):
    snapshot(conn, 0, 10.0)
    snapshot(conn, 10, 12.0)

    assert calibration.outside_share(conn, PLANS, DAY) == [OutsideShare("a", DAY, 2.0, 0.0, 1.0)]


def test_outside_share_is_the_rise_local_turns_do_not_explain(conn):
    calibrated_day_one(conn)
    snapshot(conn, 1440, 12.0)
    turn(conn, 1450, 200)
    snapshot(conn, 1460, 16.0)

    result = calibration.outside_share(conn, PLANS, (DAY, DAY + timedelta(days=1)))

    assert [(r.day, r.rise) for r in result] == [(DAY, 2.0), (DAY + timedelta(days=1), 4.0)]
    assert result[1].local_pct == pytest.approx(3.0)
    assert result[1].share == pytest.approx(0.25)


def test_outside_share_has_no_row_for_a_day_without_a_rise(conn):
    snapshot(conn, 0, 10.0)
    snapshot(conn, 10, 10.0)
    snapshot(conn, 30, 11.0, RESET_B)
    turn(conn, 20, 100)

    assert calibration.outside_share(conn, PLANS, DAY) == []


def test_outside_share_is_unknown_when_local_turns_have_no_ratio(conn):
    """The calibration interval is dominated by Opus from before a gap; the sonnet turns after it have no ratio."""
    snapshot(conn, 0, 10.0)
    turn(conn, 5, 2000)
    snapshot(conn, 10, 10.0)
    snapshot(conn, 500, 10.0)
    turn(conn, 505, 20, model="claude-sonnet-4-5")
    snapshot(conn, 510, 12.0)

    assert calibration.outside_share(conn, PLANS, DAY) == [OutsideShare("a", DAY, 2.0, None, None)]


def test_outside_share_does_not_attribute_a_rise_across_a_gap(conn):
    """Mac asleep for 10 hours: the rise over the hole cannot be tied to local turns, so it is set apart."""
    calibrated_day_one(conn)
    snapshot(conn, 20, 12.0)
    snapshot(conn, 620, 22.0)
    snapshot(conn, 630, 24.0)

    (row,) = calibration.outside_share(conn, PLANS, DAY)

    assert (row.rise, row.unattributed) == (4.0, 10.0)
    assert row.local_pct == pytest.approx(2.0) and row.share == pytest.approx(0.5)


def test_outside_share_of_a_day_with_only_a_gap_has_no_share(conn):
    snapshot(conn, 0, 10.0)
    snapshot(conn, 10, 10.0)
    snapshot(conn, 620, 20.0)

    assert calibration.outside_share(conn, PLANS, DAY) == [OutsideShare("a", DAY, 0.0, 0.0, None, 10.0)]


def test_outside_share_counts_the_turns_of_an_interval_that_straddles_midnight(conn):
    snapshot(conn, 950, 10.0)
    turn(conn, 955, 200)
    snapshot(conn, 965, 12.0)

    result = calibration.outside_share(conn, PLANS, (DAY, DAY + timedelta(days=1)))

    assert result == [OutsideShare("a", DAY + timedelta(days=1), 2.0, 2.0, 0.0)]


def test_outside_share_keeps_the_turns_of_flat_snapshots_for_the_next_rise(conn):
    """Percentages have 1% resolution: tokens spent over several flat snapshots show up at the next tick."""
    snapshot(conn, 0, 10.0)
    snapshot(conn, 10, 10.0)
    turn(conn, 15, 200)
    snapshot(conn, 20, 10.0)
    snapshot(conn, 30, 12.0)

    assert calibration.outside_share(conn, PLANS, DAY) == [OutsideShare("a", DAY, 2.0, 2.0, 0.0)]


WEIGHTS = {"max20x": 20.0, "max5x": 5.0}
SWITCH = datetime(2026, 11, 2, tzinfo=timezone.utc)
TWO_PLANS = (settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc)),
             settings.Plan("a", "max5x", SWITCH))
HISTORY = [Ratio("a", "max20x", "opus", 500.0, 4, 8.0), Ratio("a", "max20x", "mixed", 250.0, 2, 3.0)]


def test_estimate_sums_classes():
    known = [Ratio("a", "max20x", "opus", 500.0, 3, 6.0), Ratio("a", "max20x", "sonnet", 100.0, 3, 6.0)]

    result = calibration.estimate(known, PLANS, {}, "a", START, {"opus": 1000.0, "sonnet": 500.0})

    assert result.pct == pytest.approx(7.0)
    assert "max20x" in result.basis and "scaled" not in result.basis


def test_estimate_uses_mixed_for_a_class_without_a_ratio():
    result = calibration.estimate(HISTORY, PLANS, {}, "a", START, {"haiku": 500.0})

    assert result.pct == pytest.approx(2.0)


def test_estimate_scales_previous_plan_after_a_switch():
    after = SWITCH + timedelta(days=1)

    result = calibration.estimate(HISTORY, TWO_PLANS, WEIGHTS, "a", after, {"opus": 1000.0})

    assert result.pct == pytest.approx(8.0)
    assert "scaled from max20x" in result.basis


def test_estimate_prefers_own_plan_calibration_over_scaling():
    own = HISTORY + [Ratio("a", "max5x", "opus", 100.0, 2, 2.0)]

    result = calibration.estimate(own, TWO_PLANS, WEIGHTS, "a", SWITCH + timedelta(days=1), {"opus": 1000.0})

    assert result.pct == pytest.approx(10.0)
    assert "scaled" not in result.basis


def test_estimate_says_when_the_plan_period_is_estimated():
    guessed = (settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc), True),)

    result = calibration.estimate(HISTORY, guessed, {}, "a", START, {"opus": 1000.0})

    assert "estimated plan period" in result.basis


def test_estimate_is_none_when_nothing_applies():
    assert calibration.estimate([], PLANS, {}, "a", START, {"opus": 1.0}) is None
    assert calibration.estimate(HISTORY, PLANS, {}, "a", START - timedelta(days=999), {"opus": 1.0}) is None
    no_weights = calibration.estimate(HISTORY, TWO_PLANS, {}, "a", SWITCH + timedelta(days=1), {"opus": 1.0})
    assert no_weights is None


def test_codex_snapshots_stay_out_of_the_claude_calibration(conn):
    snapshot(conn, 0, 10.0, account="codex:a")
    snapshot(conn, 10, 12.0, account="codex:a")
    turn(conn, 5, 200, account="codex:a", provider="codex")

    analysis = calibration.analyze(conn, PLANS)

    assert (analysis.ratios, analysis.unexplained) == ([], {})
    assert calibration.outside_share(conn, PLANS, DAY) == []
