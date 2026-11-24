import stat
from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura.usage.archive import settings, store


def test_open_db_creates_schema_and_private_file(tmp_path):
    path = tmp_path / "state" / "sakura" / "usage.db"

    conn = store.open_db(path)

    assert conn.execute("PRAGMA user_version").fetchone()[0] == 5
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    views = {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'view'")}
    assert views == {"sessions", "pr_turns"}
    assert conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"


def test_open_db_is_idempotent(tmp_path):
    path = tmp_path / "usage.db"
    first = store.open_db(path)
    first.execute("INSERT INTO meta VALUES ('k', 'v')")
    first.commit()
    first.close()

    second = store.open_db(path)

    assert second.execute("SELECT value FROM meta WHERE key = 'k'").fetchone()[0] == "v"
    assert second.execute("PRAGMA user_version").fetchone()[0] == 5


def test_archive_lock_refuses_a_second_holder(tmp_path):
    path = tmp_path / "usage.db"

    with store.archive_lock(path) as first:
        with store.archive_lock(path) as second:
            assert (first, second) == (True, False)
    with store.archive_lock(path) as again:
        assert again is True


def test_settings_defaults_without_file(tmp_path):
    env = {"HOME": str(tmp_path)}
    (tmp_path / ".claude" / "projects").mkdir(parents=True)
    (tmp_path / ".codex" / "sessions").mkdir(parents=True)

    loaded = settings.load(tmp_path / "missing.toml", env)

    assert loaded.db == tmp_path / ".local" / "state" / "sakura" / "usage.db"
    assert loaded.sources == (
        settings.Source("claude", "default", tmp_path / ".claude" / "projects", tmp_path / ".claude"),
        settings.Source("codex", "default", tmp_path / ".codex" / "sessions", tmp_path / ".codex"),
    )
    assert loaded.pr_repos == ()
    assert loaded.pr_days == 60


def test_settings_codex_home_overrides_default(tmp_path):
    env = {"HOME": str(tmp_path), "CODEX_HOME": str(tmp_path / "cx")}
    (tmp_path / "cx" / "sessions").mkdir(parents=True)

    loaded = settings.load(tmp_path / "missing.toml", env)

    assert [source.root for source in loaded.sources] == [tmp_path / "cx" / "sessions"]


def test_settings_reads_sources_and_repos(tmp_path):
    config = tmp_path / "usage.toml"
    config.write_text(
        'db = "~/data/u.db"\npr_repos = ["acme/widgets"]\npr_days = 14\n'
        '[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "~/w/projects"\n'
        '[[sources]]\nprovider = "codex"\naccount = "lab"\nroot = "/abs/sessions"\n'
    )

    loaded = settings.load(config, {"HOME": str(tmp_path)})

    assert loaded.db == tmp_path / "data" / "u.db"
    assert loaded.pr_repos == ("acme/widgets",)
    assert loaded.pr_days == 14
    assert loaded.sources == (
        settings.Source("claude", "work", tmp_path / "w" / "projects", tmp_path / "w"),
        settings.Source("codex", "lab", Path("/abs/sessions"), Path("/abs")),
    )


def test_settings_rejects_unknown_provider(tmp_path):
    config = tmp_path / "usage.toml"
    config.write_text('[[sources]]\nprovider = "gemini"\naccount = "a"\nroot = "/x"\n')

    with pytest.raises(settings.UsageConfigError, match="provider"):
        settings.load(config, {"HOME": str(tmp_path)})


def test_settings_rejects_malformed_file(tmp_path):
    config = tmp_path / "usage.toml"
    config.write_text("pr_days = [")

    with pytest.raises(settings.UsageConfigError):
        settings.load(config, {"HOME": str(tmp_path)})


def test_open_db_migrates_version_one_view_and_adds_cwd_index(tmp_path):
    path = tmp_path / "usage.db"
    conn = store.open_db(path)
    conn.executescript(
        "DROP VIEW pr_turns; DROP TABLE budget_holds; DROP TABLE budget_alerts; DROP INDEX turns_cwd; DROP TABLE quota_snapshots;"
        " DROP TABLE limit_events;"
        " CREATE VIEW pr_turns AS SELECT 1 AS legacy; PRAGMA user_version = 1;")
    conn.close()

    migrated = store.open_db(path)

    assert migrated.execute("PRAGMA user_version").fetchone()[0] == 5
    columns = [row[1] for row in migrated.execute("PRAGMA table_info(pr_turns)")]
    assert "legacy" not in columns and "number" in columns
    indexes = {row[1] for row in migrated.execute("PRAGMA index_list(turns)")}
    assert "turns_cwd" in indexes


def test_open_db_rechecks_the_version_inside_the_write_transaction(tmp_path, monkeypatch):
    path = tmp_path / "usage.db"
    store.open_db(path).close()
    real = store._user_version
    reads = []

    def stale_first(conn):
        reads.append(1)
        return 0 if len(reads) == 1 else real(conn)

    monkeypatch.setattr(store, "_user_version", stale_first)

    conn = store.open_db(path)

    assert conn.execute("PRAGMA user_version").fetchone()[0] == 5
    assert len(reads) == 2


def test_open_db_migrates_v2_to_v3_and_rescans_claude_files(tmp_path):
    path = tmp_path / "usage.db"
    conn = store.open_db(path)
    conn.executescript("DROP TABLE budget_holds; DROP TABLE budget_alerts; DROP TABLE quota_snapshots; DROP TABLE limit_events;"
                       " PRAGMA user_version = 2;")
    insert = "INSERT INTO files (path, provider, account, offset, size, mtime_ns) VALUES (?, ?, 'a', 9, 9, 9)"
    conn.execute(insert, ("/c.jsonl", "claude"))
    conn.execute(insert, ("/x.jsonl", "codex"))
    conn.commit()
    conn.close()

    migrated = store.open_db(path)

    assert migrated.execute("PRAGMA user_version").fetchone()[0] == 5
    files = dict((row[0], row[1:]) for row in migrated.execute("SELECT path, offset, size, mtime_ns FROM files"))
    assert files == {"/c.jsonl": (0, 0, 0), "/x.jsonl": (9, 9, 9)}
    tables = {row[0] for row in migrated.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
    assert {"quota_snapshots", "limit_events"} <= tables
    indexes = {row[1] for row in migrated.execute("PRAGMA index_list(limit_events)")}
    assert "limit_events_ts" in indexes


def test_open_db_migrates_v1_through_v3(tmp_path):
    path = tmp_path / "usage.db"
    conn = store.open_db(path)
    conn.executescript(
        "DROP VIEW pr_turns; DROP TABLE budget_holds; DROP TABLE budget_alerts; DROP INDEX turns_cwd; DROP TABLE quota_snapshots;"
        " DROP TABLE limit_events;"
        " CREATE VIEW pr_turns AS SELECT 1 AS legacy; PRAGMA user_version = 1;")
    conn.close()

    migrated = store.open_db(path)

    assert migrated.execute("PRAGMA user_version").fetchone()[0] == 5
    assert migrated.execute("SELECT COUNT(*) FROM quota_snapshots").fetchone()[0] == 0


def write_config(tmp_path, text):
    config = tmp_path / "usage.toml"
    config.write_text(text)
    return settings.load(config, {"HOME": str(tmp_path)})


def test_settings_reads_plans_and_weights(tmp_path):
    loaded = write_config(
        tmp_path,
        'claude_bin = "~/.local/bin/claude"\n'
        '[[sources]]\nprovider = "claude"\naccount = "personal"\nroot = "/h/.claude/projects"\n'
        '[[plans]]\naccount = "personal"\nplan = "max20x"\nsince = "2026-09-01"\nestimated = true\n'
        '[[plans]]\naccount = "personal"\nplan = "max5x"\nsince = 2026-11-02T08:30:00Z\n'
        '[plan_weights]\nmax20x = 20\nmax5x = 5\npro = 1.0\n')

    assert loaded.plans == (
        settings.Plan("personal", "max20x", datetime(2026, 9, 1, tzinfo=timezone.utc), True),
        settings.Plan("personal", "max5x", datetime(2026, 11, 2, 8, 30, tzinfo=timezone.utc)),
    )
    assert dict(loaded.plan_weights) == {"max20x": 20.0, "max5x": 5.0, "pro": 1.0}
    assert loaded.claude_bin == str(tmp_path / ".local" / "bin" / "claude")


def test_settings_plan_defaults(tmp_path):
    loaded = settings.load(tmp_path / "missing.toml", {"HOME": str(tmp_path)})

    assert (loaded.plans, dict(loaded.plan_weights), loaded.claude_bin) == ((), {}, "claude")


def test_settings_rejects_a_bad_plan_date_and_weight(tmp_path):
    with pytest.raises(settings.UsageConfigError, match="since"):
        write_config(tmp_path, '[[plans]]\naccount = "a"\nplan = "pro"\nsince = "last week"\n')
    with pytest.raises(settings.UsageConfigError, match="estimated"):
        write_config(tmp_path, '[[plans]]\naccount = "a"\nplan = "pro"\nsince = "2026-01-01"\nestimated = "yes"\n')
    with pytest.raises(settings.UsageConfigError, match="plan_weights"):
        write_config(tmp_path, '[plan_weights]\npro = 0\n')


def test_plan_at_picks_the_period():
    plans = (
        settings.Plan("a", "max20x", datetime(2026, 9, 1, tzinfo=timezone.utc)),
        settings.Plan("a", "max5x", datetime(2026, 11, 2, tzinfo=timezone.utc)),
        settings.Plan("b", "pro", datetime(2026, 1, 1, tzinfo=timezone.utc)),
    )
    at = lambda day: datetime(2026, 11, day, tzinfo=timezone.utc)

    assert settings.plan_at(plans, "a", at(1)) == "max20x"
    assert settings.plan_at(plans, "a", at(2)) == "max5x"
    assert settings.plan_at(plans, "a", datetime(2026, 8, 1, tzinfo=timezone.utc)) is None
    assert settings.plan_at(plans, "c", at(1)) is None


def test_claude_source_config_dir_defaults_to_parent(tmp_path):
    loaded = write_config(
        tmp_path,
        '[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "/h/.claude/projects"\n'
        '[[sources]]\nprovider = "claude"\naccount = "b"\nroot = "~/alt/projects"\nconfig_dir = "~/alt"\n'
        '[[sources]]\nprovider = "codex"\naccount = "c"\nroot = "/h/sessions"\n')

    assert [source.config_dir for source in loaded.sources] == [Path("/h/.claude"), tmp_path / "alt", Path("/h")]


def test_a_source_whose_root_is_not_its_config_dir_plus_the_transcript_subdir_is_refused(tmp_path):
    # The registry derives the transcript root from config_dir (ledger ruling A6); a layout it cannot express is an error.
    with pytest.raises(settings.UsageConfigError, match="add config_dir"):
        write_config(tmp_path, '[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "/h/custom/transcripts"\n')
    with pytest.raises(settings.UsageConfigError, match="config_dir plus"):
        write_config(tmp_path, '[[sources]]\nprovider = "claude"\naccount = "b"\nroot = "/h/custom/transcripts"\n'
                               'config_dir = "/h/cfg"\n')


def test_open_db_migrates_v3_to_v4_and_rescans_claude_files_again(tmp_path):
    path = tmp_path / "usage.db"
    conn = store.open_db(path)
    conn.executescript("DROP TABLE budget_holds; DROP TABLE budget_alerts; ALTER TABLE quota_snapshots DROP COLUMN overage_in_use;"
                       " PRAGMA user_version = 3;")
    insert = "INSERT INTO files (path, provider, account, offset, size, mtime_ns) VALUES (?, ?, 'a', 9, 9, 9)"
    conn.execute(insert, ("/c.jsonl", "claude"))
    conn.execute(insert, ("/x.jsonl", "codex"))
    conn.commit()
    conn.close()

    migrated = store.open_db(path)

    assert migrated.execute("PRAGMA user_version").fetchone()[0] == 5
    files = dict((row[0], row[1:]) for row in migrated.execute("SELECT path, offset, size, mtime_ns FROM files"))
    assert files == {"/c.jsonl": (0, 0, 0), "/x.jsonl": (9, 9, 9)}


V4_SNAPSHOT = ("work", "2026-10-28T12:00:00+00:00", "max", "allowed", 10.0, "2026-10-28T17:00:00Z", 32.0,
               "2026-11-03T08:00:00Z")


def test_open_db_migrates_v4_to_v5_in_place_keeping_rows(tmp_path):
    path = tmp_path / "usage.db"
    conn = store.open_db(path)
    conn.executescript("DROP TABLE budget_holds; DROP TABLE budget_alerts; ALTER TABLE quota_snapshots DROP COLUMN overage_in_use;"
                       " PRAGMA user_version = 4;")
    conn.execute("INSERT INTO quota_snapshots VALUES (?, ?, ?, ?, ?, ?, ?, ?)", V4_SNAPSHOT)
    conn.execute("INSERT INTO meta VALUES ('k', 'v')")
    conn.commit()
    conn.close()

    migrated = store.open_db(path)

    assert migrated.execute("PRAGMA user_version").fetchone()[0] == 5
    assert migrated.execute("SELECT * FROM quota_snapshots").fetchall() == [(*V4_SNAPSHOT, None)]
    assert migrated.execute("SELECT value FROM meta WHERE key = 'k'").fetchone() == ("v",)
    assert migrated.execute("SELECT COUNT(*) FROM budget_holds").fetchone() == (0,)
    assert migrated.execute("SELECT COUNT(*) FROM budget_alerts").fetchone() == (0,)
