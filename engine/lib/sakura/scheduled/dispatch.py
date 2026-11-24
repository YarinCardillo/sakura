"""Claim due clock slots and connector events in durable transactions."""

from __future__ import annotations

import json
from dataclasses import asdict
from datetime import UTC, date, datetime, time, timedelta
from functools import partial
from secrets import token_hex
from zoneinfo import ZoneInfo

from sakura.connectors import events, kinds, registry
from sakura.digest import feed
from sakura.scheduled import definition, store

HISTORY = timedelta(days=30)
ONE_DAY = timedelta(days=1)
ONE_MINUTE = timedelta(minutes=1)
MAX_EVENT_INPUT = 16_384


def daily_at(day: date, clock: str, zone: ZoneInfo) -> datetime:
    """Choose the first instant at a repeated wall time or the first valid minute after a gap."""
    wall = datetime.combine(day, time.fromisoformat(clock))
    while True:
        moment = wall.replace(tzinfo=zone, fold=0).astimezone(UTC)
        if moment.astimezone(zone).replace(tzinfo=None) == wall:
            return moment
        wall += ONE_MINUTE


def _slots(task: store.ScheduledTask, after: datetime, through: datetime) -> list[tuple[str, datetime]]:
    when = task.definition["when"]
    if when["kind"] == "interval":
        anchor = datetime.fromisoformat(task.anchor)
        period = timedelta(minutes=when["minutes"])
        return [(str(index), anchor + index * period)
                for index in range(max(1, (after - anchor) // period + 1), (through - anchor) // period + 1)]
    if when["kind"] == "daily":
        zone = ZoneInfo(when["timezone"])
        first, last = after.astimezone(zone).date(), through.astimezone(zone).date()
        dates = (first + ONE_DAY * offset for offset in range((last - first).days + 1))
        return [(day.isoformat(), moment) for day in dates
                if (moment := _task_daily(task, day, zone)) is not None and after < moment <= through]
    return []



def _task_daily(task: store.ScheduledTask, day: date, zone: ZoneInfo) -> datetime | None:
    due = daily_at(day, task.definition["when"]["time"], zone)
    if trigger := task.definition.get("legacy_dispatch"):
        from sakura.scheduled import morning

        return morning.legacy_opportunity(day, due, zone, trigger, datetime.fromisoformat(task.anchor))
    return due

def next_at(task: store.ScheduledTask, now: datetime) -> str | None:
    """Use the same calendar rules for the next displayed instant and for claims."""
    lower = max(now, datetime.fromisoformat(task.anchor))
    slots = _slots(task, lower, lower + timedelta(days=3))
    return store.stamp(slots[0][1]) if slots else None


def claim_clock(tasks: store.Store, now: datetime) -> list[str]:
    """Skip older slots and claim the newest eligible occurrence per task."""
    if not tasks.path.exists():
        return []
    with registry.admission_lock(), tasks.connect(write=True) as db:
        available = {item.id: item for item in registry.load()}
        result = []
        for row in db.execute("SELECT * FROM tasks WHERE NOT deleted"):
            task = store._task(row)
            if task.status(available)[0] == "active":
                result.extend(_claim_slots(db, task, row["frontier"], now))
        return result


def _claim_slots(db, task: store.ScheduledTask, frontier: str | None, now: datetime) -> list[str]:
    after = max(datetime.fromisoformat(frontier or task.anchor), now - HISTORY)
    slots = _slots(task, after, now)
    if not slots:
        return []
    newest = None
    for identity, moment in slots:
        skipped = moment != slots[-1][1]
        newest = _queue(db, task, f"clock:{task.anchor}:{identity}",
                        {"kind": task.definition["when"]["kind"], "at": store.stamp(moment)}, now,
                        skipped=skipped)
    db.execute("UPDATE tasks SET frontier=? WHERE id=?", (store.stamp(slots[-1][1]), task.id))
    return [newest] if newest else []


def _queue(db, task: store.ScheduledTask, occurrence: str, trigger: dict, now: datetime, *, skipped=False) -> str | None:
    run_id = token_hex(16)
    state = "skipped" if skipped else "queued"
    reason = "missed_occurrence" if skipped else None
    at = store.stamp(now)
    inserted = db.execute(
        "INSERT OR IGNORE INTO runs (id,task_id,task_revision,definition,occurrence,trigger,queued_at,state,reason,ended_at)"
        " VALUES (?,?,?,?,?,?,?,?,?,?)",
        (run_id, task.id, task.revision, definition.encode(task.definition), occurrence,
         definition.encode(trigger), at, state, reason, at if skipped else None),
    )
    return run_id if inserted.rowcount else None


def run_now(tasks: store.Store, task_id: str, revision: str, op: str, now: datetime) -> str:
    """Claim one manual run per operation, including a replay after the task changes."""
    tasks._valid_op(op)
    request = definition.encode([task_id, revision])
    with registry.admission_lock(), tasks.connect(write=True) as db:
        prior = tasks._operation(db, definition.RUN, op, request)
        if prior:
            return prior
        task = tasks._get(db, task_id, revision)
        if task.status({item.id: item for item in registry.load()})[0] != "active":
            raise store.Refused("action_unavailable")
        run_id = _queue(db, task, f"manual:{op}", {"kind": "manual", "at": store.stamp(now)}, now)
        db.execute("INSERT INTO operations VALUES (?, ?, ?, ?)", (definition.RUN, op, request, run_id))
        return run_id



def replay_target(tasks: store.Store, task_id: str, revision: str, op: str) -> store.ScheduledTask | None:
    """Resolve an already admitted request even after its task revision is gone."""
    if not tasks.path.exists() or not isinstance(op, str) or not definition.OP.fullmatch(op):
        return None
    with tasks.connect() as db:
        prior = tasks._operation(db, definition.RUN, op, definition.encode([task_id, revision]))
    return (store.ScheduledTask(task_id, revision, {"enabled": True}, "", replayed_run=prior)
            if prior else None)

def runs(tasks: store.Store, task_id: str, *, limit: int = 50, before: int | None = None) -> list[dict]:
    """Read a descending, bounded history page without starting anything."""
    if not tasks.path.exists():
        return []
    with tasks.connect() as db:
        return [_run_row(row) for row in db.execute(
            "SELECT rowid AS sequence, * FROM runs WHERE task_id=? AND (? IS NULL OR rowid < ?) "
            "ORDER BY rowid DESC LIMIT ?", (task_id, before, before, limit))]


def _run_row(row) -> dict:
    return {**{key: row[key] for key in ("id", "task_id", "sequence", "state", "reason", "queued_at", "started_at", "ended_at")},
            "title": json.loads(row["definition"])["title"], "trigger": _public_trigger(json.loads(row["trigger"])),
            "conversation": row["id"] if row["started_at"] else None}


def _public_trigger(trigger: dict) -> dict:
    return {key: value for key, value in trigger.items() if key not in ("input", "namespace")}


def admit_events(tasks: store.Store, batch: events.EventBatch, now: datetime) -> list[str]:
    """Commit a page's checkpoint, per-task baselines and claims together."""
    with registry.admission_lock(), tasks.connect(write=True) as db:
        available = {item.id: item for item in registry.load()}
        connector = available.get(batch.connector_id)
        if (connector is None or connector.revoked or not connector.connected
                or events.revision(connector) != batch.revision):
            raise store.Refused("stale_connector")
        _validate_batch(batch, connector)
        stream = (batch.connector_id, events.namespace(connector), batch.kind)
        current = _checkpoint(db, stream)
        if current != batch.checkpoint:
            if current == batch.next_checkpoint:
                return []
            raise store.Refused("stale_checkpoint")
        result = _admit(db, batch, available, connector, now)
        db.execute("INSERT OR REPLACE INTO checkpoints VALUES (?, ?, ?, ?)", (*stream, batch.next_checkpoint))
        return result


def _validate_batch(batch: events.EventBatch, connector) -> None:
    if (batch.kind != events.ENTRY_ADDED or batch.kind not in events.capabilities(connector)
            or len(batch.events) > events.MAX_EVENTS
            or not isinstance(batch.next_checkpoint, str) or len(batch.next_checkpoint) > 4096):
        raise store.Refused("invalid_event_batch")
    try:
        for event in batch.events:
            if ((event.connector_id, event.revision, event.kind) != (batch.connector_id, batch.revision, batch.kind)
                    or not all(isinstance(key, str) and 0 < len(key) <= 512 for key in (event.occurrence, event.item_id))
                    or not isinstance(event.input, dict) or len(definition.encode(event.input)) > MAX_EVENT_INPUT
                    or datetime.fromisoformat(event.occurred_at).utcoffset() is None):
                raise ValueError
    except (ValueError, TypeError, RecursionError):
        raise store.Refused("invalid_event_batch") from None


def _admit(db, batch: events.EventBatch, available: dict, connector, now: datetime) -> list[str]:
    namespace = events.namespace(connector)
    new_events = [event for event in batch.events if db.execute(
        "INSERT OR IGNORE INTO event_receipts VALUES (?, ?, ?, ?)",
        (batch.connector_id, namespace, batch.kind, event.occurrence)).rowcount]
    result = []
    match = {"kind": "connector_event", "connector_id": batch.connector_id, "event_kind": batch.kind}
    for row in db.execute("SELECT * FROM tasks WHERE NOT deleted"):
        task = store._task(row)
        if task.definition["when"] != match or task.status(available)[0] != "active":
            continue
        baseline = db.execute("INSERT OR IGNORE INTO subscriptions VALUES (?, ?, ?, ?)",
                              (task.id, task.anchor, batch.revision, batch.kind)).rowcount
        if baseline:
            continue
        for event in new_events:
            trigger = {**asdict(event), "kind": "connector_event", "event_kind": event.kind,
                       "namespace": namespace, "source_label": connector.label}
            run_id = _queue(db, task, f"event:{namespace}:{event.occurrence}", trigger, now)
            if run_id:
                result.append(run_id)
    return result


def _checkpoint(db, stream: tuple[str, str, str]) -> str | None:
    row = db.execute("SELECT value FROM checkpoints WHERE connector_id=? AND revision=? AND kind=?", stream).fetchone()
    return row[0] if row else None


def poll_events(tasks: store.Store, now: datetime, fetch: feed.Fetcher = feed.fetch) -> None:
    """Fetch each subscribed stream once, keeping a failed page's prior checkpoint."""
    available = {item.id: item for item in registry.load()}
    streams = {task.definition["when"]["connector_id"] for task in tasks.tasks()
               if task.definition["when"]["kind"] == "connector_event"
               and task.definition["when"]["event_kind"] == events.ENTRY_ADDED
               and task.status(available)[0] == "active"}
    for key in sorted(streams):
        connector = available[key]
        with tasks.connect() as db:
            checkpoint = _checkpoint(db, (key, events.namespace(connector), events.ENTRY_ADDED))
        try:
            admit_events(tasks, events.read_feed(connector, checkpoint, now, fetch), now)
        except (feed.FeedError, store.Refused) as error:
            store.note_problem("event_poll_failed", error)


    adapter = _ScopedEvents(tasks, now)
    events.poll(adapter, now, reader=partial(events.read, ports=kinds.Ports(fetch_feed=fetch)))


class _ScopedEvents:
    """Bridge verified provider transitions to the same durable run queue as clock slots."""

    def __init__(self, tasks: store.Store, now: datetime) -> None:
        self._tasks, self._now = tasks, now

    def subscriptions(self) -> list[events.Subscription]:
        """Return the enabled bindings and exact task revisions for this poll."""
        available = {item.id: item for item in registry.load()}
        result = []
        for task in self._tasks.tasks():
            trigger = task.definition["when"]
            if (trigger["kind"] != "connector_event" or trigger["event_kind"] == events.ENTRY_ADDED
                    or not task.definition["enabled"] or task.connector_pause):
                continue
            connector = available.get(trigger["connector_id"])
            result.append(events.Subscription(task.id, trigger["connector_id"], trigger["event_kind"],
                                              connector.revision if connector else 0, task.anchor, task.revision))
        return result

    def admit(self, task_id: str, event: events.Event) -> None:
        """Keep a stable event occurrence and source reference without saving provider bodies."""
        available = {item.id: item for item in registry.load()}
        connector = available.get(event.connector_id)
        if connector is None or connector.revoked or connector.revision != event.revision:
            return
        with self._tasks.connect(write=True) as db:
            task = self._tasks._get(db, task_id)
            if task.status(available)[0] != "active":
                return
            namespace = events.namespace(connector)
            trigger = {"kind": "connector_event", "event_kind": event.kind, "connector_id": connector.id,
                       "revision": events.revision(connector), "occurrence": event.key, "item_id": event.item_ref,
                       "occurred_at": event.at or store.stamp(self._now), "namespace": namespace,
                       "source_label": connector.label, "input": {"title": connector.label}}
            _queue(db, task, f"event:{namespace}:{event.key}", trigger, self._now)

    def pause(self, task_id: str, code: str, args: dict[str, str]) -> None:
        """Record a disconnected dependency with the task's normal queue cancellation."""
        task = self._tasks.get(task_id)
        connector_id = task.definition["when"]["connector_id"]
        self._tasks.pause_connector(task_id, connector_id, args.get("label"), self._now)
