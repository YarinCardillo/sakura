import json

from sakura.usage.archive import scan, settings, store

COMPACT = {"separators": (",", ":")}
MARKER = "SECRET-MARKER-TEXT"


def limit_line(**overrides):
    line = {
        "type": "assistant", "uuid": "u-limit-1", "requestId": "req_1", "timestamp": "2026-10-09T01:00:04.528Z",
        "error": "rate_limit", "isApiErrorMessage": True, "apiErrorStatus": 429,
        "quotaLimits": {"status": "rejected", "resetsAt": 1791512400, "rateLimitType": "five_hour",
                        "overageStatus": "rejected"},
        "message": {"id": "msg_limit", "model": "<synthetic>", "content": [{"type": "text", "text": MARKER}],
                    "usage": {"input_tokens": 0, "output_tokens": 0}},
        "sessionId": "s9", "cwd": MARKER,
    }
    line.update(overrides)
    return json.dumps(line, **COMPACT) + "\n"


def scanned(tmp_path, *lines):
    root = tmp_path / "claude"
    (root / "proj").mkdir(parents=True)
    (root / "proj" / "s9.jsonl").write_text("".join(lines))
    conn = store.open_db(tmp_path / "state" / "usage.db")
    scan.scan_source(conn, settings.Source("claude", "work", root))
    return conn


def events(conn):
    return conn.execute("SELECT account, event_key, ts, window, resets_at FROM limit_events").fetchall()


def test_limit_event_is_archived_with_window_and_reset(tmp_path):
    conn = scanned(tmp_path, limit_line())

    assert events(conn) == [("work", "u-limit-1", "2026-10-09T01:00:04.528000+00:00", "five_hour",
                             "2026-10-09T02:20:00Z")]
    assert conn.execute("SELECT COUNT(*) FROM turns").fetchone()[0] == 0


def test_limit_event_falls_back_to_the_request_id_and_ignores_duplicates(tmp_path):
    no_uuid = limit_line(uuid=None)
    conn = scanned(tmp_path, no_uuid, no_uuid)

    assert [row[1] for row in events(conn)] == ["req_1"]


def test_limit_event_stores_no_text(tmp_path):
    conn = scanned(tmp_path, limit_line())

    dump = "\n".join(conn.iterdump())
    assert MARKER not in dump


def test_other_api_errors_and_malformed_limits_are_not_events(tmp_path):
    other = limit_line(error="server_error", uuid="u2")
    no_key = limit_line(uuid=None, requestId=None)
    no_time = limit_line(timestamp=None, uuid="u3")
    not_error = limit_line(isApiErrorMessage=False, uuid="u4")

    conn = scanned(tmp_path, other, no_key, no_time, not_error)

    assert events(conn) == []


def test_limit_event_without_reset_time_keeps_a_null_reset(tmp_path):
    bare = limit_line(quotaLimits={"rateLimitType": "seven_day"})

    conn = scanned(tmp_path, bare)

    assert [(row[3], row[4]) for row in events(conn)] == [("seven_day", None)]


def legacy_line(text, timestamp="2026-07-07T13:14:54.075Z", **overrides):
    line = {"type": "assistant", "uuid": "u-old-1", "requestId": "req_old", "timestamp": timestamp,
            "error": "rate_limit", "isApiErrorMessage": True, "apiErrorStatus": 429,
            "message": {"content": [{"type": "text", "text": text}]}, "sessionId": "s9"}
    line.update(overrides)
    return json.dumps(line, **COMPACT) + "\n"


def legacy_events(tmp_path, *lines):
    return sorted(row[1:] for row in events(scanned(tmp_path, *lines)))


def test_legacy_session_line_is_a_five_hour_event_with_a_zoned_reset(tmp_path):
    line = legacy_line("You've hit your session limit \u00b7 resets 4:10pm (Europe/Rome)")

    assert legacy_events(tmp_path, line) == [
        ("u-old-1", "2026-07-07T13:14:54.075000+00:00", "five_hour", "2026-07-07T14:10:00Z")]


def test_legacy_weekly_line_is_a_seven_day_event_and_other_words_pass_through(tmp_path):
    weekly = legacy_line("You've hit your weekly limit \u00b7 resets 9am (Europe/Rome)", uuid="w")
    opus = legacy_line("You've hit your Opus limit \u00b7 resets 9am (Europe/Rome)", uuid="o")

    rows = legacy_events(tmp_path, weekly, opus)

    assert [(row[0], row[2], row[3]) for row in rows] == [
        ("o", "Opus", "2026-07-08T07:00:00Z"), ("w", "seven_day", "2026-07-08T07:00:00Z")]


def test_legacy_reset_on_the_next_local_day(tmp_path):
    line = legacy_line("You've hit your session limit \u00b7 resets 5:10am (Europe/Rome)",
                       timestamp="2026-07-07T21:44:00.000Z")

    assert legacy_events(tmp_path, line)[0][3] == "2026-07-08T03:10:00Z"


def test_legacy_reset_without_minutes_and_twelve_oclock(tmp_path):
    noon = legacy_line("You've hit your session limit \u00b7 resets 12pm (UTC)", uuid="n")
    midnight = legacy_line("You've hit your session limit \u00b7 resets 12am (UTC)", uuid="m")

    rows = legacy_events(tmp_path, noon, midnight)

    assert [(row[0], row[3]) for row in rows] == [("m", "2026-07-08T00:00:00Z"), ("n", "2026-07-08T12:00:00Z")]


def test_legacy_unknown_zone_or_missing_reset_keeps_the_event_with_a_null_reset(tmp_path):
    bad_zone = legacy_line("You've hit your session limit \u00b7 resets 4:10pm (Mars/Olympus)", uuid="z")
    no_reset = legacy_line("You've hit your session limit", uuid="r")
    dated = legacy_line("You've hit your weekly limit \u00b7 resets Jul 10, 5pm (Europe/Rome)", uuid="d")

    rows = legacy_events(tmp_path, bad_zone, no_reset, dated)

    assert [(row[0], row[3]) for row in rows] == [("d", None), ("r", None), ("z", None)]


def test_legacy_lines_without_the_limit_phrase_store_nothing(tmp_path):
    other = legacy_line("API Error: 429 something else", uuid="a")
    no_text = legacy_line("x", uuid="b", message={"content": []})
    second_block = legacy_line("x", uuid="c", message={"content": [
        {"type": "text", "text": "hello"}, {"type": "text", "text": "You've hit your session limit"}]})

    assert legacy_events(tmp_path, other, no_text, second_block) == []


def test_legacy_line_stores_no_text_and_is_not_a_turn(tmp_path):
    line = legacy_line(f"You've hit your session limit \u00b7 resets 4:10pm (Europe/Rome) {MARKER}",
                       message={"content": [{"type": "text", "text":
                                            f"You've hit your session limit \u00b7 resets 4:10pm (UTC) {MARKER}"}],
                                "usage": {"input_tokens": 1, "output_tokens": 1}, "id": "m1", "model": "x"})

    conn = scanned(tmp_path, line)

    assert MARKER not in "\n".join(conn.iterdump())
    assert len(events(conn)) == 1
    assert conn.execute("SELECT COUNT(*) FROM turns").fetchone()[0] == 0
