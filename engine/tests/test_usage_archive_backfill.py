import hashlib
import json
import sqlite3
from pathlib import Path

from sakura.usage.archive import backfill, quota, scan, settings, store

COMPACT = {"separators": (",", ":")}
WEEK_RESET = 1793082754
MARKER = "SECRET-MARKER-TEXT"


def rate_line(second, pct):
    """A token_count line at 18:00:<second> carrying a weekly sample and a marker-bearing extra field."""
    body = {"limit_id": MARKER, "plan_type": "prolite", "rate_limit_reached_type": None,
            "primary": {"used_percent": pct, "window_minutes": 10080, "resets_at": WEEK_RESET}, "secondary": None}
    line = {"timestamp": f"2026-10-18T18:00:{second:02d}.000Z", "type": "event_msg",
            "payload": {"type": "token_count", "info": None, "rate_limits": body}}
    return json.dumps(line, **COMPACT).encode() + b"\n"


def make_rollout(root, name, *lines):
    root.mkdir(parents=True, exist_ok=True)
    path = root / name
    path.write_bytes(b"".join(lines))
    return path


def config_for(tmp_path, *accounts):
    sources = tuple(settings.Source("codex", account, tmp_path / account) for account in accounts)
    return settings.Settings(db=tmp_path / "state" / "usage.db", sources=sources, pr_repos=(), pr_days=7)


def codex_rows(config, account="alpha"):
    conn = store.open_db(config.db)
    try:
        return conn.execute("SELECT ts, seven_day_pct, overage_in_use FROM quota_snapshots WHERE account = ?"
                            " ORDER BY ts", (f"codex:{account}",)).fetchall()
    finally:
        conn.close()


def db_digest(config):
    return hashlib.sha256(config.db.read_bytes()).hexdigest()


def seeded(tmp_path, *accounts):
    """Settings whose rollouts hold three samples per account, with the archive db created and empty."""
    config = config_for(tmp_path, *accounts)
    for account in accounts:
        make_rollout(tmp_path / account, "rollout-1.jsonl", rate_line(1, 10.0), rate_line(2, 11.0))
        make_rollout(tmp_path / account / "2026", "rollout-2.jsonl", rate_line(3, 12.0), b"not json\n")
    store.open_db(config.db).close()
    return config


def test_dry_run_counts_and_writes_nothing(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")
    before = db_digest(config)

    code = backfill.run(config, write=False)

    assert code == 0
    assert db_digest(config) == before
    assert capsys.readouterr().out.splitlines()[0] == (
        "codex:alpha: 2 rollouts, 3 samples, 3 new, 0 present, "
        "earliest 2026-10-18T18:00:01+00:00, latest 2026-10-18T18:00:03+00:00")


def test_write_inserts_every_sample_once(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")

    code = backfill.run(config, write=True)

    assert code == 0
    assert codex_rows(config) == [("2026-10-18T18:00:01+00:00", 10.0, None), ("2026-10-18T18:00:02+00:00", 11.0, None),
                                  ("2026-10-18T18:00:03+00:00", 12.0, None)]
    assert capsys.readouterr().out.splitlines()[-1] == "inserted 3"


def test_second_write_inserts_nothing(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")
    backfill.run(config, write=True)
    capsys.readouterr()

    backfill.run(config, write=True)

    out = capsys.readouterr().out.splitlines()
    assert "3 samples, 0 new, 3 present" in out[0]
    assert out[-1] == "inserted 0"
    assert len(codex_rows(config)) == 3


def test_existing_rows_are_kept_and_counted_present(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")
    conn = store.open_db(config.db)
    with conn:
        conn.execute(quota.INSERT_SNAPSHOT, ("codex:alpha", "2026-10-18T18:00:02+00:00", "plus", "allowed",
                                             None, None, 99.0, None, 1))
    conn.close()

    backfill.run(config, write=True)

    rows = codex_rows(config)
    assert [row[1] for row in rows] == [10.0, 99.0, 12.0]
    assert "3 samples, 2 new, 1 present" in capsys.readouterr().out.splitlines()[0]


def test_write_holds_the_archive_lock(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")

    with store.archive_lock(config.db):
        code = backfill.run(config, write=True)

    captured = capsys.readouterr()
    assert code == 1
    assert "running" in captured.out + captured.err
    assert codex_rows(config) == []


def test_write_holds_the_lock_while_inserting(tmp_path, monkeypatch):
    config = seeded(tmp_path, "alpha")
    held_during = []
    real = store.open_db

    def open_while_locked(path):
        with store.archive_lock(path) as free:
            held_during.append(not free)
        return real(path)

    monkeypatch.setattr(store, "open_db", open_while_locked)

    backfill.run(config, write=True)

    assert held_during == [True]


def test_two_codex_accounts_backfill_separately(tmp_path, capsys):
    config = seeded(tmp_path, "beta", "alpha")
    make_rollout(tmp_path / "beta", "rollout-3.jsonl", rate_line(4, 20.0))

    backfill.run(config, write=True)

    out = capsys.readouterr().out.splitlines()
    assert out[0].startswith("codex:alpha: 2 rollouts, 3 samples")
    assert out[1].startswith("codex:beta: 3 rollouts, 4 samples")
    assert (len(codex_rows(config, "alpha")), len(codex_rows(config, "beta"))) == (3, 4)
    assert out[-1] == "inserted 7"


def test_disabled_codex_account_is_skipped(tmp_path, capsys):
    config = seeded(tmp_path, "alpha", "gamma")
    config = settings.Settings(db=config.db, sources=config.sources[:1], pr_repos=(), pr_days=7,
                               disabled=config.sources[1:])

    backfill.run(config, write=True)

    assert "gamma" not in capsys.readouterr().out
    assert codex_rows(config, "gamma") == []
    assert len(codex_rows(config, "alpha")) == 3


def test_token_check_reports_unread_rollouts(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")
    conn = store.open_db(config.db)
    source = config.sources[0]
    full, short = tmp_path / "alpha" / "rollout-1.jsonl", tmp_path / "alpha" / "2026" / "rollout-2.jsonl"
    with conn:
        conn.execute(scan.UPSERT_FILE, (str(full), "codex", "alpha", full.stat().st_size, full.stat().st_size, 0, "{}"))
        conn.execute(scan.UPSERT_FILE, (str(short), "codex", "alpha", 5, short.stat().st_size, 0, "{}"))
    conn.close()

    backfill.run(config, write=False)

    assert "codex:alpha: 1 rollouts not fully read into turns" in capsys.readouterr().out.splitlines()
    assert source.provider == "codex"


def test_token_check_counts_rollouts_missing_from_files(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")

    backfill.run(config, write=False)

    assert "codex:alpha: 2 rollouts not fully read into turns" in capsys.readouterr().out.splitlines()


def test_nothing_of_a_line_is_printed_or_stored(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")

    backfill.run(config, write=True)

    conn = sqlite3.connect(config.db)
    stored = "".join(str(value) for row in conn.execute("SELECT * FROM quota_snapshots") for value in row)
    conn.close()
    assert MARKER not in capsys.readouterr().out + stored


def test_dry_run_without_an_archive_db_exits_one(tmp_path, capsys):
    config = config_for(tmp_path, "alpha")

    code = backfill.run(config, write=False)

    assert code == 1
    assert not config.db.exists()
    assert "no usage archive" in capsys.readouterr().err


def test_claude_sources_are_ignored(tmp_path, capsys):
    config = seeded(tmp_path, "alpha")
    claude = settings.Source("claude", "delta", Path(tmp_path / "delta"))
    config = settings.Settings(db=config.db, sources=(claude, *config.sources), pr_repos=(), pr_days=7)

    backfill.run(config, write=False)

    assert "delta" not in capsys.readouterr().out


def test_cli_backfill_codex_is_a_dry_run_unless_write_is_given(tmp_path, monkeypatch, capsys):
    from sakura import cli

    config = seeded(tmp_path, "alpha")
    monkeypatch.setattr(settings, "load", lambda *args, **kwargs: config)

    assert cli.main(["usage", "backfill-codex"]) == 0
    assert codex_rows(config) == []
    assert cli.main(["usage", "backfill-codex", "--write"]) == 0
    assert len(codex_rows(config)) == 3
    assert "inserted 3" in capsys.readouterr().out
