"""Private SQLite storage for scheduled definitions and approvals."""

from __future__ import annotations

import json
import os
import sqlite3
import stat
import sys
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from secrets import token_hex

from sakura import engine_home
from sakura.connectors import kinds, registry
from sakura.flow import registry as projects
from sakura.flow.render import strip_controls
from sakura.scheduled import definition
from sakura.scheduled.definition import Refused

SCHEMA = (
    ("CREATE TABLE tasks (id TEXT PRIMARY KEY, revision INTEGER NOT NULL, definition TEXT NOT NULL,"
     " anchor TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0)"),
    ("CREATE TABLE approvals (id TEXT PRIMARY KEY, revision TEXT NOT NULL, definition TEXT NOT NULL,"
     " author TEXT NOT NULL, created_at TEXT NOT NULL, decision TEXT)"),
    ("CREATE TABLE operations (kind TEXT NOT NULL, op TEXT NOT NULL, request TEXT NOT NULL,"
     " result TEXT NOT NULL, PRIMARY KEY (kind, op))"),
)
DISPATCH_SCHEMA = (
    "ALTER TABLE tasks ADD COLUMN frontier TEXT",
    "ALTER TABLE tasks ADD COLUMN connector_pause TEXT",
    ("CREATE TABLE runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, task_revision TEXT NOT NULL,"
     " definition TEXT NOT NULL, occurrence TEXT NOT NULL, trigger TEXT NOT NULL, queued_at TEXT NOT NULL,"
     " state TEXT NOT NULL, reason TEXT, started_at TEXT, ended_at TEXT, UNIQUE (task_id, occurrence))"),
    "CREATE INDEX runs_task ON runs(task_id)",
    ("CREATE TABLE checkpoints (connector_id TEXT, revision TEXT, kind TEXT, value TEXT NOT NULL,"
     " PRIMARY KEY (connector_id,revision,kind))"),
    ("CREATE TABLE event_receipts (connector_id TEXT, revision TEXT, kind TEXT, occurrence TEXT,"
     " PRIMARY KEY (connector_id,revision,kind,occurrence))"),
    ("CREATE TABLE subscriptions (task_id TEXT, anchor TEXT, revision TEXT, kind TEXT,"
     " PRIMARY KEY (task_id,anchor,revision,kind))"),
)
RUNNER_SCHEMA = (
    "ALTER TABLE runs ADD COLUMN process TEXT",
    "ALTER TABLE runs ADD COLUMN grant TEXT",
    ("CREATE TABLE conversation (sequence INTEGER PRIMARY KEY AUTOINCREMENT,"
     " run_id TEXT NOT NULL, kind TEXT NOT NULL, text TEXT NOT NULL)"),
    "CREATE INDEX frames_by_run ON conversation(run_id,sequence)",
)
FINDING_SCHEMA = (
    ("CREATE TABLE run_evidence (run_id TEXT, reference TEXT, evidence TEXT NOT NULL,"
     " PRIMARY KEY (run_id,reference))"),
    ("CREATE TABLE staged_findings (run_id TEXT, namespace TEXT, item TEXT, version TEXT, payload TEXT NOT NULL,"
     " PRIMARY KEY (run_id,namespace,item,version))"),
    ("CREATE TABLE findings (id TEXT PRIMARY KEY, namespace TEXT NOT NULL, item TEXT NOT NULL,"
     " payload TEXT NOT NULL, active INTEGER NOT NULL)"),
    "CREATE UNIQUE INDEX active_source_item ON findings(namespace,item) WHERE active",
    ("CREATE TABLE finding_receipts (namespace TEXT, item TEXT, version TEXT, finding_id TEXT NOT NULL,"
     " PRIMARY KEY (namespace,item,version))"),
    ("CREATE TABLE finding_origins (finding_id TEXT, run_id TEXT, origin TEXT NOT NULL,"
     " PRIMARY KEY (finding_id,run_id))"),
)
MIGRATION_SCHEMA = (
    ("CREATE TABLE digest_migration (singleton INTEGER PRIMARY KEY CHECK(singleton=1),"
     " phase TEXT NOT NULL, manifest TEXT NOT NULL, problem TEXT)"),
    "CREATE TABLE digest_checkpoints (source_id TEXT PRIMARY KEY, cursor TEXT NOT NULL, at TEXT NOT NULL)",
    "CREATE TABLE digest_run_updates (run_id TEXT PRIMARY KEY, state TEXT NOT NULL)",
    "CREATE TABLE imported_connectors (id TEXT PRIMARY KEY, definition TEXT NOT NULL, disconnected INTEGER NOT NULL)",
    "CREATE TABLE retired_automations (name TEXT PRIMARY KEY, reason TEXT NOT NULL)",
)
SCHEMAS = (SCHEMA, DISPATCH_SCHEMA, RUNNER_SCHEMA, FINDING_SCHEMA, MIGRATION_SCHEMA)
STORE_PROBLEM = "scheduled_store_unavailable"
DATABASE = "tasks.sqlite3"


@dataclass(frozen=True)
class ScheduledTask:
    id: str
    revision: str
    definition: dict
    anchor: str

    connector_pause: dict | None = None
    replayed_run: str | None = None

    kind = definition.TASK_KIND

    @property
    def key(self) -> tuple[str, str, str]:
        return self.kind, self.id, self.revision

    @property
    def repo(self) -> Path:
        return Path("/")


    def status(self, available: dict) -> tuple[str, str | None, str | None]:
        """Combine the operator setting with any explicit connector pause."""
        state, reason, label = definition.effective(self.definition, available)
        if self.connector_pause:
            if state == "active":
                return "paused", "connector_reconnected", self.connector_pause["label"]
            if state == "connector_unavailable" and label is None:
                label = self.connector_pause["label"]
        return state, reason, label


@dataclass(frozen=True)
class Approval:
    id: str
    revision: str
    definition: dict
    author: str
    created_at: str

    @property
    def preview(self) -> str:
        return json.dumps(self.definition, ensure_ascii=True, sort_keys=True, indent=2)


def stamp(now: datetime) -> str:
    """Normalize engine timestamps to UTC with fixed-width precision."""
    return now.astimezone(UTC).isoformat(timespec="microseconds")


def _task(row: sqlite3.Row) -> ScheduledTask:
    return ScheduledTask(row["id"], str(row["revision"]), json.loads(row["definition"]), row["anchor"],
                         json.loads(row["connector_pause"]) if dict(row).get("connector_pause") else None)


def _private(path: Path, kind: int, mode: int) -> None:
    info = path.lstat()
    if stat.S_IFMT(info.st_mode) != kind or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & ~mode:
        raise Refused(STORE_PROBLEM)


class Store:
    """Read live tasks without creating storage. Serialize mutations with short transactions."""

    def __init__(self, path: Path | None = None):
        self.path = path or engine_home.root() / "scheduled" / DATABASE

    @contextmanager
    def connect(self, *, write: bool = False):
        """Open a private connection, initializing or upgrading it only for a writer."""
        database = None
        try:
            if write:
                self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                _private(self.path.parent, stat.S_IFDIR, 0o700)
                fd = os.open(self.path, os.O_CREAT | os.O_NOFOLLOW | os.O_WRONLY | os.O_NONBLOCK, 0o600)
                os.close(fd)
            _private(self.path.parent, stat.S_IFDIR, 0o700)
            _private(self.path, stat.S_IFREG, 0o600)
            database = sqlite3.connect(self.path.as_uri() + ("?mode=rw" if write else "?mode=ro"), uri=True)
            database.row_factory = sqlite3.Row
            with database:
                database.execute("BEGIN IMMEDIATE" if write else "BEGIN")
                self._schema(database, write)
                yield database
        except (sqlite3.Error, OSError, kinds.ConnectorError, projects.RegistryError) as error:
            raise Refused(STORE_PROBLEM) from error
        finally:
            if database is not None:
                database.close()

    @staticmethod
    def _schema(db: sqlite3.Connection, write: bool) -> None:
        version = db.execute("PRAGMA user_version").fetchone()[0]
        if version > len(SCHEMAS) or (not write and version == 0):
            raise Refused(STORE_PROBLEM)
        if write:
            for number, statements in enumerate(SCHEMAS[version:], version + 1):
                for ddl in statements:
                    db.execute(ddl)
                db.execute(f"PRAGMA user_version = {number}")

    def tasks(self) -> list[ScheduledTask]:
        """Read live definitions in creation order."""
        if not self.path.exists() and not self.path.is_symlink():
            return []
        with self.connect() as db:
            return [_task(row) for row in db.execute("SELECT * FROM tasks WHERE NOT deleted ORDER BY rowid")]

    def get(self, task_id: str) -> ScheduledTask:
        """Read a live definition by its opaque ID."""
        if not self.path.exists() and not self.path.is_symlink():
            raise Refused("stale_task")
        with self.connect() as db:
            return self._get(db, task_id)

    @staticmethod
    def _get(db: sqlite3.Connection, task_id: str, revision: str | None = None) -> ScheduledTask:
        row = db.execute("SELECT * FROM tasks WHERE id = ? AND NOT deleted", (task_id,)).fetchone()
        if row is None or (revision is not None and str(row["revision"]) != revision):
            raise Refused("stale_task")
        return _task(row)

    def create(self, value: dict, op: str, now: datetime) -> ScheduledTask:
        """Create once per operation and canonical request."""
        self._valid_op(op)
        data = definition.validate(value, choices=False)
        request = definition.encode(data)
        with registry.admission_lock():
            if self.path.exists():
                with self.connect() as db:
                    prior = self._operation(db, definition.CREATE, op, request)
                if prior:
                    return ScheduledTask(**json.loads(prior))
            definition.validate_choices(data)
            with self.connect(write=True) as db:
                made = self._insert(db, token_hex(16), data, now)
                result = definition.encode({"id": made.id, "revision": made.revision, "definition": data,
                                            "anchor": made.anchor})
                db.execute("INSERT INTO operations VALUES (?, ?, ?, ?)", (definition.CREATE, op, request, result))
                return made

    @staticmethod
    def _valid_op(op: str) -> None:
        if not isinstance(op, str) or not definition.OP.fullmatch(op):
            raise Refused("invalid_operation")

    @staticmethod
    def _operation(db: sqlite3.Connection, kind: str, op: str, request: str) -> str | None:
        prior = db.execute("SELECT request, result FROM operations WHERE kind=? AND op=?", (kind, op)).fetchone()
        if prior and prior["request"] != request:
            raise Refused("operation_conflict")
        return prior["result"] if prior else None

    @staticmethod
    def _insert(db: sqlite3.Connection, task_id: str, data: dict, now: datetime) -> ScheduledTask:
        anchor = stamp(now)
        db.execute("INSERT INTO tasks (id, revision, definition, anchor) VALUES (?, 1, ?, ?)",
                   (task_id, definition.encode(data), anchor))
        return ScheduledTask(task_id, "1", data, anchor)

    def change(self, task_id: str, revision: str, action: str, inputs: dict, now: datetime) -> ScheduledTask:
        """Apply one mutation against the exact current revision."""
        with registry.admission_lock():
            with self.connect() as db:
                task = self._get(db, task_id, revision)
                if task.definition.get("legacy_digest"):
                    from sakura.scheduled import migrate

                    phase = db.execute("SELECT phase FROM digest_migration WHERE singleton=1").fetchone()
                    if phase is None or phase[0] != "active":
                        raise Refused(migrate.TASK_UNAVAILABLE)
            replacement = definition.validate(definition.decode(inputs["definition"])) if action == definition.UPDATE else None
            current = task.definition
            resume_connector = action == definition.RESUME and task.connector_pause is not None
            data = self._changed({**current, "enabled": False} if resume_connector else current, action, replacement)
            legacy = data.get("legacy_dispatch")
            if data["enabled"] and legacy is not None and not (legacy["base"] or legacy["aliases"]):
                data = {**data, "legacy_dispatch": {**legacy, "base": True}}
            is_reset = (resume_connector or data["when"] != current["when"] or data["enabled"] != current["enabled"])
            pause = task.connector_pause if action == definition.UPDATE and data["enabled"] else None
            anchor = stamp(now) if is_reset else task.anchor
            index = int(revision) + 1
            with self.connect(write=True) as db:
                self._get(db, task_id, revision)
                db.execute("UPDATE tasks SET revision=?, definition=?, anchor=?, deleted=?,"
                           " frontier=CASE WHEN ? THEN NULL ELSE frontier END, connector_pause=? WHERE id=?",
                           (index, definition.encode(data), anchor, action == definition.DELETE, is_reset,
                            definition.encode(pause) if pause else None, task_id))
                self._skip_queued(db, task_id, now, "task_changed")
            return ScheduledTask(task_id, str(index), data, anchor, pause)

    @staticmethod
    def _skip_queued(db, task_id: str, now: datetime, reason: str) -> None:
        db.execute("UPDATE runs SET state='skipped', reason=?, ended_at=? WHERE task_id=? AND state='queued'",
                   (reason, stamp(now), task_id))

    def connector_disconnected(self, connector, now: datetime) -> None:
        """Pause affected tasks while the caller holds the connector admission lock."""
        if not self.path.exists():
            return
        with self.connect(write=True) as db:
            for row in db.execute("SELECT * FROM tasks WHERE NOT deleted"):
                task = _task(row)
                if connector.id in definition.required_connectors(task.definition):
                    self._pause_connector(db, task, connector.id, connector.label, now)

    def pause_connector(self, task_id: str, connector_id: str, label: str | None, now: datetime) -> None:
        """Pause the task for a verified dependency loss while the registry is locked."""
        with self.connect(write=True) as db:
            self._pause_connector(db, self._get(db, task_id), connector_id, label, now)

    def _pause_connector(self, db, task: ScheduledTask, connector_id: str, label: str | None, now: datetime) -> None:
        if not task.definition["enabled"] or (task.connector_pause or {}).get("id") == connector_id:
            return
        issue = definition.encode({"id": connector_id, "label": label or connector_id})
        db.execute("UPDATE tasks SET revision=revision+1, connector_pause=? WHERE id=?", (issue, task.id))
        self._skip_queued(db, task.id, now, "connector_unavailable")

    @staticmethod
    def _changed(data: dict, action: str, replacement: dict | None) -> dict:
        if action == definition.UPDATE:
            if data.get("legacy_digest"):
                internal = {key: value for key, value in data.items() if key.startswith("legacy_")}
                if data["when"] != replacement["when"]:
                    internal.pop("legacy_dispatch", None)
                return {**replacement, **internal}
            return replacement
        if action == definition.DELETE:
            return data
        if action == definition.PAUSE and data["enabled"]:
            return {**data, "enabled": False}
        if action == definition.RESUME and not data["enabled"]:
            definition.validate_choices(data)
            available = {item.id: item for item in registry.load()}
            enabled = {**data, "enabled": True}
            if definition.effective(enabled, available)[0] == "active":
                return enabled
            raise Refused("connector_unavailable")
        raise Refused("action_unavailable")

    def propose(self, value: dict, author: str, now: datetime) -> Approval:
        """Stage the exact normalized definition for a human decision."""
        data = definition.validate(value, proposed=True)
        pending = Approval(token_hex(16), token_hex(16), data, author, stamp(now))
        with self.connect(write=True) as db:
            db.execute("INSERT INTO approvals VALUES (?, ?, ?, ?, ?, NULL)",
                       (pending.id, pending.revision, definition.encode(data), author, pending.created_at))
        return pending

    def approvals(self) -> list[Approval]:
        """Read undecided approvals in creation order."""
        if not self.path.exists() and not self.path.is_symlink():
            return []
        with self.connect() as db:
            return [Approval(row["id"], row["revision"], json.loads(row["definition"]), row["author"], row["created_at"])
                    for row in db.execute("SELECT * FROM approvals WHERE decision IS NULL ORDER BY rowid")]

    def decide(self, proposal_id: str, revision: str, accepted: bool, now: datetime) -> ScheduledTask | None:
        """Consume the staged revision and optionally create its task in the same transaction."""
        with registry.admission_lock():
            with self.connect() as db:
                row = db.execute("SELECT definition FROM approvals WHERE id=? AND revision=? AND decision IS NULL",
                                 (proposal_id, revision)).fetchone()
            if row is None:
                raise Refused("stale_approval")
            data = json.loads(row["definition"])
            if accepted:
                definition.validate_choices(data)
            with self.connect(write=True) as db:
                changed = db.execute("UPDATE approvals SET decision=? WHERE id=? AND revision=? AND decision IS NULL",
                                     ("accepted" if accepted else "rejected", proposal_id, revision))
                if not changed.rowcount:
                    raise Refused("stale_approval")
                if accepted and "legacy_alias" in data:
                    from sakura.scheduled import migrate

                    return migrate.accept_imported(db, data, now)
                return self._insert(db, proposal_id, data, now) if accepted else None


def note_problem(code: str, error: Exception) -> None:
    """Write bounded diagnostics to the engine process's local stderr."""
    print(f"scheduled: {code}: {strip_controls(str(error))[:320]}", file=sys.stderr)
