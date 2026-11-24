from datetime import datetime, timedelta, timezone

from sakura.usage.archive import quota_report, settings, store

BASE = datetime(2026, 10, 20, 10, 0, tzinfo=timezone.utc)
SWITCH = BASE + timedelta(days=3)
PLANS = (settings.Plan("work", "max20x", BASE - timedelta(days=30)), settings.Plan("work", "max5x", SWITCH))


def at(hours):
    return BASE + timedelta(hours=hours)


def reset(hours):
    return at(hours).strftime("%Y-%m-%dT%H:%M:%SZ")


def build(tmp_path, hits, turns, plans=PLANS):
    """hits: (hour, window, resets_at or None); turns: (hour, output tokens), 200 output = 1000 weighted."""
    conn = store.open_db(tmp_path / "usage.db")
    for index, (hour, window, resets_at) in enumerate(hits):
        conn.execute("INSERT INTO limit_events VALUES ('work', ?, ?, ?, ?)",
                     (f"e{index}", at(hour).isoformat(), window, resets_at))
    for index, (hour, output) in enumerate(turns):
        conn.execute("INSERT INTO turns VALUES ('claude', ?, 'work', 's', NULL, ?, NULL, NULL, 'm', 0, 0, 0, ?, NULL)",
                     (f"t{index}", at(hour).isoformat(), output))
    config = settings.Settings(tmp_path / "usage.db", (), (), 7, plans=plans)
    return quota_report.build(conn, config)["limit_events"]


def test_repeated_hits_in_one_window_count_once_using_the_first_hit(tmp_path):
    hits = [(4, "five_hour", reset(5)), (4.5, "five_hour", reset(5)), (4.9, "five_hour", reset(5))]
    turns = [(1, 200), (4.4, 400)]

    events = build(tmp_path, hits, turns)

    assert len(events) == 1 and events[0]["windows"] == 1
    assert events[0]["detail"] == [{"first_hit": at(4).isoformat(), "resets_at": reset(5), "tokens": 1000.0}]


def test_five_hour_window_starts_at_the_reset_minus_five_hours(tmp_path):
    # reset at hour 6 means the window began at hour 1; the hit was at hour 4, a turn at 0.5 is outside.
    events = build(tmp_path, [(4, "five_hour", reset(6))], [(0.5, 200), (1.5, 100), (3, 100)])

    assert events[0]["detail"][0]["tokens"] == 1000.0


def test_five_hour_window_without_a_reset_looks_five_hours_back_from_the_hit(tmp_path):
    events = build(tmp_path, [(10, "five_hour", None)], [(4, 200), (6, 100)])

    assert events[0]["detail"][0]["tokens"] == 500.0


def test_windows_aggregate_per_plan_and_window_with_median_min_max(tmp_path):
    hits = [(2, "five_hour", reset(3)), (8, "five_hour", reset(9)), (14, "five_hour", reset(15)),
            (80, "five_hour", reset(81))]
    turns = [(1, 200), (7, 400), (13, 1000), (79, 40)]

    events = build(tmp_path, hits, turns)

    by_plan = {event["plan"]: event for event in events}
    old = by_plan["max20x"]
    assert (old["windows"], old["median"], old["min"], old["max"]) == (3, 2000.0, 1000.0, 5000.0)
    assert (by_plan["max5x"]["windows"], by_plan["max5x"]["max"]) == (1, 200.0)


def test_same_reset_in_different_windows_or_accounts_are_distinct_windows(tmp_path):
    hits = [(4, "five_hour", reset(5)), (4, "seven_day", reset(5)), (80, "five_hour", reset(81))]

    events = build(tmp_path, hits, [])

    assert sorted((e["plan"], e["window"], e["windows"]) for e in events) == [
        ("max20x", "five_hour", 1), ("max20x", "seven_day", 1), ("max5x", "five_hour", 1)]


def test_hits_without_a_reset_time_are_each_their_own_window(tmp_path):
    events = build(tmp_path, [(4, "five_hour", None), (4.5, "five_hour", None)], [])

    assert events[0]["windows"] == 2


def test_no_configured_plan_is_reported_as_unknown(tmp_path):
    events = build(tmp_path, [(4, "five_hour", reset(5))], [], plans=())

    assert events[0]["plan"] == "unknown"
