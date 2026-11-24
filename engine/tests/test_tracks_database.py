import sqlite3
import stat
from contextlib import closing
from pathlib import Path

import pytest
from sakura.tracks import TrackService
from sakura.tracks.database import (
    SCHEMA,
    SCHEMA_VERSION,
    Database,
    SchemaError,
    name_key,
)
from sakura.tracks.domain import NAME_LIMIT


def test_missing_reads_leave_engine_home_untouched(tmp_path):
    path = tmp_path / "tracks" / "tracks.db"
    service = TrackService(Database(path).connect)
    assert service.read_all() == ()
    assert service.read_audit("t-00000000") == ()
    assert not path.parent.exists()


def test_private_database_and_connection_settings(tmp_path):
    path = tmp_path / "tracks" / "tracks.db"
    database = Database(path)
    for write in (True, False):
        with closing(database.connect(write)) as connection:
            for pragma, expected in (("user_version", SCHEMA_VERSION), ("journal_mode", "delete"), ("foreign_keys", 1)):
                assert connection.execute(f"PRAGMA {pragma}").fetchone()[0] == expected
            assert 0 < connection.execute("PRAGMA busy_timeout").fetchone()[0] < 60_000
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700


def test_future_version_leaves_database_untouched(tmp_path):
    path = tmp_path / "tracks" / "tracks.db"
    path.parent.mkdir(mode=0o700)
    path.touch(mode=0o600)
    with closing(sqlite3.connect(path)) as connection:
        connection.execute("PRAGMA user_version = 999")
    before = path.read_bytes(), path.stat().st_mtime_ns
    for write in (False, True):
        with pytest.raises(SchemaError):
            Database(path).connect(write)
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before


def test_database_constraints(tmp_path):
    with closing(Database(tmp_path / "tracks" / "tracks.db").connect(True)) as db:
        track = "INSERT INTO tracks VALUES (?, ?, ?, ?, ?, '', 'now', 'now')"
        db.execute(track, ("t-00000001", "Straße", "strasse", "Goal", "open"))
        db.execute(track, ("t-00000002", "STRASSE", "strasse", "Goal", "closed"))
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("UPDATE tracks SET status = 'paused' WHERE id = 't-00000002'")
        for name, key in ((" ", ""), ("x" * (NAME_LIMIT + 1), "x" * (NAME_LIMIT + 1)), ("test", "else")):
            with pytest.raises(sqlite3.IntegrityError):
                db.execute(track, ("t-00000003", name, key, "Goal", "open"))
        db.execute("INSERT INTO track_projects VALUES ('t-00000001', 'project', '/repo', 'Repo')")
        db.execute("INSERT INTO track_projects VALUES ('t-00000002', 'project', '/repo', 'Repo')")
        db.execute("INSERT INTO track_lanes VALUES ('project', 'lane', 't-00000001', 'Lane', NULL)")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO track_lanes VALUES ('project', 'lane', 't-00000002', 'Other', NULL)")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO track_lanes VALUES ('unknown', 'lane', 't-00000001', 'Lane', NULL)")
        db.execute("INSERT INTO track_conversations VALUES ('p', 'a', 's', 't-00000001', 'Chat')")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("INSERT INTO track_conversations VALUES ('p', 'a', 's', 't-00000002', 'Chat')")
        db.execute("INSERT INTO track_decisions VALUES ('t-00000001', 1, 'Question', 'open', NULL, 'then', NULL)")
        for state, answer, at in (("settled", None, "now"), ("settled", "yes", None), ("open", "yes", "now")):
            with pytest.raises(sqlite3.IntegrityError):
                db.execute("UPDATE track_decisions SET state = ?, answer = ?, settled_at = ?", (state, answer, at))


def test_default_database_location():
    from sakura import engine_home

    assert Database().path == Path(engine_home.root()) / "tracks" / "tracks.db"


def test_audit_rows_cannot_be_edited_replaced_or_deleted(tmp_path):
    database = Database(tmp_path / "tracks" / "tracks.db")
    service = TrackService(database.connect)
    from sakura.actors import Actor

    track = service.create("Track", "Ship", actor=Actor("cli", "owner"), op_id="new")
    with closing(database.connect(True)) as db:
        for sql in ("UPDATE track_audit SET at = 'changed'", "DELETE FROM track_audit",
                    "INSERT OR REPLACE INTO track_audit SELECT * FROM track_audit"):
            with pytest.raises(sqlite3.IntegrityError, match="append-only"):
                db.execute(sql)
    assert len(service.read_audit(track.id)) == 1


def test_reservation_schema_upgrade_preserves_existing_records(tmp_path):
    path = tmp_path / "tracks" / "tracks.db"
    path.parent.mkdir(mode=0o700)
    path.touch(mode=0o600)
    with closing(sqlite3.connect(path)) as db, db:
        db.create_function("track_name_key", 1, name_key, deterministic=True)
        db.executescript(SCHEMA)
        db.execute("INSERT INTO tracks VALUES ('t-00000001', 'One', 'one', 'Ship', 'open', '', 'then', 'then')")
        db.execute("PRAGMA user_version = 1")
    service = TrackService(Database(path).connect)
    assert service.read_all()[0].name == "One"
    with closing(Database(path).connect(False)) as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == SCHEMA_VERSION
        assert db.execute("SELECT count(*) FROM track_starts").fetchone()[0] == 0
