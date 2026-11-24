"""Pre-flight estimate: a prediction in weighted tokens becomes a share of the week and of the 5-hour window."""

from datetime import datetime, timedelta, timezone

import pytest

from sakura.budget import plan_file, predict, preflight
from sakura.budget.settings import BudgetSettings
from sakura.usage.archive import calibration, settings, store

NOW = datetime(2026, 10, 28, 12, 0, tzinfo=timezone.utc)
RESET_A, RESET_B = "2026-11-03T08:00:00Z", "2026-11-10T08:00:00Z"
FIVE_A, FIVE_B = "2026-10-28T15:00:00Z", "2026-10-28T20:00:00Z"
SWITCH = datetime(2026, 10, 20, tzinfo=timezone.utc)
PLAN_OLD = settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc))
ONE_PLAN = (PLAN_OLD,)
TWO_PLANS = (PLAN_OLD, settings.Plan("a", "max5x", SWITCH))
BUDGET = BudgetSettings(account="a", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
                        ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00", snapshot_max_age_minutes=30)
SONNET_BAND = predict.Band(10000.0, 5000.0, 20000.0)


@pytest.fixture
def conn(tmp_path):
    return store.open_db(tmp_path / "usage.db")


def usage_for(plans=ONE_PLAN, weights=None):
    return settings.Settings(db=None, sources=(), pr_repos=(), pr_days=7, plans=plans, plan_weights=weights or {})


def prediction(band=SONNET_BAND, basis="8 of 8 parts from history"):
    return predict.FlowPrediction({"sonnet": band}, {}, (), basis)


def snapshot(conn, when, week, five, week_resets=RESET_A, five_resets=FIVE_A, account="a"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at)"
                 " VALUES (?, ?, 'max', 'allowed', ?, ?, ?, ?)",
                 (account, when.isoformat(), five, five_resets, week, week_resets))


def calibrate(conn, per_pct, start=datetime(2026, 10, 10, tzinfo=timezone.utc), model="claude-sonnet-4-5"):
    """One snapshot per 1% rise of both windows, each rise explained by one turn of `per_pct[i]` weighted tokens."""
    snapshot(conn, start, 10.0, 10.0)
    for step, weighted in enumerate(per_pct, start=1):
        moment = start + timedelta(minutes=10 * step)
        conn.execute("INSERT INTO turns VALUES ('claude', ?, 'a', 's', NULL, ?, NULL, NULL, ?, 0, 0, 0, ?, NULL)",
                     (f"k{step}", (moment - timedelta(minutes=5)).isoformat(), model, round(weighted / 5)))
        snapshot(conn, moment, 10.0 + step, 10.0 + step)


def latest(conn, week, five=20.0, minutes_ago=5):
    """The newest snapshot, in a new window so it forms no calibration interval."""
    snapshot(conn, NOW - timedelta(minutes=minutes_ago), week, five, RESET_B, FIVE_B)


def estimate(conn, pred=None, plans=ONE_PLAN, weights=None):
    return preflight.estimate(pred or prediction(), "a", NOW, conn, BUDGET, usage_for(plans, weights))


@pytest.fixture
def calibrated(conn):
    calibrate(conn, [1000.0] * 6)
    return conn


def test_preflight_text_is_exact():
    result = preflight.Preflight(preflight.Band(4.4, 8.4, 14.6), preflight.Band(9.0, 12.2, 20.0), "max20x calibration",
                                 True, "")
    assert preflight.text(result) == "about 8% of your week (4-15), 12% of the 5-hour window; max20x calibration"


def test_the_week_and_the_5_hour_window_are_converted_by_their_own_ratios(calibrated):
    latest(calibrated, 30.0)
    result = estimate(calibrated)
    assert result.week_pct.mid == pytest.approx(10.0)
    assert result.five_hour_pct.mid == pytest.approx(10.0)
    assert (result.week_pct.low, result.week_pct.high) == (pytest.approx(5.0), pytest.approx(20.0))
    assert "max20x calibration" in result.basis and "8 of 8 parts from history" in result.basis


def test_preflight_asks_above_the_ask_threshold(calibrated):
    latest(calibrated, 30.0)
    result = estimate(calibrated)
    assert result.fits
    assert preflight.level(result, BUDGET) == preflight.ASK


def test_preflight_warns_between_the_two_thresholds(calibrated):
    latest(calibrated, 30.0)
    result = estimate(calibrated, prediction(predict.Band(4000.0, 2000.0, 8000.0)))
    assert (result.week_pct.high, result.fits) == (pytest.approx(8.0), True)
    assert preflight.level(result, BUDGET) == preflight.WARN


def test_a_cheap_flow_that_fits_does_not_stop(calibrated):
    latest(calibrated, 30.0)
    result = estimate(calibrated, prediction(predict.Band(2000.0, 1000.0, 4000.0)))
    assert preflight.level(result, BUDGET) == preflight.OK


def test_preflight_does_not_fit_into_the_reserve(calibrated):
    latest(calibrated, 74.0)
    result = estimate(calibrated, prediction(predict.Band(1000.0, 500.0, 2000.0)))
    assert result.week_pct.high == pytest.approx(2.0)
    assert not result.fits
    assert "74" in result.reason and "25% reserve" in result.reason
    assert preflight.level(result, BUDGET) == preflight.ASK


def test_a_flow_that_ends_exactly_at_the_reserve_fits(calibrated):
    latest(calibrated, 73.0)
    assert estimate(calibrated, prediction(predict.Band(1000.0, 500.0, 2000.0))).fits


@pytest.mark.parametrize("minutes_ago", [31, 600])
def test_stale_snapshot_makes_preflight_ask(calibrated, minutes_ago):
    latest(calibrated, 1.0, minutes_ago=minutes_ago)
    result = estimate(calibrated)
    assert not result.fits
    assert "quota unknown" in result.reason and f"{minutes_ago} min" in result.reason
    assert preflight.level(result, BUDGET) == preflight.ASK


def test_no_snapshot_at_all_makes_preflight_ask(conn):
    result = estimate(conn)
    assert not result.fits
    assert "quota unknown" in result.reason and "no snapshot" in result.reason


def test_a_snapshot_without_a_weekly_percentage_does_not_count(calibrated):
    latest(calibrated, None)
    assert "quota unknown" in estimate(calibrated).reason


def test_without_any_calibration_no_percent_is_computed_and_the_flow_asks(conn):
    latest(conn, 5.0)
    result = estimate(conn)
    assert (result.week_pct, result.five_hour_pct, result.basis) == (None, None, "no calibration")
    assert not result.fits and "no calibration" in result.reason
    assert preflight.text(result) == "no percent estimate; no calibration"
    assert preflight.level(result, BUDGET) == preflight.ASK


def test_a_prediction_without_history_computes_no_percent_either(calibrated):
    latest(calibrated, 5.0)
    result = estimate(calibrated, predict.FlowPrediction({}, {}, ("T1",), predict.NO_HISTORY))
    assert result.week_pct is None and "no history" in result.basis
    assert not result.fits


def test_tokens_of_non_claude_models_do_not_draw_on_the_week(calibrated):
    latest(calibrated, 30.0)
    mixed = predict.FlowPrediction({"sonnet": SONNET_BAND, "other": predict.Band(9e9, 9e9, 9e9)}, {}, (), "x")
    assert estimate(calibrated, mixed).week_pct.mid == pytest.approx(10.0)


def switched_conn(conn):
    calibrate(conn, [1000.0] * 6, start=datetime(2026, 10, 1, tzinfo=timezone.utc))
    latest(conn, 30.0)
    return conn


def test_a_fresh_plan_uses_the_scaled_previous_plan_and_widens_the_band(conn):
    result = estimate(switched_conn(conn), plans=TWO_PLANS, weights={"max20x": 20.0, "max5x": 5.0})
    assert result.week_pct.mid == pytest.approx(40.0)
    assert result.week_pct.low <= 20.0 and result.week_pct.high >= 80.0
    assert "scaled from max20x" in result.basis and "band widened" in result.basis


def test_a_calibrated_plan_is_not_widened(calibrated):
    latest(calibrated, 30.0)
    assert "widened" not in estimate(calibrated).basis


def test_the_calibration_spread_widens_the_band(conn):
    calibrate(conn, [800.0, 1000.0, 1200.0, 1000.0, 1000.0])
    latest(conn, 30.0)
    result = estimate(conn)
    assert result.week_pct.mid == pytest.approx(10.0)
    assert result.week_pct.low == pytest.approx(5000 / 1040)
    assert result.week_pct.high == pytest.approx(20000 / 960)


def test_a_ratio_without_spread_keeps_the_token_band(monkeypatch, conn):
    latest(conn, 30.0)
    bare = [calibration.Ratio("a", "max20x", "sonnet", 1000.0, 3, 3.0)]
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window=calibration.SEVEN_DAY: bare)
    result = estimate(conn)
    assert (result.week_pct.low, result.week_pct.mid, result.week_pct.high) == (5.0, 10.0, 20.0)


def test_5_hour_window_without_calibration_leaves_the_week_computed(conn):
    calibrate(conn, [1000.0] * 3)
    conn.execute("UPDATE quota_snapshots SET five_hour_pct = NULL")
    latest(conn, 30.0)
    result = estimate(conn)
    assert result.week_pct is not None and result.five_hour_pct is None
    assert "5-hour window not calibrated" in preflight.text(result)


# -- the plan file ------------------------------------------------------------

SAMPLE_PLAN = """# Feature plan

Some prose.

### Task 1: Parser

**Files:** Create `lib/a/parser.py`, `lib/a/other.py`. Modify `lib/a/cli.py` (line 40). Test: `tests/test_parser.py`.

Class: mechanical

- [ ] Step one

### Task 2: Wiring

**Files:**
- Create: `lib/b/one.py`
- Modify: `lib/b/two.py:10-20`
- Test: `tests/test_b.py`

### Task 3: Docs

**Files:** README.md, docs/guide.md
**Class:** design

## Not a task

**Files:** `ignored.py`
"""


def test_plan_file_counts_the_files_of_each_task():
    tasks = plan_file.parse(SAMPLE_PLAN)
    assert [task.id for task in tasks] == ["1", "2", "3"]
    assert [task.title for task in tasks] == ["Parser", "Wiring", "Docs"]
    assert [len(task.files) for task in tasks] == [4, 3, 2]


def test_plan_file_reads_the_class_or_defaults_to_integration():
    assert [task.task_class for task in plan_file.parse(SAMPLE_PLAN)] == ["mechanical", "integration", "design"]


def test_plan_file_rejects_an_unknown_class():
    with pytest.raises(plan_file.PlanFileError, match="Task 1.*frobnicate"):
        plan_file.parse("### Task 1: X\n\n**Files:** `a.py`\nClass: frobnicate\n")


def test_plan_file_without_tasks_is_an_error():
    with pytest.raises(plan_file.PlanFileError, match="no ### Task"):
        plan_file.parse("# Only prose\n")


def test_the_post_build_share_is_what_the_evaluator_and_reporter_hold_of_the_high_estimate(calibrated):
    latest(calibrated, 30.0)
    parts = {name: predict.Prediction(high / 2, high / 4, high, "b", 5)
             for name, high in (("T1", 16000.0), ("evaluator", 3000.0), ("reporter", 1000.0))}
    result = estimate(calibrated, predict.FlowPrediction({"sonnet": SONNET_BAND}, parts, (), "x"))
    assert result.post_build_share == pytest.approx(0.2)
    assert preflight.summary(result, BUDGET)["post_build_pct"] == pytest.approx(result.week_pct.high * 0.2)


def test_a_prediction_without_parts_has_no_post_build_share(calibrated):
    latest(calibrated, 30.0)
    result = estimate(calibrated)
    assert result.post_build_share == 0.0
    assert preflight.summary(result, BUDGET)["post_build_pct"] == 0.0
