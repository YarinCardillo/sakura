"""Read connector events and hand them to the scheduler's durable admission path."""

from __future__ import annotations

import hashlib
import json
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Protocol

from sakura import engine_home, text_limits
from sakura.connectors import contract, kinds, providers, registry
from sakura.digest import feed
from sakura.flow import gates

STATE = "connector-event-checkpoints.json"
DEPENDENCIES = "connector-task-dependencies.json"
DISCONNECTED = "connector_disconnected"
TIMEOUT = 30
MAX_OBSERVATIONS = 600


@dataclass(frozen=True)
class Subscription:
    """One active scheduled task's binding to an instance event stream."""

    task_id: str
    connector_id: str
    kind: str
    resumed_revision: int = 0
    anchor: str = ""
    task_revision: str = ""


@dataclass(frozen=True)
class Observation:
    """Provider occurrence or current state of an item tracked across polls."""

    key: str
    item_ref: str
    at: str
    active: bool = True
    subject: str = ""


@dataclass(frozen=True)
class Event:
    """A safe event reference with a stable deduplication key across subscriptions."""

    key: str
    connector_id: str
    revision: int
    kind: str
    at: str
    item_ref: str


class Scheduler(Protocol):
    """Admission and pause methods are durable, idempotent and never start a run inline."""

    def subscriptions(self) -> Sequence[Subscription]: ...

    def admit(self, task_id: str, event: Event) -> None: ...

    def pause(self, task_id: str, code: str, args: dict[str, str]) -> None: ...


Reader = Callable[[registry.Connector, str, datetime], list[Observation]]


def poll(scheduler: Scheduler, now: datetime, *, reader: Reader | None = None) -> None:
    """Poll active streams. Only acknowledge batches after durable scheduler admission."""
    reader = reader or read
    batches = {}
    for binding in tuple(scheduler.subscriptions()):
        with registry.locked():
            found = _dependency(scheduler, binding.task_id, binding.connector_id, binding.resumed_revision)
            if found is None or binding.kind not in kinds.event_keys(found):
                continue
            before = _checkpoints().get(_stream(binding, found))
        key = found.id, found.revision, binding.kind
        if key not in batches:
            try:
                batches[key] = reader(found, binding.kind, now)
            except (kinds.ConnectorError, OSError):
                batches[key] = None
        if batches[key] is not None:
            _deliver(scheduler, binding, found, before, batches[key])


def _deliver(scheduler: Scheduler, binding: Subscription, found: registry.Connector,
             before: dict | None, observations: list[Observation]) -> None:
    if len(observations) > MAX_OBSERVATIONS:
        return
    with registry.locked():
        current = _dependency(scheduler, binding.task_id, binding.connector_id, binding.resumed_revision)
        if current is None or current.revision != found.revision or binding not in scheduler.subscriptions():
            return
        checkpoints = _checkpoints()
        stream = _stream(binding, found)
        if checkpoints.get(stream) != before:
            return
        following, events = _transition(found, binding.kind, before, observations)
        for event in events:
            scheduler.admit(binding.task_id, event)
        checkpoints[stream] = following
        gates.write_atomically(engine_home.root() / STATE, json.dumps(checkpoints).encode())


def _transition(found: registry.Connector, kind: str, before: dict | None,
                observations: list[Observation]) -> tuple[dict, list[Event]]:
    seen = set((before or {}).get("seen", ()))
    active = dict((before or {}).get("active", {}))
    result = []
    for item in observations:
        was_active = item.subject and active.get(item.subject, False)
        if item.subject:
            active[item.subject] = item.active
        occurrence = _digest(kind, item.key)
        if not item.active or occurrence in seen:
            continue
        seen.add(occurrence)
        if before is not None and not was_active:
            ref = item.item_ref if contract.ID.fullmatch(item.item_ref) else _digest(item.item_ref)
            result.append(Event(occurrence, found.id, found.revision, kind, _time(item.at), ref))
    return {"seen": sorted(seen), "active": active}, result


def claim(scheduler: Scheduler, task_id: str, dependencies: Mapping[str, int],
          next_run: Callable[[], object]) -> object | None:
    """Claim under the same lock as disconnect, including tool-only dependencies."""
    with registry.locked():
        for connector_id, resumed_revision in dependencies.items():
            if _dependency(scheduler, task_id, connector_id, resumed_revision) is None:
                return None
        return next_run()


def _dependency(scheduler: Scheduler, task: str, connector_id: str, resumed_revision: int) -> registry.Connector | None:
    found = registry.find(connector_id)
    if not found or found.revoked or found.revoked_at_revision > resumed_revision:
        args = {"label": found.label} if found else {}
        scheduler.pause(task, DISCONNECTED, args)
        return None
    return found


def _stream(binding: Subscription, found: registry.Connector) -> str:
    parts = (binding.task_id, found.id, str(found.revision), binding.kind)
    return _digest(*parts, binding.anchor) if binding.anchor else _digest(*parts)


def _checkpoints() -> dict:
    try:
        return json.loads((engine_home.root() / STATE).read_text())
    except FileNotFoundError:
        return {}


def _digest(*parts: str) -> str:
    return hashlib.sha256(json.dumps(parts).encode()).hexdigest()


def _time(value: str) -> str:
    try:
        parsed = datetime.fromisoformat(value)
        return text_limits.utc_text(parsed) if parsed.tzinfo is not None else ""
    except (ValueError, TypeError):
        return ""


def read(connector: registry.Connector, kind: str, now: datetime,
         ports: kinds.Ports | None = None) -> list[Observation]:
    """Read a bounded listing with fresh provider membership and credential checks."""
    if kind not in kinds.event_keys(connector):
        raise kinds.ConnectorError("scope_unavailable")
    ports = ports or kinds.Ports()
    if kind == "new_item":
        fetched = kinds.read(connector, "", "", TIMEOUT, ports)
        return [Observation(_digest(connector.target, item.id), _digest(item.id), item.at or now.isoformat())
                for item in fetched.items]
    deadline = time.monotonic() + TIMEOUT
    access, records = kinds.scoped_listing(connector, deadline, ports)
    if kind == "check_failed":
        return _check_observations(access, records, deadline)
    if kind == "issue_assigned":
        recipient = connector.selectors.get("recipient") or access.session.recipient
        return [_assignment(access, record, recipient) for record in records]
    return [Observation(_digest(access.resource, row["target"]), row["target"], row["created"]) for row in records]


def _assignment(access: kinds.Access, record: dict, recipient: str) -> Observation:
    subject = _digest(access.resource, recipient, record["target"])
    return Observation(_digest(subject, record["item"]["at"]), record["target"], record["item"]["at"],
                       record["assignee"] == recipient, subject)


def _check_observations(access: kinds.Access, records: list[dict], deadline: float) -> list[Observation]:
    found = []
    try:
        for row in records:
            if row.get("is_issue"):
                continue
            for check in providers.failed_checks(access, row, deadline):
                key = _digest(access.resource, row["target"], str(check["id"]), check["completed_at"])
                found.append(Observation(key, row["target"], check["completed_at"]))
                if len(found) > MAX_OBSERVATIONS:
                    raise ValueError
    except (ValueError, TypeError, KeyError, AttributeError, OSError):
        raise kinds.ConnectorError("connector_unavailable") from None
    return found


def publish_dependencies(dependencies: Mapping[str, Sequence[tuple[str, str]]]) -> None:
    """Publish the scheduler's safe task labels for the saved connector snapshot."""
    safe = {connector: [{"id": task, "label": label} for task, label in tasks
                        if contract.ID.fullmatch(task) and not text_limits.one_line_problem(label, 120)]
            for connector, tasks in dependencies.items() if contract.ID.fullmatch(connector)}
    gates.write_atomically(engine_home.root() / DEPENDENCIES, json.dumps(safe).encode())


def dependent_rows() -> dict:
    """Read the last dependency projection without invoking the scheduler."""
    try:
        saved = json.loads((engine_home.root() / DEPENDENCIES).read_text())
        return saved if isinstance(saved, dict) else {}
    except (OSError, ValueError):
        return {}


ENTRY_ADDED = "entry_added"
MAX_EVENTS = 1024


@dataclass(frozen=True)
class ConnectorEvent:
    connector_id: str
    revision: str
    kind: str
    occurrence: str
    item_id: str
    occurred_at: str
    input: dict


@dataclass(frozen=True)
class EventBatch:
    connector_id: str
    revision: str
    kind: str
    checkpoint: str | None
    next_checkpoint: str
    events: tuple[ConnectorEvent, ...]


def capabilities(connector) -> tuple[str, ...]:
    """Advertise events only for adapters with a complete bounded read."""
    legacy = (ENTRY_ADDED,) if connector.kind == kinds.FEED and not connector.revoked else ()
    return (*legacy, *kinds.event_keys(connector))


def revision(connector) -> str:
    """The configured stream identity, independent of status checks and last use."""
    identity = f"{namespace(connector)}\0{connector.generation}"
    return _hash(f"{identity}\0{connector.revision}" if connector.selectors else identity)


def run_revision(connector) -> str:
    """Bind a running tool to both the instance revision and the saved stream generation."""
    return _digest(str(connector.revision), revision(connector))


def namespace(connector) -> str:
    """Stable evidence namespace for a source or an explicitly verified provider scope."""
    if connector.selectors and connector.kind != kinds.FEED:
        scope = json.dumps(connector.selectors, sort_keys=True)
        return f"{connector.kind}:{_digest(connector.principal, scope, connector.target)}"
    return f"{connector.kind}:{_hash(connector.target)}"


def read_feed(connector, checkpoint: str | None, now: datetime, fetch: feed.Fetcher = feed.fetch) -> EventBatch:
    """Read a whole feed snapshot using stable entry IDs, including undated entries."""
    body = fetch(connector.target, 15)
    items, _, _ = feed.entries(body, "", "")
    if len(items) > MAX_EVENTS:
        raise feed.FeedError("too many feed events to admit")
    rev = revision(connector)
    rows = tuple(ConnectorEvent(connector.id, rev, ENTRY_ADDED, _hash(item["id"]), item["id"],
                                item.get("at") or now.isoformat(), item) for item in items)
    return EventBatch(connector.id, rev, ENTRY_ADDED, checkpoint, hashlib.sha256(body).hexdigest(), rows)


def _hash(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()
