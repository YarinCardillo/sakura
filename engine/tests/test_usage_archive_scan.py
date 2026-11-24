import json
import shutil
from pathlib import Path

from sakura.usage import codex
from sakura.usage.archive import repos, scan, settings, store

COMPACT = {"separators": (",", ":")}
FIXTURES = Path(__file__).parent / "fixtures" / "usage" / "archive"
MARKER = "SECRET-MARKER-TEXT"
CLAUDE_SESSION = "proj/session-1.jsonl"
CODEX_ROLLOUT = "rollout-2026-10-18T16-59-20-sess-codex-1.jsonl"


def claude_source(tmp_path):
    root = tmp_path / "claude"
    shutil.copytree(FIXTURES / "claude", root)
    return settings.Source("claude", "personal", root)


def codex_source(tmp_path):
    root = tmp_path / "codex"
    shutil.copytree(FIXTURES / "codex", root)
    return settings.Source("codex", "personal", root)


def db(tmp_path):
    return store.open_db(tmp_path / "state" / "usage.db")


def turns(conn, provider):
    conn.row_factory = None
    return conn.execute(
        "SELECT event_key, session_id, agent_id, ts, cwd, branch, model, input_tokens, cache_write_tokens,"
        " cache_read_tokens, output_tokens, cost_usd FROM turns WHERE provider = ? ORDER BY ts, event_key",
        (provider,)).fetchall()


def test_claude_turns_one_row_per_message_with_max_counts(tmp_path):
    conn = db(tmp_path)

    stats = scan.scan_source(conn, claude_source(tmp_path))

    rows = turns(conn, "claude")
    assert [row[0] for row in rows] == ["msg_a", "msg_b"]
    assert rows[0][1:3] == ("s1", None)
    assert rows[0][4:] == ("/work/acme", "feature/x", "claude-test-1", 3, 100, 50, 25, None)
    assert stats == scan.ScanStats(files_seen=1, files_read=1, turns_written=2)


def test_claude_skips_synthetic_and_text_only_lines(tmp_path):
    conn = db(tmp_path)

    scan.scan_source(conn, claude_source(tmp_path))

    keys = {row[0] for row in turns(conn, "claude")}
    assert "msg_syn" not in keys and "msg_text" not in keys


def test_claude_head_branch_is_null(tmp_path):
    conn = db(tmp_path)

    scan.scan_source(conn, claude_source(tmp_path))

    by_key = {row[0]: row for row in turns(conn, "claude")}
    assert by_key["msg_b"][5] is None


def test_claude_subagent_file_carries_agent_id(tmp_path):
    source = claude_source(tmp_path)
    sub = source.root / "proj" / "session-1" / "subagents" / "agent-a1.jsonl"
    sub.parent.mkdir(parents=True)
    sub.write_text(json.dumps({
        "type": "assistant", "timestamp": "2026-10-20T11:00:00.000Z", "sessionId": "s1", "agentId": "a1",
        "message": {"id": "msg_sub", "model": "claude-test-1",
                    "usage": {"input_tokens": 1, "output_tokens": 2}}}, **COMPACT) + "\n")
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    by_key = {row[0]: row for row in turns(conn, "claude")}
    assert by_key["msg_sub"][2] == "a1"


def test_partial_last_line_waits_for_the_next_run(tmp_path):
    source = claude_source(tmp_path)
    path = source.root / CLAUDE_SESSION
    complete = path.read_bytes().splitlines(keepends=True)[0]
    tail = json.dumps({"type": "assistant", "timestamp": "2026-10-20T12:00:00.000Z", "sessionId": "s1",
                       "message": {"id": "msg_late", "model": "m",
                                   "usage": {"input_tokens": 1, "output_tokens": 2}}}, **COMPACT).encode()
    path.write_bytes(complete + tail[:40])
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert [row[0] for row in turns(conn, "claude")] == ["msg_a"]
    assert conn.execute("SELECT offset FROM files").fetchone()[0] == len(complete)
    path.write_bytes(complete + tail + b"\n")
    scan.scan_source(conn, source)
    assert [row[0] for row in turns(conn, "claude")] == ["msg_a", "msg_late"]


def test_unchanged_file_is_not_reread(tmp_path):
    source = claude_source(tmp_path)
    conn = db(tmp_path)
    scan.scan_source(conn, source)

    stats = scan.scan_source(conn, source)

    assert stats == scan.ScanStats(files_seen=1, files_read=0, turns_written=0)


def test_shrunk_file_is_rescanned_without_duplicates(tmp_path):
    source = claude_source(tmp_path)
    path = source.root / CLAUDE_SESSION
    conn = db(tmp_path)
    scan.scan_source(conn, source)
    before = turns(conn, "claude")
    lines = path.read_bytes().splitlines(keepends=True)
    path.write_bytes(b"".join(line for line in lines if b'"type":"user"' not in line))

    stats = scan.scan_source(conn, source)

    assert stats.files_read == 1
    assert turns(conn, "claude") == before


def test_codex_deltas_survive_between_runs(tmp_path):
    source = codex_source(tmp_path)
    path = source.root / CODEX_ROLLOUT
    lines = path.read_bytes().splitlines(keepends=True)
    full = path.read_bytes()
    path.write_bytes(b"".join(lines[:4]))
    conn = db(tmp_path)
    scan.scan_source(conn, source)
    path.write_bytes(full)
    scan.scan_source(conn, source)

    split_rows = turns(conn, "codex")
    whole = db(tmp_path / "other")
    whole_source = codex_source(tmp_path / "other")
    scan.scan_source(whole, whole_source)
    expected = codex.session_events(path)
    assert split_rows == turns(whole, "codex")
    assert sum(row[7] + row[9] + row[10] for row in split_rows) == sum(e.usage.total for e in expected)
    assert [row[10] for row in split_rows] == [e.usage.output for e in expected]


def test_codex_rows_carry_model_cwd_branch(tmp_path):
    conn = db(tmp_path)

    scan.scan_source(conn, codex_source(tmp_path))

    first = turns(conn, "codex")[0]
    assert first[1] == "sess-codex-1"
    assert first[4:7] == ("/work/acme/sub", "feature/y", "gpt-test-1")
    assert first[7:11] == (600, 0, 400, 50)


def test_no_message_text_is_stored(tmp_path):
    conn = db(tmp_path)
    scan.scan_source(conn, claude_source(tmp_path))
    scan.scan_source(conn, codex_source(tmp_path))
    repos.resolve_new_cwds(conn)

    dump = "\n".join(conn.iterdump())

    assert MARKER not in dump
    assert conn.execute("SELECT COUNT(*) FROM turns").fetchone()[0] > 0


def test_repo_name_main_checkout_and_worktree(tmp_path):
    main = tmp_path / "widgets"
    (main / ".git" / "worktrees" / "feat").mkdir(parents=True)
    (main / ".git" / "worktrees" / "feat" / "commondir").write_text("../..\n")
    (main / "src").mkdir()
    tree = tmp_path / "trees" / "feat"
    (tree / "lib").mkdir(parents=True)
    (tree / ".git").write_text(f"gitdir: {main}/.git/worktrees/feat\n")

    assert repos.repo_name(str(main / "src")) == "widgets"
    assert repos.repo_name(str(tree / "lib")) == "widgets"


def test_repo_name_missing_cwd_is_none(tmp_path):
    assert repos.repo_name(str(tmp_path / "gone" / "deeper")) is None
    assert repos.repo_name(str(tmp_path)) is None


def test_resolve_new_cwds_records_each_cwd_once(tmp_path):
    conn = db(tmp_path)
    scan.scan_source(conn, claude_source(tmp_path))

    assert repos.resolve_new_cwds(conn) == 1
    assert repos.resolve_new_cwds(conn) == 0
    assert conn.execute("SELECT cwd, repo FROM cwds").fetchall() == [("/work/acme", None)]


def test_resolve_new_cwds_re_resolves_a_cwd_that_reappears(tmp_path):
    conn = db(tmp_path)
    checkout = tmp_path / "widgets"
    conn.execute("INSERT INTO turns VALUES ('claude', 'k', 'a', 's', NULL, '2026-10-20T00:00:00+00:00', ?,"
                 " 'b', 'm', 1, 0, 0, 1, NULL)", (str(checkout),))
    assert repos.resolve_new_cwds(conn) == 1
    assert conn.execute("SELECT repo FROM cwds").fetchone()[0] is None

    (checkout / ".git").mkdir(parents=True)

    assert repos.resolve_new_cwds(conn) == 1
    assert conn.execute("SELECT repo FROM cwds").fetchone()[0] == "widgets"
    assert repos.resolve_new_cwds(conn) == 0


def test_claude_user_line_with_a_usage_object_is_not_parsed(monkeypatch):
    from sakura.usage.archive import records

    parsed = []
    real = records.json.loads
    monkeypatch.setattr(records.json, "loads", lambda line: parsed.append(line) or real(line))
    line = (b'{"type":"user","timestamp":"2026-10-20T10:00:00.000Z","toolUseResult":{"message":{"id":"msg_x",'
            b'"usage":{"input_tokens":1,"output_tokens":2}}}}\n')

    assert records.claude_turn(line, "personal", "stem") is None
    assert parsed == []


def split_scan(tmp_path, source, path, whole, cut):
    conn = db(tmp_path)
    path.write_bytes(whole[:cut])
    scan.scan_source(conn, source)
    path.write_bytes(whole)
    scan.scan_source(conn, source)
    return conn


def test_claude_line_split_inside_a_multibyte_character_is_read_once_whole(tmp_path):
    source = claude_source(tmp_path)
    path = source.root / CLAUDE_SESSION
    text = "café € \U0001f600"
    extra = json.dumps({"type": "assistant", "timestamp": "2026-10-20T12:00:00.000Z", "sessionId": "s1",
                        "cwd": "/work/é\U0001f600", "message": {"id": "msg_utf", "model": "m", "content": text,
                        "usage": {"input_tokens": 4, "output_tokens": 5}}}, ensure_ascii=False, **COMPACT) + "\n"
    whole = path.read_bytes() + extra.encode()
    start = len(path.read_bytes())
    emoji_at = start + extra.encode().index("\U0001f600".encode())
    for cut in range(emoji_at - 1, emoji_at + 5):
        conn = split_scan(tmp_path / f"cut{cut}", claude_source(tmp_path / f"cut{cut}"),
                          tmp_path / f"cut{cut}" / "claude" / CLAUDE_SESSION, whole, cut)
        by_key = {row[0]: row for row in turns(conn, "claude")}
        assert by_key["msg_utf"][4] == "/work/é\U0001f600"
        assert conn.execute("SELECT offset FROM files").fetchone()[0] == len(whole)
        assert len(by_key) == 3


def test_codex_keys_are_byte_offsets_when_a_multibyte_line_is_split(tmp_path):
    source = codex_source(tmp_path)
    path = source.root / CODEX_ROLLOUT
    lines = path.read_bytes().splitlines(keepends=True)
    lines[0] = lines[0].replace(MARKER.encode(), "café € \U0001f600".encode())
    whole = b"".join(lines)
    cut = len(lines[0]) + len(lines[1]) + lines[2].index(b'"event_msg"')
    whole_db = db(tmp_path / "whole")
    path.write_bytes(whole)
    scan.scan_source(whole_db, source)

    split_db = split_scan(tmp_path / "split", codex_source(tmp_path / "split"),
                          tmp_path / "split" / "codex" / CODEX_ROLLOUT, whole, cut)

    assert turns(split_db, "codex") == turns(whole_db, "codex")
    assert all("@" in row[0] and int(row[0].split("@")[1]) > len(lines[0]) for row in turns(split_db, "codex"))


# -- Codex quota from the rate limits of token_count lines --------------------------------------------------------

WEEK_RESET = 1793082754
WEEK_RESET_ISO = "2026-10-27T06:32:34Z"
SNAPSHOT_COLUMNS = ("account, ts, subscription_type, status, five_hour_pct, five_hour_resets_at, seven_day_pct,"
                    " seven_day_resets_at, overage_in_use")


def rate_line(second, limits, plan="prolite", reached=None):
    """A token_count line at 18:00:<second> carrying invented rate limits and a marker-bearing extra field."""
    body = {"limit_id": "codex", "limit_name": MARKER, "credits": {"balance": "777"}, "plan_type": plan,
            "rate_limit_reached_type": reached, **limits}
    line = {"timestamp": f"2026-10-18T18:00:{second:02d}.000Z", "type": "event_msg",
            "payload": {"type": "token_count", "info": None, "rate_limits": body}}
    return json.dumps(line, **COMPACT).encode() + b"\n"


def weekly(pct, window=10080):
    return {"primary": {"used_percent": pct, "window_minutes": window, "resets_at": WEEK_RESET}, "secondary": None}


def snapshots(conn):
    return conn.execute(f"SELECT {SNAPSHOT_COLUMNS} FROM quota_snapshots ORDER BY ts").fetchall()


def append(path, *lines):
    with open(path, "ab") as handle:
        handle.writelines(lines)


def test_codex_rollout_yields_a_weekly_snapshot(tmp_path):
    source = codex_source(tmp_path)
    append(source.root / CODEX_ROLLOUT, rate_line(1, weekly(47.0)))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert snapshots(conn) == [("codex:personal", "2026-10-18T18:00:01+00:00", "prolite", "allowed", None, None,
                                47.0, WEEK_RESET_ISO, None)]


def test_codex_snapshot_only_from_the_last_line_of_a_chunk(tmp_path):
    source = codex_source(tmp_path)
    path = source.root / CODEX_ROLLOUT
    append(path, rate_line(1, weekly(40.0)), rate_line(2, weekly(41.0)), rate_line(3, weekly(42.0)))
    conn = db(tmp_path)

    scan.scan_source(conn, source)
    append(path, rate_line(4, weekly(43.0)), rate_line(5, weekly(44.0)))
    scan.scan_source(conn, source)

    assert [(row[1][17:19], row[6]) for row in snapshots(conn)] == [("03", 42.0), ("05", 44.0)]


def test_codex_windows_are_mapped_by_length_not_position(tmp_path):
    source = codex_source(tmp_path)
    limits = {"primary": {"used_percent": 12.5, "window_minutes": 300, "resets_at": WEEK_RESET - 86400},
              "secondary": {"used_percent": 61.0, "window_minutes": 10080, "resets_at": WEEK_RESET}}
    append(source.root / CODEX_ROLLOUT, rate_line(1, limits))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    row = snapshots(conn)[0]
    assert (row[4], row[5], row[6], row[7]) == (12.5, "2026-10-26T06:32:34Z", 61.0, WEEK_RESET_ISO)


def test_codex_window_of_another_length_is_ignored(tmp_path):
    source = codex_source(tmp_path)
    path = source.root / CODEX_ROLLOUT
    append(path, rate_line(1, weekly(30.0)), rate_line(2, weekly(99.0, window=1440)))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert [(row[1][17:19], row[6]) for row in snapshots(conn)] == [("01", 30.0)]


def test_codex_reached_limit_type_becomes_the_status(tmp_path):
    source = codex_source(tmp_path)
    append(source.root / CODEX_ROLLOUT, rate_line(1, weekly(100.0), reached="primary"))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert snapshots(conn)[0][3] == "primary"


def test_codex_null_rate_limits_write_no_snapshot(tmp_path):
    conn = db(tmp_path)

    scan.scan_source(conn, codex_source(tmp_path))

    assert snapshots(conn) == []


def test_codex_rescan_of_the_same_bytes_does_not_duplicate_snapshots(tmp_path):
    source = codex_source(tmp_path)
    path = source.root / CODEX_ROLLOUT
    append(path, rate_line(1, weekly(47.0)))
    conn = db(tmp_path)
    scan.scan_source(conn, source)
    path.write_bytes(path.read_bytes() + b"\n")
    conn.execute("UPDATE files SET offset = 0")

    scan.scan_source(conn, source)

    assert len(snapshots(conn)) == 1


def test_codex_snapshot_keeps_apart_from_a_claude_account_of_the_same_name(tmp_path):
    source = codex_source(tmp_path)
    append(source.root / CODEX_ROLLOUT, rate_line(1, weekly(47.0)))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert [row[0] for row in snapshots(conn)] == ["codex:personal"]


def test_codex_snapshot_stores_nothing_else_from_the_line(tmp_path):
    source = codex_source(tmp_path)
    append(source.root / CODEX_ROLLOUT, rate_line(1, weekly(47.0)))
    conn = db(tmp_path)

    scan.scan_source(conn, source)

    assert MARKER not in repr(snapshots(conn)) and "777" not in repr(snapshots(conn))


def test_codex_snapshot_insert_names_its_columns_so_a_later_column_cannot_break_it(tmp_path):
    source = codex_source(tmp_path)
    append(source.root / CODEX_ROLLOUT, rate_line(1, weekly(47.0)))
    conn = db(tmp_path)
    conn.execute("ALTER TABLE quota_snapshots ADD COLUMN added_by_a_later_version TEXT")

    scan.scan_source(conn, source)

    assert conn.execute("SELECT COUNT(*) FROM quota_snapshots WHERE account = 'codex:personal'").fetchone() == (1,)
