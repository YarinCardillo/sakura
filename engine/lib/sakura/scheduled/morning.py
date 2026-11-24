"""The Morning summary template and the migrated digest's read/checkpoint path."""

from __future__ import annotations

import json
from dataclasses import asdict
from datetime import UTC, date, datetime, timedelta
from zoneinfo import ZoneInfo

from sakura.connectors import events, registry
from sakura.digest import config, intake, state, summarize
from sakura.scheduled import definition, findings, migrate, store

TITLE = {"en": "Morning summary", "it": "Riepilogo del mattino"}
PROMPT = {
    "en": "Summarize what changed in my selected sources. Propose actionable findings in Inbox.",
    "it": "Riassumi le novità nelle fonti selezionate. Proponi in Posta in arrivo gli elementi che richiedono un'azione.",
}


def default_definition(zone: str, language: str) -> dict:
    """Return a paused definition that the person can customize before enabling."""
    locale = language.split("-")[0]
    return {**definition.OPTIONAL, "title": TITLE.get(locale, TITLE["en"]),
            "prompt": PROMPT.get(locale, f"{PROMPT['en']} Write the summary in language {language}."),
            "connector_ids": [], "when": {"kind": "daily", "time": config.DEFAULT_AT, "timezone": zone},
            "enabled": False}


def templates() -> list[dict]:
    """Expose a localized draft for each language the app presents."""
    zone = migrate.zone_name()
    return [{"id": migrate.TASK_ID, "labels": TITLE,
             "definitions": {language: default_definition(zone, language) for language in TITLE}}]


def legacy_opportunity(day: date, due: datetime, zone: ZoneInfo, trigger: dict, anchor: datetime) -> datetime | None:
    """Choose the first legacy dispatch opportunity on or after the digest's local eligibility time."""
    from sakura.scheduled.dispatch import daily_at

    if day.isoformat() <= trigger["done_date"]:
        return None
    eligible = max(due, anchor + timedelta(microseconds=1))
    moments = [due] if trigger["base"] and due >= eligible else []
    for schedule in trigger["aliases"]:
        form, value, *_ = schedule.split()
        if form == "daily":
            moment = daily_at(day, value, zone)
        else:
            period = timedelta(minutes=int(value))
            moment = anchor + max(0, -((anchor - eligible) // period)) * period
        if moment >= eligible and moment.astimezone(zone).date() == day:
            moments.append(moment)
    return min(moments) if moments else None


def prepare_run(tasks: store.Store, claim, now: datetime) -> dict:
    """Read with legacy intake, keeping a staged checkpoint until durable publication."""
    manifest, memory = migrate.digest_state(tasks)
    settings = _settings(manifest, claim.definition)
    if not settings.active_sources:
        raise store.Refused("legacy_sources_unavailable")
    sinces = intake.sinces(settings, memory, now)
    saved_windows = state.with_windows(memory, sinces)
    if saved_windows != memory:
        _save_windows(tasks, saved_windows)
    pinned = claim.definition.get("connector_revisions", {})
    available = {connector.id: connector for connector in registry.load() if connector.connected
                 and not connector.revoked and pinned.get(connector.id) == events.run_revision(connector)}
    collected = summarize.fit(intake.collect(settings, memory, now, record_use=False, lookup=available.get))
    if not any(result.report.ok for result in collected):
        raise store.Refused("all_sources_failed")
    evidence = _evidence(tasks, claim, manifest, collected, available)
    zone = ZoneInfo(manifest["timezone"])
    proposed = state.committed(memory, [result.report for result in collected],
                               {result.report.id: result.cursor for result in collected}, now.astimezone(UTC),
                               now.astimezone(zone).date().isoformat(), sinces)
    with tasks.connect(write=True) as db:
        db.execute("INSERT OR REPLACE INTO digest_run_updates VALUES (?,?)",
                   (claim.id, definition.encode(asdict(proposed))))
    return {"language": manifest["language"], "items": evidence,
            "sources": [{"name": _source_label(manifest, result.report.id), "ok": result.report.ok}
                        for result in collected]}


def _settings(manifest: dict, task: dict) -> config.DigestConfig:
    sources = tuple(config.Source(source["id"], connector=manifest["bindings"][source["id"]],
                                  timeout_seconds=source["timeout_seconds"], label=source["label"], reads=source["reads"],
                                  enabled=source["enabled"] and manifest["bindings"][source["id"]] in task["connector_ids"])
                    for source in manifest["sources"])
    return config.DigestConfig(manifest["at"], manifest["model"], manifest["language"],
                               manifest["lookback_hours"], sources, manifest["account"])


def _save_windows(tasks, memory) -> None:
    with tasks.connect(write=True) as db:
        db.executemany("INSERT OR IGNORE INTO digest_checkpoints VALUES (?,?,?)",
                       ((key, value.cursor, value.at) for key, value in memory.sources.items()))


def _evidence(tasks, claim, manifest, collected, available) -> list[dict]:
    documents = []
    with tasks.connect(write=True) as db:
        for result in collected:
            connector = available.get(manifest["bindings"][result.report.id])
            if not result.report.ok or connector is None:
                continue
            namespace = events.namespace(connector)
            for item in result.items:
                reference = findings.remember(db, claim.id, namespace, connector.label, item)
                documents.append({**asdict(item), "reference": reference, "source": connector.label})
    return documents



def _source_label(manifest: dict, source_id: str) -> str:
    source = next(source for source in manifest["sources"] if source["id"] == source_id)
    return source["label"] or source_id

def finish(db, run_id: str, is_success: bool) -> None:
    """Commit successful source cursors alongside findings and the finished transcript."""
    row = db.execute("SELECT state FROM digest_run_updates WHERE run_id=?", (run_id,)).fetchone()
    if row and is_success:
        if not db.execute("SELECT 1 FROM conversation WHERE run_id=? AND kind='response' AND length(text)>0", (run_id,)).fetchone():
            raise store.Refused("empty_summary")
        memory = json.loads(row[0])
        db.executemany("INSERT OR REPLACE INTO digest_checkpoints VALUES (?,?,?)",
                       ((key, value["cursor"], value["at"]) for key, value in memory["sources"].items()))
        db.execute("UPDATE digest_migration SET manifest=json_set(manifest,'$.done_date',?,'$.last_run_at',?) WHERE singleton=1",
                   (memory["done_date"], memory["last_run_at"]))
    db.execute("DELETE FROM digest_run_updates WHERE run_id=?", (run_id,))
