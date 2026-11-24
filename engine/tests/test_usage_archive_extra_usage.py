import json
from datetime import date, datetime, timedelta, timezone

import pytest

from sakura.usage.archive import command, settings, store

NOW = datetime(2026, 10, 28, 12, 0, tzinfo=timezone.utc)


def load(tmp_path, text):
    config = tmp_path / "usage.toml"
    config.write_text(text)
    return settings.load(config, {"HOME": str(tmp_path)})


def test_settings_reads_extra_usage_purchases(tmp_path):
    loaded = load(
        tmp_path,
        '[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "/h/projects"\n'
        '[[extra_usage]]\naccount = "work"\non = 2026-10-20\namount = 25\ncurrency = "EUR"\nnote = "top up"\n'
        '[[extra_usage]]\naccount = "work"\non = "2026-10-22"\namount = 10.5\ncurrency = "USD"\n')

    assert loaded.extra_usage == (settings.ExtraUsage("work", date(2026, 10, 20), 25.0, "EUR", "top up"),
                                  settings.ExtraUsage("work", date(2026, 10, 22), 10.5, "USD", ""))


def test_settings_extra_usage_defaults_to_none(tmp_path):
    assert load(tmp_path, "").extra_usage == ()


@pytest.mark.parametrize("entry, key", [
    ('account = "w"\non = "soon"\namount = 1\ncurrency = "EUR"', "extra_usage.on"),
    ('account = "w"\namount = 1\ncurrency = "EUR"', "extra_usage.on"),
    ('account = "w"\non = 2026-10-20\namount = "5"\ncurrency = "EUR"', "extra_usage.amount"),
    ('account = "w"\non = 2026-10-20\namount = -5\ncurrency = "EUR"', "extra_usage.amount"),
    ('account = "w"\non = 2026-10-20\namount = 5', "currency"),
    ('on = 2026-10-20\namount = 5\ncurrency = "EUR"', "account"),
    ('account = "w"\non = 2026-10-20\namount = 5\ncurrency = "EUR"\nnote = 3', "extra_usage.note"),
])
def test_settings_rejects_a_malformed_extra_usage_entry(tmp_path, entry, key):
    with pytest.raises(settings.UsageConfigError, match=key):
        load(tmp_path, f"[[extra_usage]]\n{entry}\n")


def seeded(tmp_path, purchases):
    config = settings.Settings(tmp_path / "state" / "usage.db", (), (), 7, extra_usage=purchases)
    conn = store.open_db(config.db)
    for key, ts in (("e1", "2026-10-19T23:00:00+00:00"), ("e2", "2026-10-21T10:00:00+00:00"),
                    ("e3", "2026-10-30T10:00:00+00:00")):
        conn.execute("INSERT INTO limit_events VALUES ('work', ?, ?, 'five_hour', NULL)", (key, ts))
    conn.execute("INSERT INTO limit_events VALUES ('other', 'x', '2026-10-20T12:00:00+00:00', 'seven_day', NULL)")
    for index, ts in enumerate(("2026-10-20T00:00:00+00:00", "2026-10-20T23:59:59+00:00", "2026-10-21T00:00:00Z")):
        conn.execute("INSERT INTO turns VALUES ('claude', ?, 'work', 's', NULL, ?, NULL, NULL, 'm', 0, 0, 0, 1, NULL)",
                     (f"t{index}", ts))
    conn.execute("INSERT INTO turns VALUES ('codex', 'c', 'work', 's', NULL, '2026-10-20T05:00:00+00:00',"
                 " NULL, NULL, 'm', 0, 0, 0, 1, NULL)")
    conn.commit()
    conn.close()
    return config


def view(config):
    conn = store.open_db(config.db)
    try:
        from sakura.usage.archive import quota_report
        return quota_report.build(conn, config)
    finally:
        conn.close()


def test_quota_view_shows_extra_usage_with_nearest_event_and_turns(tmp_path, capsys):
    purchase = settings.ExtraUsage("work", date(2026, 10, 20), 25.0, "EUR", "top up")
    config = seeded(tmp_path, (purchase,))

    assert command.quota_view(config, False) == 0

    lines = capsys.readouterr().out.split("extra usage\n", 1)[1].splitlines()
    assert lines[0].split()[:3] == ["account", "date", "amount"]
    assert lines[1].split() == ["work", "2026-10-20", "25.00", "EUR", "top", "up", "2026-10-19T23:00:00+00:00",
                                "five_hour", "2"]


def test_quota_json_has_the_same_extra_usage_facts(tmp_path):
    config = seeded(tmp_path, (settings.ExtraUsage("work", date(2026, 10, 20), 25.0, "EUR", "top up"),))

    entry = view(config)["extra_usage"][0]

    assert entry == {"account": "work", "date": "2026-10-20", "amount": 25.0, "currency": "EUR", "note": "top up",
                     "nearest_limit_event": {"ts": "2026-10-19T23:00:00+00:00", "window": "five_hour"},
                     "local_turns": 2}


def test_nearest_limit_event_window_is_two_days_before_to_one_day_after(tmp_path):
    config = seeded(tmp_path, (settings.ExtraUsage("work", date(2026, 10, 29), 5.0, "EUR", ""),
                               settings.ExtraUsage("work", date(2026, 10, 25), 5.0, "EUR", ""),
                               settings.ExtraUsage("work", date(2026, 10, 22), 5.0, "EUR", ""),
                               settings.ExtraUsage("ghost", date(2026, 10, 20), 5.0, "EUR", "")))

    found = [(e["nearest_limit_event"] or {}).get("ts") for e in view(config)["extra_usage"]]

    assert found == ["2026-10-30T10:00:00+00:00", None, "2026-10-21T10:00:00+00:00", None]
    assert [e["local_turns"] for e in view(config)["extra_usage"]] == [0, 0, 0, 0]


def test_quota_text_says_none_without_a_nearby_event(tmp_path, capsys):
    config = seeded(tmp_path, (settings.ExtraUsage("work", date(2026, 3, 1), 5.0, "EUR", ""),))

    command.quota_view(config, False)

    row = capsys.readouterr().out.split("extra usage\n", 1)[1].splitlines()[1].split()
    assert row[-2:] == ["none", "0"]
