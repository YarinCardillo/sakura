"""SQLite connections and schema for the Track store."""

import os
import sqlite3
import stat
from pathlib import Path

from sakura import engine_home
from sakura.tracks.domain import (
    ANSWER_LIMIT,
    DECISION_LIMIT,
    GOAL_LIMIT,
    NAME_LIMIT,
    SUMMARY_LIMIT,
)

SCHEMA_VERSION = 2
BUSY_TIMEOUT_MS = 5000
PRIVATE_FILE_MODE = 0o600
PRIVATE_DIR_MODE = 0o700

SCHEMA = f"""
CREATE TABLE tracks (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 10 AND substr(id, 1, 2) = 't-'
        AND substr(id, 3) NOT GLOB '*[^0-9a-f]*'),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND {NAME_LIMIT}),
    name_key TEXT NOT NULL CHECK (name_key <> '' AND name_key = track_name_key(name)),
    goal TEXT NOT NULL CHECK (length(goal) <= {GOAL_LIMIT} AND track_name_key(goal) <> ''),
    status TEXT NOT NULL CHECK (status IN ('open', 'paused', 'closed')),
    summary TEXT NOT NULL CHECK (length(summary) <= {SUMMARY_LIMIT}),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX active_track_names ON tracks(name_key) WHERE status <> 'closed';

CREATE TABLE track_projects (
    track_id TEXT NOT NULL REFERENCES tracks(id),
    project_id TEXT NOT NULL,
    folder TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (track_id, project_id)
);
CREATE TABLE track_lanes (
    project_id TEXT NOT NULL,
    task TEXT NOT NULL,
    track_id TEXT NOT NULL,
    title TEXT NOT NULL,
    start_op TEXT UNIQUE,
    PRIMARY KEY (project_id, task),
    FOREIGN KEY (track_id, project_id) REFERENCES track_projects(track_id, project_id)
);
CREATE INDEX lanes_by_track ON track_lanes(track_id, project_id);

CREATE TABLE track_conversations (
    provider TEXT NOT NULL,
    account TEXT NOT NULL,
    session TEXT NOT NULL,
    track_id TEXT NOT NULL REFERENCES tracks(id),
    title TEXT NOT NULL,
    PRIMARY KEY (provider, account, session)
);
CREATE INDEX conversations_by_track ON track_conversations(track_id);

CREATE TABLE track_decisions (
    track_id TEXT NOT NULL REFERENCES tracks(id),
    number INTEGER NOT NULL CHECK (typeof(number) = 'integer' AND number > 0),
    text TEXT NOT NULL CHECK (length(text) <= {DECISION_LIMIT} AND track_name_key(text) <> ''),
    state TEXT NOT NULL,
    answer TEXT CHECK (length(answer) <= {ANSWER_LIMIT} AND track_name_key(answer) <> ''),
    created_at TEXT NOT NULL,
    settled_at TEXT,
    PRIMARY KEY (track_id, number),
    CHECK ((state = 'open' AND answer IS NULL AND settled_at IS NULL)
        OR (state = 'settled' AND answer IS NOT NULL AND settled_at IS NOT NULL AND settled_at <> ''))
);

CREATE TABLE track_audit (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id TEXT NOT NULL REFERENCES tracks(id),
    at TEXT NOT NULL,
    actor TEXT NOT NULL,
    actor_class TEXT NOT NULL CHECK (actor_class IN ('person', 'master', 'engine')),
    actor_session TEXT,
    place TEXT,
    operation TEXT NOT NULL,
    op_id TEXT NOT NULL,
    before TEXT NOT NULL,
    after TEXT NOT NULL
);
CREATE INDEX audit_by_track ON track_audit(track_id, seq);
CREATE TRIGGER audit_no_update BEFORE UPDATE ON track_audit
    BEGIN SELECT RAISE(ABORT, 'Track audit is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON track_audit
    BEGIN SELECT RAISE(ABORT, 'Track audit is append-only'); END;

CREATE TABLE track_operations (
    op_id TEXT PRIMARY KEY NOT NULL,
    actor_scope TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result TEXT NOT NULL,
    at TEXT NOT NULL
);
"""

STARTS_SCHEMA = """
CREATE TABLE track_starts (
    op_id TEXT PRIMARY KEY NOT NULL REFERENCES track_operations(op_id) DEFERRABLE INITIALLY DEFERRED,
    track_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    task TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    inputs TEXT NOT NULL,
    actor TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'interrupted', 'started', 'removed')),
    FOREIGN KEY (track_id, project_id) REFERENCES track_projects(track_id, project_id)
);
"""

MIGRATIONS = {0: SCHEMA, 1: STARTS_SCHEMA}


class SchemaError(RuntimeError):
    """The database has a schema the running engine cannot use."""


class Database:
    """A connection factory for one engine's Track store."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path if path is not None else engine_home.root() / "tracks" / "tracks.db"

    def connect(self, write: bool, *, create: bool = True) -> sqlite3.Connection | None:
        """Open a short-lived connection. A read of an absent database returns None."""
        if write and create:
            self.path.parent.mkdir(parents=True, exist_ok=True, mode=PRIVATE_DIR_MODE)
        try:
            _require_private(self.path.parent, stat.S_ISDIR, PRIVATE_DIR_MODE)
            if write and create:
                os.close(os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, PRIVATE_FILE_MODE))
            _require_private(self.path, stat.S_ISREG, PRIVATE_FILE_MODE)
        except FileNotFoundError:
            if write and create:
                raise
            return None
        mode = "rw" if write else "ro"
        conn = sqlite3.connect(f"{self.path.absolute().as_uri()}?mode={mode}", uri=True, isolation_level=None,
                               timeout=BUSY_TIMEOUT_MS / 1000)
        try:
            conn.row_factory = sqlite3.Row
            conn.create_function("track_name_key", 1, name_key, deterministic=True)
            conn.execute("PRAGMA foreign_keys = ON")
            conn.execute("PRAGMA recursive_triggers = ON")
            if not write and _version(conn) in MIGRATIONS:
                self.connect(True, create=False).close()
            _prepare(conn, write)
        except BaseException:
            conn.close()
            raise
        return conn


def name_key(text: str | None) -> str | None:
    """Normalize Unicode names for SQLite's unique index."""
    return None if text is None else text.strip().casefold()


def _require_private(path, is_kind, mode):
    info = path.lstat()
    if not is_kind(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != mode:
        raise SchemaError("Track storage must be private to the engine user")


def _prepare(conn: sqlite3.Connection, write: bool) -> None:
    version = _version(conn)
    if version != SCHEMA_VERSION:
        if not write or version not in MIGRATIONS:
            raise SchemaError("Unsupported Track database version")
        conn.execute("BEGIN IMMEDIATE")
        with conn:
            _upgrade(conn)
    journal = "journal_mode = DELETE" if write else "journal_mode"
    if conn.execute(f"PRAGMA {journal}").fetchone()[0] != "delete":
        raise SchemaError("Track database requires a rollback journal")


def _upgrade(conn: sqlite3.Connection) -> None:
    version = _version(conn)
    while version != SCHEMA_VERSION:
        if version not in MIGRATIONS:
            raise SchemaError("Unsupported Track database version")
        statement = ""
        for line in MIGRATIONS[version].splitlines(keepends=True):
            statement += line
            if sqlite3.complete_statement(statement):
                conn.execute(statement)
                statement = ""
        version += 1
        conn.execute(f"PRAGMA user_version = {version}")


def _version(conn: sqlite3.Connection) -> int:
    return conn.execute("PRAGMA user_version").fetchone()[0]
