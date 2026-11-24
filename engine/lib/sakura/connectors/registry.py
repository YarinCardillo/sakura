"""The connectors the person has connected, in `<engine home>/connectors.json` (mode 0600).

The file holds ids, kinds, labels, the feed address or program path of a `feed` or `program` connector, and the last
sign-in check; never a secret. A `notion` or `linear` token stays in the Keychain and is read only when it is used.
The check a snapshot shows is the last one made (by `connect`, `list` or a digest run): a snapshot never starts a program
or opens the Keychain.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field, replace
from datetime import UTC, datetime, timezone
from pathlib import Path
from uuid import uuid4

from sakura import engine_home
from sakura.connectors import contract, kinds
from sakura.connectors.kinds import ConnectorError, Ports
from sakura.digest import installation
from sakura.flow import gates

FILE_NAME = "connectors.json"
FILE_MODE = 0o600
REGISTRY_VERSION = 1
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
SINGLE_KINDS = (kinds.GITHUB, kinds.NOTION, kinds.LINEAR)
LABELS = {kinds.GITHUB: "GitHub", kinds.NOTION: "Notion", kinds.LINEAR: "Linear"}
SLUG = re.compile(r"[^a-z0-9]+")
MAX_SLUG = 20
MAX_LABEL = 40
ROW_FIELDS = ("id", "kind", "label", "connected", "detail", "last_used_at")


@dataclass(frozen=True)
class Connector:
    """One connected account or source; `target` is the address of a feed or the path of a program, else empty."""

    id: str
    kind: str
    label: str
    target: str = ""
    connected: bool = True
    detail: str = ""
    last_used_at: str | None = None
    program_identity: str = ""
    revision: int = 1
    revoked: bool = False
    revoked_at_revision: int = 0
    selectors: dict = field(default_factory=dict)
    display: dict = field(default_factory=dict)
    principal: str = ""
    arguments: tuple[str, ...] = ()
    reads_text: str = ""
    status: str = ""
    executable_identity: str = ""
    permissions: tuple[str, ...] = ()
    legacy_source_id: str = ""
    legacy_fingerprint: str = ""
    replacements: dict = field(default_factory=dict)
    generation: str = ""
    timeout_seconds: int | None = None


def path() -> Path:
    """The registry file in the engine home; not created."""
    return engine_home.root() / FILE_NAME


def load() -> list[Connector]:
    """Read operator-configured and activated migrated connectors."""
    explicit = _load()[0]
    known = {item.id for item in explicit}
    return explicit + [item for item in _imported() if item.id not in known]


def _imported() -> list[Connector]:
    from sakura.scheduled import migrate, store

    try:
        return migrate.imported_connectors()
    except (store.Refused, ValueError, TypeError, AttributeError):
        return []


def decode(entry: dict) -> Connector:
    """Read a saved connector, including definitions saved before versioned instances."""
    values = dict(entry)
    argv = values.pop("argv", ())
    reads = values.pop("reads", "")
    values["arguments"] = tuple(values.get("arguments", argv[1:]))
    values["permissions"] = tuple(values.get("permissions", ()))
    values.setdefault("reads_text", reads)
    return Connector(**values)


def _load() -> tuple[list[Connector], dict]:
    try:
        data = json.loads(path().read_text())
        if type(data.get("version", 0)) is not int or data.get("version", 0) not in (0, REGISTRY_VERSION):
            raise ValueError
        records = [decode(entry) for entry in data["connectors"]]
        receipts = data.get("receipts", {})
        if not isinstance(receipts, dict) or len({record.id for record in records}) != len(records):
            raise ValueError
        return records, receipts
    except FileNotFoundError:
        return [], {}
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise ConnectorError("connector_unavailable") from error

def find(connector_id: str) -> Connector | None:
    """The connector with this id, or None."""
    return next((found for found in load() if found.id == connector_id), None)


def rows(*, versioned: bool = False) -> list[dict]:
    """Return saved connector rows, including legacy digest program rows until import."""
    from sakura.connectors import events

    dependencies = events.dependent_rows() if versioned else {}
    if versioned:
        _scheduled_dependencies(dependencies)
    records = load()
    from sakura.scheduled import migrate, store

    rows = [_row(found, versioned, dependencies.get(found.id, [])) for found in records]
    try:
        if migrate.owns_digest():
            return rows
    except store.Refused:
        return rows
    imported = {found.legacy_source_id for found in records if found.legacy_source_id}
    return rows + _digest_program_rows(imported, versioned=versioned)


def _scheduled_dependencies(dependencies: dict) -> None:
    from sakura.scheduled import definition, store

    try:
        tasks = store.Store().tasks()
    except store.Refused:
        return
    for task in tasks:
        for connector_id in definition.required_connectors(task.definition):
            entries = dependencies.setdefault(connector_id, [])
            if all(entry["id"] != task.id for entry in entries):
                entries.append({"id": task.id, "label": task.definition["title"]})


def _row(found: Connector, versioned: bool, dependent_tasks: list) -> dict:
    from sakura.connectors import events

    row = {name: getattr(found, name) for name in ROW_FIELDS}
    if event_kinds := events.capabilities(found):
        row["event_kinds"] = list(event_kinds)
    if not versioned:
        return {**row, "reads": found.reads_text} if found.reads_text else row
    status = "disconnected" if found.revoked else found.status or ("connected" if found.connected else "connector_unavailable")
    return {**row, "detail": status.replace("_", " "), "revision": found.revision, "status": status,
            "scope": {"mode": "scoped" if found.selectors else "legacy", "selectors": found.selectors,
                      "labels": found.display},
            "reads": {"text": found.reads_text} if found.reads_text else
                     {"key": f"connector_reads_{found.kind}", "args": found.display},
            "operations": list(kinds.operation_keys(found)), "events": list(kinds.event_keys(found)), "dependent_tasks": dependent_tasks}

def connect(kind: str, label: str, target: str, ports: Ports, now: datetime) -> Connector:
    """Check the sign-in (or the Keychain item, the address, the program) and record the connector.

    A `github`, `notion` or `linear` connector has the kind as its id (a label only renames it); a `feed` or `program`
    needs a label, which makes its id (`feed-news`). Connecting again replaces the record. Raises ConnectorError with the
    reason when the check fails; then nothing is recorded."""
    if kind not in kinds.KINDS:
        raise ConnectorError(f"unknown kind {kind!r}; the kinds are {', '.join(kinds.KINDS)}")
    if kind not in SINGLE_KINDS and not label.strip():
        raise ConnectorError(f"a {kind} connector needs a label")
    if len(label) > MAX_LABEL or "\n" in label:
        raise ConnectorError(f"the label is one line of at most {MAX_LABEL} characters")
    status = kinds.check(kind, target, ports)
    if not status.is_connected:
        raise ConnectorError(status.detail)
    connector_id = kind if kind in SINGLE_KINDS else f"{kind}-{_slug(label)}"
    try:
        identity = installation.inspect((target,)).identity() if kind == kinds.PROGRAM else ""
    except installation.InstallationError as error:
        raise ConnectorError(str(error)) from None
    found = Connector(connector_id, kind, label.strip() or LABELS[kind], target, True, status.detail,
                      program_identity=identity)
    with locked():
        records, receipts = _load()
        previous = next((row for row in records if row.id == connector_id), None)
        if previous:
            if previous.selectors or previous.revoked:
                raise ConnectorError("invalid_request")
            found = replace(found, revision=previous.revision + 1)
        generation = previous.generation if previous and previous.target == target else uuid4().hex
        found = replace(found, generation=generation)
        _save([*(other for other in records if other.id != connector_id), found], receipts)
    return found


def disconnect(connector_id: str) -> None:
    """Forget a connector (a Keychain item is the person's own and stays). Raises ConnectorError when there is none."""
    from sakura.scheduled import store

    with locked():
        records, receipts = _load()
        imported = next((record for record in _imported() if record.id == connector_id), None)
        found = next((record for record in records if record.id == connector_id), imported)
        if found is None:
            raise ConnectorError(f"no connector {connector_id!r}")
        if found.selectors or found.revoked:
            raise ConnectorError("invalid_request")
        storage = store.Store()
        storage.connector_disconnected(found, datetime.now(UTC))
        if imported is not None:
            with storage.connect(write=True) as db:
                db.execute("UPDATE imported_connectors SET disconnected=1 WHERE id=?", (connector_id,))
        if found in records:
            _save([record for record in records if record.id != connector_id], receipts)


def refresh(ports: Ports, now: datetime) -> list[Connector]:
    """Refresh each active instance without overwriting a newer revision."""
    for found in load():
        if found.revoked:
            continue
        status = kinds.check_instance(found, ports)
        _report(found.id, found.revision, connected=status.is_connected, detail=status.detail, status=status.code)
    return load()


def mark_used(connector_id: str, now: datetime, *, revision: int, is_ok: bool, detail: str = "") -> None:
    """Update the result only if the instance and revision still match the one read."""
    stamp = now.astimezone(timezone.utc).strftime(TIME_FORMAT)
    changes = {"last_used_at": stamp, "connected": is_ok, "status": "connected" if is_ok else contract.client_reason(detail)}
    if not is_ok:
        changes["detail"] = detail
    _report(connector_id, revision, **changes)


def _report(connector_id: str, revision: int, **changes) -> None:
    with locked():
        records, receipts = _load()
        for index, found in enumerate(records):
            if found.id == connector_id and found.revision == revision and not found.revoked:
                records[index] = replace(found, **changes)
                _save(records, receipts)
                return


@contextmanager
def locked() -> Iterator[None]:
    """Serialize registry mutations and operations that depend on its current revisions."""
    target = path().with_suffix(".lock")
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(target, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, FILE_MODE)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


admission_lock = locked


def mutate(verb: str, op: str, ports: Ports, now: datetime, *, connector_id: str = "",
           revision: int = 0, fields: dict | None = None, candidate: str = "", candidate_revision: int = 0) -> dict:
    """Apply one revision-checked mutation and atomically keep its retry receipt."""
    if not contract.ID.fullmatch(op) or verb not in ("add", "edit", "revoke", "replace"):
        raise contract.Refusal("invalid_request")
    fields = contract.validate(fields, edit=verb == "edit") if verb in ("edit", "add") else {}
    request = [verb, connector_id, revision, fields, candidate, candidate_revision]
    fingerprint = hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest()
    with locked():
        records, receipts = _load()
        if previous := receipts.get(op):
            if previous["fingerprint"] != fingerprint:
                raise contract.Refusal("operation_conflict")
            return previous["result"]
        available = [*records, *_imported()] if verb != "add" else records
        old = None if verb == "add" else _current(available, connector_id, revision)
        if verb == "replace":
            choice = _current(available, candidate, candidate_revision)
            found = _replace_connector(old, choice, ports)
        else:
            found = _changed(verb, old, fields, ports)
        result = {"id": found.id, "revision": found.revision}
        receipts[op] = {"fingerprint": fingerprint, "result": result}
        if verb == "revoke":
            from sakura.scheduled import store

            store.Store().connector_disconnected(old, now)
        records = [found if record.id == connector_id else record for record in records]
        if found not in records:
            records.append(found)
        _save(records, receipts)
        return result


def _current(records: list[Connector], connector_id: str, revision: int) -> Connector:
    found = next((row for row in records if row.id == connector_id), None)
    if found is None:
        raise contract.Refusal("unknown_connector")
    if found.revision != revision:
        raise contract.Refusal("stale_revision")
    return found


def _changed(verb: str, old: Connector | None, fields: dict, ports: Ports) -> Connector:
    if verb == "revoke":
        return replace(old, revision=old.revision + 1, connected=False, revoked=True,
                       revoked_at_revision=old.revision + 1, status="disconnected")
    if "kind" not in fields:
        if old is None or ("reads" in fields and old.kind != kinds.PROGRAM):
            raise contract.Refusal("invalid_choices")
        return replace(old, revision=old.revision + 1, label=fields["label"], reads_text=fields.get("reads", old.reads_text))
    if old and old.kind != fields["kind"]:
        raise contract.Refusal("invalid_choices")
    found = _prepare(fields, ports)
    if old:
        return replace(found, id=old.id, revision=old.revision + 1, last_used_at=old.last_used_at,
                       revoked_at_revision=old.revoked_at_revision, legacy_source_id=old.legacy_source_id,
                       legacy_fingerprint=old.legacy_fingerprint, replacements=old.replacements)
    return replace(found, id=f"cn-{uuid4().hex}")


def _prepare(fields: dict, ports: Ports) -> Connector:
    kind = fields["kind"]
    target = fields.get("address", fields.get("program", ""))
    access, selectors, display = kinds.prepare_scope(kind, fields, ports)
    arguments = tuple(fields.get("args", ()))
    try:
        identity = installation.inspect((target, *arguments)).identity() if kind == kinds.PROGRAM else ""
    except installation.InstallationError:
        raise ConnectorError("approval_required") from None
    return Connector("", kind, fields["label"], target=target, selectors=selectors, display=display,
                     principal=access.principal, reads_text=fields.get("reads", ""),
                     arguments=arguments, status="connected", program_identity=identity,
                     executable_identity=access.resource if kind == kinds.PROGRAM else "",
                     permissions=access.operations)


def preview_replacement(ports: Ports, *, connector_id: str, revision: int, candidate: str,
                        candidate_revision: int) -> dict:
    """Verify a replacement without mutating any state, and return safe display words."""
    from sakura.connectors import replacement

    with locked():
        records = load()
        original, choice = _current(records, connector_id, revision), _current(records, candidate, candidate_revision)
        replacement.validate(original, choice, ports)
        return {"label": original.label, "candidate_label": choice.label}


def _replace_connector(original: Connector, candidate: Connector, ports: Ports) -> Connector:
    from sakura.connectors import replacement

    return replacement.swapped(original, candidate, replacement.validate(original, candidate, ports))


def import_legacy(sources) -> dict[str, str]:
    """Map saved source IDs to stable connector IDs in one atomic registry write."""
    with locked():
        records, receipts = _load()
        positions = {found.legacy_source_id: index for index, found in enumerate(records) if found.legacy_source_id}
        result, changed = {}, False
        for source, label, reads in sources:
            if source.connector:
                result[source.id] = source.connector
                continue
            index = positions.get(source.id)
            previous = records[index] if index is not None else None
            found = _legacy_connector(source, label, reads, previous)
            result[source.id] = found.id
            if previous is None:
                records.append(found)
            elif previous != found:
                records[index] = found
            changed = changed or previous != found
        if changed:
            _save(records, receipts)
        return result


def _legacy_connector(source, label: str, reads: str, previous: Connector | None) -> Connector:
    kind, target = (kinds.PROGRAM, source.run[0]) if source.run else (kinds.FEED, source.feed)
    arguments = tuple(source.run[1:])
    fingerprint = hashlib.sha256(json.dumps([kind, target, arguments]).encode()).hexdigest()
    if previous and previous.legacy_fingerprint == fingerprint:
        return previous
    if previous and (previous.revoked or previous.selectors):
        raise ConnectorError("operation_conflict")
    try:
        identity = installation.inspect(source.run).identity() if source.run else ""
    except installation.InstallationError:
        identity = ""
    return Connector(previous.id if previous else f"cn-{uuid4().hex}", kind, label, target=target,
                     revision=previous.revision + 1 if previous else 1,
                     arguments=arguments, reads_text=reads, legacy_source_id=source.id,
                     legacy_fingerprint=fingerprint, program_identity=identity)


def _digest_program_rows(imported: set[str], *, versioned: bool) -> list[dict]:
    """Show unmigrated digest readers from saved facts in the cockpit, and check them on legacy list."""
    from sakura.digest import config as digest_config, state as digest_state

    try:
        settings, memory = digest_config.load_or_default(), digest_state.load()
    except (digest_config.ConfigError, digest_state.StateError):
        return []
    found = []
    for source in settings.sources:
        if source.run and source.id not in imported:
            problem = "managed by the legacy digest" if versioned else kinds.program_problem(source.run[0])
            found.append({"id": source.id, "kind": kinds.PROGRAM, "label": source.label or source.id,
                          "connected": problem is None, "detail": problem or "the program is in place",
                          "last_used_at": memory.of(source.id).at or None})
    return found


def _slug(label: str) -> str:
    slug = SLUG.sub("-", label.lower()).strip("-")[:MAX_SLUG].strip("-")
    if not slug:
        raise ConnectorError("the label needs letters or digits")
    return slug


def _save(connectors: list[Connector], receipts: dict) -> None:
    document = {"version": REGISTRY_VERSION, "connectors": [asdict(found) for found in connectors], "receipts": receipts}
    gates.write_atomically(path(), json.dumps(document, indent=1).encode())
