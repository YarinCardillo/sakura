"""Cut over the morning digest while keeping the legacy writer authoritative until activation."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
from contextlib import contextmanager
from dataclasses import asdict
from datetime import datetime, time, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

from sakura import engine_home
from sakura.automations import launchd as auto_launchd
from sakura.automations import registry as automations
from sakura.connectors import kinds, migration as connector_migration, registry
from sakura.digest import config, installation, run, state
from sakura.digest import launchd as legacy_launchd
from sakura.scheduled import definition, launchd, store

DIGEST_VERB = "digest run --scheduled"
MIGRATED = "scheduled_migration_complete"
TASK_ID = "morning-summary"
CONFIG_PROBLEM = "legacy_digest_config_unavailable"
STATE_PROBLEM = "legacy_digest_state_unavailable"
REMOVAL_PROBLEM = "legacy_agent_removal_pending"
RUN_BUSY = "legacy_digest_busy"
TASK_UNAVAILABLE = "legacy_cutover_pending"


@contextmanager
def lock():
    """Serialize legacy settings changes and migration retries."""
    path = engine_home.root() / "scheduled-migration.lock"
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_CREAT | os.O_NOFOLLOW | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def owns_digest(tasks: store.Store | None = None) -> bool:
    """Return the durable writer mark. An unreadable store fails closed."""
    row = _migration(tasks or store.Store())
    return row is not None and row["phase"] == "active"


def status(tasks: store.Store | None = None) -> str | None:
    """Read a fixed migration problem without running reconciliation."""
    row = _migration(tasks or store.Store())
    return row["problem"] if row else None


def _migration(tasks: store.Store):
    if not tasks.path.exists() and not tasks.path.is_symlink():
        return None
    with tasks.connect() as db:
        if db.execute("PRAGMA user_version").fetchone()[0] < 5:
            return None
        return db.execute("SELECT * FROM digest_migration WHERE singleton=1").fetchone()


def zone_name() -> str:
    """Resolve the configured local zone to the IANA name saved with tasks."""
    configured = os.environ.get("TZ", "").lstrip(":")
    local = str(Path("/etc/localtime").resolve()).partition("/zoneinfo/")[2]
    for choice in (configured, local, "UTC"):
        if not choice:
            continue
        try:
            return ZoneInfo(choice).key
        except (ValueError, KeyError):
            continue
    return "UTC"


def upgrade(tasks: store.Store, now: datetime, *, agent: legacy_launchd.Agent | None = None,
            verify_route=None, timezone: str | None = None) -> None:
    """Import a legacy installation once and reconcile interrupted cutovers."""
    agent = agent or legacy_launchd.Agent()
    with lock():
        if owns_digest(tasks):
            _retire_agents(tasks, agent)
            return
        _retire_pending_maintenance(tasks)
        if not config.config_path().exists():
            if legacy_launchd.is_installed(agents_dir=agent.folder, home=agent.home):
                _problem(tasks, CONFIG_PROBLEM)
            return
        with run._lock() as is_free:
            if not is_free:
                _problem(tasks, RUN_BUSY)
                return
            if connector_migration.ready():
                raise store.Refused(MIGRATED)
            _stage_and_switch(tasks, now, agent, verify_route or _verify_route, timezone or zone_name())
        if owns_digest(tasks):
            _retire_agents(tasks, agent)



def retired_names(tasks: store.Store | None = None) -> set[str]:
    """Read pending maintenance names retired by the upgrade."""
    tasks = tasks or store.Store()
    if not tasks.path.exists():
        return set()
    with tasks.connect() as db:
        if db.execute("PRAGMA user_version").fetchone()[0] < 5:
            return set()
        return {row[0] for row in db.execute("SELECT name FROM retired_automations")}


def _retire_pending_maintenance(tasks: store.Store) -> None:
    names = [row.name for row in automations.pending() if row.verb == "flow wake"]
    if names:
        with tasks.connect(write=True) as db:
            db.executemany("INSERT OR IGNORE INTO retired_automations VALUES (?, 'maintenance_only')",
                           ((name,) for name in names))

def _stage_and_switch(tasks, now, agent, verify_route, zone) -> None:
    try:
        settings = config.load()
    except config.ConfigError as error:
        _problem(tasks, CONFIG_PROBLEM, error)
        return
    try:
        memory = state.load(strict=True)
    except state.StateError as error:
        _problem(tasks, STATE_PROBLEM, error)
        return
    manifest = _manifest(settings, memory, agent, zone)
    task = _stage(tasks, manifest, memory, now)
    if manifest["enabled"]:
        try:
            verify_route(task)
        except store.Refused as error:
            _problem(tasks, str(error), error)
            return
        launchd.reconcile(tasks, agent=agent, force_enabled=True)
    _activate(tasks, task, manifest, memory, now)


def _manifest(settings: config.DigestConfig, memory: state.State, agent, zone: str) -> dict:
    named = [row for row in automations.load() if row.verb == DIGEST_VERB]
    base = next((row for row in named if row.name == automations.MORNING_DIGEST), None)
    has_agent = legacy_launchd.is_installed(agents_dir=agent.folder, home=agent.home)
    base_enabled = has_agent and (base is None or (base.enabled and base.installed))
    aliases = [asdict(row) for row in named if row.name != automations.MORNING_DIGEST]
    bindings = {source.id: source.connector or _connector_id(source) for source in settings.sources}
    return {**asdict(settings), "timezone": zone, "bindings": bindings, "enabled": base_enabled or any(
        row["enabled"] for row in aliases), "base_enabled": base_enabled, "base": asdict(base) if base else None,
        "base_installed": has_agent, "aliases": aliases, "automation_status": automations._read_state(),
        "done_date": memory.done_date, "last_run_at": memory.last_run_at}


def _connector_id(source: config.Source) -> str:
    kind = kinds.PROGRAM if source.run else kinds.FEED
    digest = hashlib.sha256(source.id.encode()).hexdigest()[:24]
    return f"legacy-{kind}-{digest}"


def _stage(tasks, manifest: dict, memory: state.State, now: datetime) -> store.ScheduledTask:
    data = _definition(manifest)
    with tasks.connect(write=True) as db:
        existing = db.execute("SELECT * FROM tasks WHERE id=?", (TASK_ID,)).fetchone()
        if existing is None:
            task = tasks._insert(db, TASK_ID, data, now)
        else:
            task = store._task(existing)
        db.execute("INSERT INTO digest_migration VALUES (1,'staged',?,NULL) "
                   "ON CONFLICT(singleton) DO UPDATE SET manifest=excluded.manifest", (definition.encode(manifest),))
        _copy_checkpoints(db, memory)
        db.execute("DELETE FROM imported_connectors")
        for source in manifest["sources"]:
            _import_connector(db, source, manifest["bindings"][source["id"]])
        return store.ScheduledTask(task.id, task.revision, data, task.anchor)


def _definition(manifest: dict) -> dict:
    from sakura.scheduled import morning

    data = morning.default_definition(manifest["timezone"], manifest["language"])
    connector_ids = list(dict.fromkeys(manifest["bindings"][source["id"]]
                                      for source in manifest["sources"] if source["enabled"]))
    labels = {manifest["bindings"][source["id"]]: source["label"] or source["id"] for source in manifest["sources"]}
    return {**data, "connector_ids": connector_ids, "account": manifest["account"] or None, "legacy_labels": labels,
            "when": {**data["when"], "time": manifest["at"]}, "enabled": False, "legacy_digest": True,
            "legacy_model": manifest["model"], "legacy_dispatch": {"base": manifest["base_enabled"],
                "aliases": [row["schedule"] for row in manifest["aliases"] if row["enabled"]],
                "done_date": manifest["done_date"]}}


def _import_connector(db, source: dict, connector_id: str) -> None:
    if source["connector"]:
        return
    run = source["run"]
    kind, target = (kinds.PROGRAM, run[0]) if run else (kinds.FEED, source["feed"])
    try:
        identity = installation.inspect(tuple(run)).identity() if run else ""
    except installation.InstallationError:
        identity = ""
    connector = registry.Connector(connector_id, kind, source["label"] or source["id"], target,
                                   arguments=tuple(run[1:]), reads_text=source["reads"],
                                   timeout_seconds=source["timeout_seconds"],
                                   legacy_source_id=source["id"], program_identity=identity)
    db.execute("INSERT INTO imported_connectors VALUES (?,?,0)", (connector_id, definition.encode(asdict(connector))))


def _copy_checkpoints(db, memory: state.State) -> None:
    db.execute("DELETE FROM digest_checkpoints")
    db.executemany("INSERT INTO digest_checkpoints VALUES (?,?,?)",
                   ((key, checkpoint.cursor, checkpoint.at) for key, checkpoint in memory.sources.items()))


def _activate(tasks, task, manifest: dict, memory: state.State, now: datetime) -> None:
    zone = ZoneInfo(manifest["timezone"])
    start = datetime.combine(now.astimezone(zone).date(), time(), zone) - timedelta(microseconds=1)
    with tasks.connect(write=True) as db:
        data = {**task.definition, "enabled": manifest["enabled"]}
        db.execute("UPDATE tasks SET revision=revision+1,definition=?,anchor=? WHERE id=?",
                   (definition.encode(data), store.stamp(start), TASK_ID))
        _copy_checkpoints(db, memory)
        _import_approvals(db, data)
        db.execute("UPDATE digest_migration SET phase='active',problem=NULL WHERE singleton=1")


def _import_approvals(db, data: dict) -> None:
    for pending in automations.pending():
        if pending.verb != DIGEST_VERB:
            continue
        proposal_id = "legacy-" + hashlib.sha256(pending.name.encode()).hexdigest()[:24]
        preview = {**data, "legacy_alias": pending.schedule, "enabled": True}
        revision = hashlib.sha256(definition.encode(preview).encode()).hexdigest()
        db.execute("INSERT OR IGNORE INTO approvals VALUES (?,?,?,?,?,NULL)",
                   (proposal_id, revision, definition.encode(preview), pending.author, pending.proposed_at))


def accept_imported(db, data: dict, now: datetime) -> store.ScheduledTask:
    """Approve an additional legacy schedule for the single existing Morning summary."""
    task = store.Store._get(db, TASK_ID)
    trigger = task.definition.get("legacy_dispatch")
    fields = definition.REQUIRED | {"project", "preset", "account", "may_start_lanes", "legacy_model"}
    if trigger is None or any(task.definition.get(field) != data.get(field) for field in fields):
        raise store.Refused("stale_approval")
    approved = {**task.definition, "enabled": True,
                "legacy_dispatch": {**trigger, "aliases": list(dict.fromkeys([*trigger["aliases"], data["legacy_alias"]]))}}
    revision = int(task.revision) + 1
    anchor = store.stamp(now) if not task.definition["enabled"] else task.anchor
    db.execute("UPDATE tasks SET revision=?,definition=?,anchor=? WHERE id=?",
               (revision, definition.encode(approved), anchor, task.id))
    store.Store._skip_queued(db, task.id, now, "task_changed")
    return store.ScheduledTask(task.id, str(revision), approved, anchor)


def _verify_route(task: store.ScheduledTask) -> None:
    from sakura.master import spec
    from sakura.scheduled import runner

    claim = runner.Claim("migration-probe", task.id, task.revision, task.definition, {}, "")
    try:
        runner.probe(spec.prepare_scheduled(claim))
    except (OSError, RuntimeError, ValueError) as error:
        code = str(error)
        if code not in ("scheduled_provider_unavailable", "scheduled_account_required"):
            code = "scheduled_adapter_unverified"
        raise store.Refused(code) from error


def _retire_agents(tasks: store.Store, agent) -> None:
    row = _migration(tasks)
    manifest = json.loads(row["manifest"])
    if manifest.get("agents_retired"):
        return
    names = [legacy_launchd.LABEL, *(auto_launchd.label_of(alias["name"]) for alias in manifest["aliases"])]
    try:
        for label in names:
            legacy_launchd.remove_confirmed(label, agent)
    except (legacy_launchd.LaunchdError, OSError) as error:
        _problem(tasks, REMOVAL_PROBLEM, error)
        return
    with tasks.connect(write=True) as db:
        db.execute("UPDATE digest_migration SET manifest=?,problem=NULL WHERE singleton=1",
                   (definition.encode({**manifest, "agents_retired": True}),))


def _problem(tasks: store.Store, code: str, error: Exception | None = None) -> None:
    if error:
        store.note_problem(code, error)
    with tasks.connect(write=True) as db:
        db.execute("INSERT INTO digest_migration VALUES (1,'waiting','{}',?) "
                   "ON CONFLICT(singleton) DO UPDATE SET problem=excluded.problem", (code,))


def digest_state(tasks: store.Store) -> tuple[dict, state.State]:
    """Read the copied manifest and lossless checkpoints from the scheduled store."""
    with tasks.connect() as db:
        row = db.execute("SELECT manifest FROM digest_migration WHERE singleton=1 AND phase='active'").fetchone()
        if row is None:
            raise store.Refused(TASK_UNAVAILABLE)
        manifest = json.loads(row["manifest"])
        sources = {entry["source_id"]: state.SourceState(entry["cursor"], entry["at"])
                   for entry in db.execute("SELECT * FROM digest_checkpoints")}
        return manifest, state.State(sources, manifest["done_date"], manifest["last_run_at"])


def imported_connectors(tasks: store.Store | None = None) -> list[registry.Connector]:
    """Read migrated connector definitions after writer activation."""
    tasks = tasks or store.Store()
    if not owns_digest(tasks):
        return []
    with tasks.connect() as db:
        return [registry.decode(json.loads(row[0])) for row in
                db.execute("SELECT definition FROM imported_connectors WHERE NOT disconnected ORDER BY rowid")]
