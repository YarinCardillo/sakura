"""Stage run findings and project durable, merged proposals into Inbox."""

from __future__ import annotations

import hashlib
import json
from dataclasses import asdict
from secrets import token_hex
from urllib.parse import urlsplit

from sakura.digest import schema
from sakura.scheduled import definition, store

PROPOSE = "triage_propose"
EVENT_REF = "event"
MAX_FINDINGS = 32
MAX_EVIDENCE = 1024
MAX_DETAIL = 4000


def remember(db, run_id: str, namespace: str, label: str, item: schema.Item, *,
             reference: str | None = None, version: str | None = None) -> str:
    """Pin validated evidence before disclosing a reference to the assistant."""
    reference = reference or _digest([namespace, item.id])
    version = version or _digest([item.at, item.title, item.text, item.url])
    entry = {"namespace": namespace, "item": item.id, "version": version,
             "label": label, "link": _safe_url(item.url)}
    db.execute("INSERT OR IGNORE INTO run_evidence VALUES (?,?,?)", (run_id, reference, definition.encode(entry)))
    return reference


def remember_event(db, claim) -> None:
    """Record the evidence for an event-triggered run."""
    trigger = claim.trigger
    if trigger["kind"] != "connector_event":
        return
    try:
        item = schema.parse_item({**trigger["input"], "id": trigger["item_id"]})
    except schema.SchemaError:
        return
    remember(db, claim.id, trigger["namespace"], trigger.get("source_label", "Connector"), item,
             reference=EVENT_REF, version=trigger["occurrence"])


def stage(tasks: store.Store, run_id: str, title: str, detail: str, *, source_ref: str | None = None) -> None:
    """Store one proposed item for publication only when the turn completes."""
    if not definition._text(title, schema.MAX_ITEM_TITLE) or not definition._text(detail, MAX_DETAIL, multiline=True):
        raise store.Refused("invalid_finding")
    if source_ref is not None and not definition._text(source_ref, 128):
        raise store.Refused("source_item_unavailable")
    with tasks.connect(write=True) as db:
        row = db.execute("SELECT * FROM runs WHERE id=? AND state='running'", (run_id,)).fetchone()
        if row is None:
            raise store.Refused("run_authority_denied")
        data = json.loads(row["definition"])
        source = _source(db, row, data, source_ref, title, detail)
        payload = _payload(row, data, source, title, detail)
        count = db.execute("SELECT count(*) FROM staged_findings WHERE run_id=?", (run_id,)).fetchone()[0]
        if count >= MAX_FINDINGS:
            raise store.Refused("finding_limit")
        db.execute("INSERT OR IGNORE INTO staged_findings VALUES (?,?,?,?,?)",
                   (run_id, source["namespace"], source["item"], source["version"], definition.encode(payload)))


def _source(db, row, data: dict, reference: str | None, title: str, detail: str) -> dict:
    if reference:
        source = db.execute("SELECT evidence FROM run_evidence WHERE run_id=? AND reference=?",
                            (row["id"], reference)).fetchone()
        if source:
            return json.loads(source[0])
    elif not definition.required_connectors(data):
        return {"namespace": f"run:{row['id']}", "item": _digest([title, detail]), "version": "1",
                "label": "Scheduled task", "link": ""}
    raise store.Refused("source_item_unavailable")


def _payload(row, data: dict, source: dict, title: str, detail: str) -> dict:
    return {"title": title, "detail": detail, "link": source["link"],
            "project": data["project"], "preset": data["preset"], "account": data["account"],
            "run_id": row["id"], "origin": {"task_id": row["task_id"], "title": data["title"],
                                           "at": row["started_at"], "source": source["label"]}}


def publish(db, run_id: str) -> None:
    """Merge findings and record occurrence receipts in the run's final transaction."""
    for row in db.execute("SELECT * FROM staged_findings WHERE run_id=? ORDER BY rowid", (run_id,)):
        _merge(db, row)


def _merge(db, row) -> None:
    pair = (row["namespace"], row["item"])
    active = db.execute("SELECT * FROM findings WHERE namespace=? AND item=? AND active", pair).fetchone()
    if active and _is_resolved(active):
        db.execute("UPDATE findings SET active=0 WHERE id=?", (active["id"],))
        active = None
    receipt = db.execute("SELECT finding_id FROM finding_receipts WHERE namespace=? AND item=? AND version=?",
                         (*pair, row["version"])).fetchone()
    if receipt:
        identity = receipt["finding_id"]
    elif active:
        identity = active["id"]
    else:
        identity = token_hex(16)
        db.execute("INSERT INTO findings VALUES (?,?,?,?,1)", (identity, *pair, row["payload"]))
    db.execute("INSERT OR IGNORE INTO finding_receipts VALUES (?,?,?,?)", (*pair, row["version"], identity))
    origin = json.loads(row["payload"])["origin"]
    db.execute("INSERT OR IGNORE INTO finding_origins VALUES (?,?,?)",
               (identity, row["run_id"], definition.encode(origin)))


def discard(db, run_id: str) -> None:
    """Remove turn-local evidence and drafts after completion or failure."""
    db.execute("DELETE FROM staged_findings WHERE run_id=?", (run_id,))
    db.execute("DELETE FROM run_evidence WHERE run_id=?", (run_id,))


def pending_waits(tasks: store.Store | None = None) -> list:
    """Project open findings with links to retained conversations."""
    tasks = tasks or store.Store()
    if not tasks.path.exists():
        return []
    try:
        with tasks.connect() as db:
            if db.execute("PRAGMA user_version").fetchone()[0] < 4:
                return []
            found = list(db.execute("SELECT findings.*, runs.id AS live_run FROM findings "
                                    "LEFT JOIN runs ON runs.id=json_extract(findings.payload,'$.run_id') "
                                    "WHERE active ORDER BY findings.rowid"))
    except store.Refused:
        return []
    return [_wait(row, row["live_run"]) for row in found if not _is_resolved(row)]


def is_current(wait) -> bool:
    """Check that the reservation addresses the same open finding."""
    tasks = store.Store()
    with tasks.connect() as db:
        row = db.execute("SELECT * FROM findings WHERE id=? AND active", (wait.task,)).fetchone()
    if row is None:
        return False
    current = _wait(row, None)
    return (current.id, current.title, current.detail) == (wait.id, wait.title, wait.detail)


def resolve(wait) -> None:
    """Close the merged row after the existing Inbox reservation records its outcome."""
    if wait.finding is None:
        return
    tasks = store.Store()
    with tasks.connect(write=True) as db:
        db.execute("UPDATE findings SET active=0 WHERE id=?", (wait.task,))


def _is_resolved(row) -> bool:
    from sakura.cockpit import proposal_actions

    payload = json.loads(row["payload"])
    key = _wait_id(row["id"], payload["project"])
    return bool(proposal_actions.outcome(key).get("answer"))


def _wait_id(identity: str, project: str | None) -> str:
    return json.dumps(["wait", project or "/", identity, "proposal", identity], separators=(",", ":"))


def _wait(row, live_run: str | None):
    from pathlib import Path

    from sakura.flow.waits import PROPOSAL, Wait

    payload = json.loads(row["payload"])
    identity, project = row["id"], payload["project"]
    origin = payload["origin"]
    links = ((origin["source"], payload["link"]),) if payload["link"] else ()
    fields = {key: payload[key] for key in ("project", "preset", "account", "origin")}
    return Wait(_wait_id(identity, project), PROPOSAL, project or "/", Path(project).name if project else "",
                identity, payload["title"], payload["detail"], origin["at"], source="scheduled",
                lane_title=origin["title"], links=links, finding={**fields, "run_id": live_run})


def evidence_documents(tasks: store.Store, run_id: str, namespace: str, label: str, items: tuple) -> list[dict]:
    """Record references for a bounded response before giving the evidence to the model."""
    with tasks.connect(write=True) as db:
        count = db.execute("SELECT count(*) FROM run_evidence WHERE run_id=?", (run_id,)).fetchone()[0]
        if count + len(items) > MAX_EVIDENCE:
            raise store.Refused("evidence_limit")
        return [{**asdict(item), "reference": remember(db, run_id, namespace, label, item)} for item in items]


def _safe_url(value: str) -> str:
    try:
        url = urlsplit(value)
        if url.scheme in ("http", "https") and url.hostname and not url.username and not url.password:
            return value
    except ValueError:
        pass
    return ""


def _digest(value: list) -> str:
    return hashlib.sha256(definition.encode(value).encode()).hexdigest()
