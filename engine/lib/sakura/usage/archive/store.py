"""The archive database: schema, private file creation and the single-writer lock."""

from __future__ import annotations

import fcntl
import os
import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

SCHEMA_VERSION = 5
BUSY_TIMEOUT_MS = 30_000
PRIVATE_DIR_MODE = 0o700
PRIVATE_FILE_MODE = 0o600

QUOTA_TABLES = """
CREATE TABLE quota_snapshots (account TEXT NOT NULL, ts TEXT NOT NULL, subscription_type TEXT, status TEXT,
  five_hour_pct REAL, five_hour_resets_at TEXT, seven_day_pct REAL, seven_day_resets_at TEXT,
  PRIMARY KEY (account, ts));
CREATE TABLE limit_events (account TEXT NOT NULL, event_key TEXT NOT NULL, ts TEXT NOT NULL,
  window TEXT NOT NULL, resets_at TEXT, PRIMARY KEY (account, event_key));
CREATE INDEX limit_events_ts ON limit_events (ts);
"""

SCHEMA = """
CREATE TABLE files (path TEXT PRIMARY KEY, provider TEXT NOT NULL, account TEXT NOT NULL,
  offset INTEGER NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, cursor TEXT NOT NULL DEFAULT '{}');
CREATE TABLE turns (provider TEXT NOT NULL, event_key TEXT NOT NULL, account TEXT NOT NULL,
  session_id TEXT NOT NULL, agent_id TEXT, ts TEXT NOT NULL, cwd TEXT, branch TEXT, model TEXT,
  input_tokens INTEGER NOT NULL, cache_write_tokens INTEGER NOT NULL, cache_read_tokens INTEGER NOT NULL,
  output_tokens INTEGER NOT NULL, cost_usd REAL, PRIMARY KEY (provider, event_key));
CREATE INDEX turns_session ON turns (provider, session_id);
CREATE INDEX turns_branch ON turns (branch);
CREATE INDEX turns_ts ON turns (ts);
CREATE TABLE cwds (cwd TEXT PRIMARY KEY, repo TEXT, resolved_at TEXT NOT NULL);
CREATE TABLE pull_requests (repo TEXT NOT NULL, number INTEGER NOT NULL, head_branch TEXT NOT NULL,
  base_branch TEXT NOT NULL, changed_files INTEGER NOT NULL, additions INTEGER NOT NULL,
  deletions INTEGER NOT NULL, commits INTEGER NOT NULL, created_at TEXT NOT NULL, ready_at TEXT,
  merged_at TEXT, closed_at TEXT, is_draft INTEGER NOT NULL, labels TEXT NOT NULL, fetched_at TEXT NOT NULL,
  PRIMARY KEY (repo, number));
CREATE INDEX pull_requests_branch ON pull_requests (head_branch);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE VIEW sessions AS SELECT provider, account, session_id, MIN(ts) AS first_ts, MAX(ts) AS last_ts,
  MIN(cwd) AS cwd, MAX(branch) AS branch, COUNT(*) AS turns, SUM(input_tokens) AS input_tokens,
  SUM(cache_write_tokens) AS cache_write_tokens, SUM(cache_read_tokens) AS cache_read_tokens,
  SUM(output_tokens) AS output_tokens, SUM(cost_usd) AS cost_usd
  FROM turns GROUP BY provider, account, session_id;
CREATE INDEX turns_cwd ON turns (cwd);
""" + QUOTA_TABLES

# A pull request owns the turns on its branch from the close of the previous pull request of the same
# repo and branch up to its own close (open-ended while open). A resolved cwd must belong to the repo; an
# unresolved one must have the repo name as a path component. Times go through datetime() because turns
# store "+00:00" and pull requests "Z".
PR_TURNS_VIEW = """
CREATE VIEW pr_turns AS SELECT p.repo, p.number, t.* FROM turns t
  JOIN pull_requests p ON t.branch = p.head_branch
  LEFT JOIN cwds c ON c.cwd = t.cwd
  WHERE t.cwd IS NOT NULL
    AND (c.repo = substr(p.repo, instr(p.repo, '/') + 1)
         OR (c.repo IS NULL
             AND instr('/' || t.cwd || '/', '/' || substr(p.repo, instr(p.repo, '/') + 1) || '/') > 0))
    AND datetime(t.ts) > COALESCE(
          (SELECT MAX(datetime(COALESCE(q.merged_at, q.closed_at))) FROM pull_requests q
           WHERE q.repo = p.repo AND q.head_branch = p.head_branch
             AND datetime(q.created_at) < datetime(p.created_at)), '0000-01-01 00:00:00')
    AND (COALESCE(p.merged_at, p.closed_at) IS NULL
         OR datetime(t.ts) <= datetime(COALESCE(p.merged_at, p.closed_at)));
"""
MIGRATION_1_TO_2 = "DROP VIEW pr_turns; CREATE INDEX turns_cwd ON turns (cwd);" + PR_TURNS_VIEW
# Zeroed claude rows are rescanned once so transcripts written before this version yield their limit
# events; turns upsert idempotently.
MIGRATION_2_TO_3 = QUOTA_TABLES + "UPDATE files SET offset = 0, size = 0, mtime_ns = 0 WHERE provider = 'claude';"
# Rescan claude files once more so legacy limit hits (no quotaLimits) written before this version are read.
MIGRATION_3_TO_4 = "UPDATE files SET offset = 0, size = 0, mtime_ns = 0 WHERE provider = 'claude';"
# `overage_in_use` is NULL when the probe did not say, 1 or 0 when it did.
# New tables of version 5 are appended to this script; fresh databases and v4 databases run it alike.
MIGRATION_4_TO_5 = """
ALTER TABLE quota_snapshots ADD COLUMN overage_in_use INTEGER;
CREATE TABLE budget_holds (task_key TEXT PRIMARY KEY, account TEXT NOT NULL, week_pct REAL NOT NULL,
  created_at TEXT NOT NULL, week_at_hold REAL);
CREATE TABLE budget_alerts (id TEXT PRIMARY KEY, account TEXT NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
  reason TEXT NOT NULL, since TEXT NOT NULL, resolved_at TEXT, url TEXT, pushed_at TEXT);
"""


def open_db(path: Path) -> sqlite3.Connection:
    """Open the archive at `path`, creating it private (0700 directory, 0600 file) with the schema."""
    path.parent.mkdir(parents=True, exist_ok=True, mode=PRIVATE_DIR_MODE)
    _create_private_file(path)
    conn = sqlite3.connect(path, timeout=BUSY_TIMEOUT_MS / 1000)
    conn.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}")
    conn.execute("PRAGMA journal_mode = WAL")
    if _user_version(conn) < SCHEMA_VERSION:
        _install_schema(conn)
    return conn


@contextmanager
def archive_lock(path: Path) -> Iterator[bool]:
    """Hold a non-blocking exclusive lock beside the database; yields False when another run holds it."""
    path.parent.mkdir(parents=True, exist_ok=True, mode=PRIVATE_DIR_MODE)
    descriptor = os.open(f"{path}.lock", os.O_RDWR | os.O_CREAT, PRIVATE_FILE_MODE)
    try:
        held = _try_lock(descriptor)
        yield held
    finally:
        os.close(descriptor)


def _try_lock(descriptor: int) -> bool:
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return False
    return True


def _create_private_file(path: Path) -> None:
    os.close(os.open(path, os.O_RDWR | os.O_CREAT, PRIVATE_FILE_MODE))


def _user_version(conn: sqlite3.Connection) -> int:
    return conn.execute("PRAGMA user_version").fetchone()[0]


def _install_schema(conn: sqlite3.Connection) -> None:
    """Install or migrate under a write lock, re-reading the version so two openers cannot both install."""
    conn.isolation_level = None
    try:
        conn.execute("BEGIN IMMEDIATE")
        try:
            version = _user_version(conn)
            if version < SCHEMA_VERSION:
                _apply(conn, version)
        except BaseException:
            conn.execute("ROLLBACK")
            raise
        conn.execute("COMMIT")
    finally:
        conn.isolation_level = ""


def _apply(conn: sqlite3.Connection, version: int) -> None:
    script = SCHEMA + PR_TURNS_VIEW + MIGRATION_4_TO_5 if version == 0 else _migration_from(version)
    for statement in _statements(script):
        conn.execute(statement)
    conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")


def _migration_from(version: int) -> str:
    steps = ((2, MIGRATION_1_TO_2), (3, MIGRATION_2_TO_3), (4, MIGRATION_3_TO_4), (5, MIGRATION_4_TO_5))
    return "".join(script for target, script in steps if version < target)


def _statements(script: str) -> list[str]:
    return [piece.strip() for piece in script.split(";") if piece.strip()]
