from datetime import datetime, timedelta, timezone

from sakura.quota import forecast as fc

NOW = datetime(2026, 11, 2, 21, 0, tzinfo=timezone.utc)
RESETS = NOW + timedelta(hours=34)


def hourly(*pcts, end=NOW):
    """One sample per hour, the last one at `end`."""
    return [(end - timedelta(hours=len(pcts) - 1 - index), pct) for index, pct in enumerate(pcts)]


def test_Forecast_NoHistory_ReturnsNone():
    assert fc.forecast([], RESETS, NOW) == fc.NO_FORECAST


def test_Forecast_NoResetTime_ReturnsNone():
    assert fc.forecast(hourly(50.0), None, NOW) == fc.NO_FORECAST


def test_Forecast_SingleSample_FlatAtCurrentWithLowConfidence():
    result = fc.forecast(hourly(94.0), RESETS, NOW)
    assert (result.seven_day_pct_at_reset, result.exhausts_at, result.confidence) == (94.0, None, fc.LOW)


def test_Forecast_RisingTrend_ProjectsPastTheLimitAndNamesWhen():
    result = fc.forecast(hourly(80.0, 82.0, 84.0, 86.0, 88.0, 90.0, 92.0), RESETS, NOW)
    assert result.seven_day_pct_at_reset == 92.0 + 2.0 * 34
    assert result.exhausts_at is not None and NOW < result.exhausts_at < RESETS
    assert result.confidence == fc.HIGH


def test_Forecast_FlatTrend_StaysAtCurrent():
    result = fc.forecast(hourly(40.0, 40.0, 40.0), RESETS, NOW)
    assert (result.seven_day_pct_at_reset, result.exhausts_at) == (40.0, None)
    assert result.confidence == fc.MEDIUM


def test_Forecast_NextRound_AddsTheRiseOfTheLastFiveHours():
    result = fc.forecast(hourly(80.0, 82.0, 84.0, 86.0, 88.0, 90.0, 92.0), RESETS, NOW)
    assert result.next_round_pct == 92.0 + 10.0


def test_Forecast_NextRound_NeverLessThanTheFloor():
    result = fc.forecast(hourly(40.0, 40.0, 40.0), RESETS, NOW)
    assert result.next_round_pct == 40.0 + fc.ROUND_FLOOR_PCT


def test_Forecast_StaleNewestSample_HasNoTrend():
    old = NOW - timedelta(hours=8)
    result = fc.forecast(hourly(50.0, 52.0, end=old), RESETS, NOW)
    assert (result.seven_day_pct_at_reset, result.confidence) == (52.0, fc.LOW)
