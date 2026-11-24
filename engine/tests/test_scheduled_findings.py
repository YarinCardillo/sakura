from datetime import timedelta

import pytest
from sakura.cockpit import proposal_actions, wait_actions
from sakura.connectors import events, registry
from sakura.scheduled import dispatch, findings, runner, store
from test_cockpit_proposal_start import world  # noqa: F401  (the fixture)
from test_scheduled_store import DEFINITION, NOW


def _event_runs():
    connector = registry.Connector("feed-news", "feed", "Industry news", "https://news.example/feed")
    registry._save([connector], {})
    tasks = store.Store()
    when = {"kind": "connector_event", "connector_id": connector.id, "event_kind": "entry_added"}
    for suffix in ("one", "two"):
        tasks.create({**DEFINITION, "title": suffix, "when": when}, suffix, NOW)
    generation = events.revision(connector)
    dispatch.admit_events(tasks, events.EventBatch(connector.id, generation, "entry_added", None, "base", ()), NOW)
    event = events.ConnectorEvent(connector.id, generation, "entry_added", "event-1", "item-1", store.stamp(NOW),
                                  {"id": "item-1", "title": "Article", "url": "https://news.example/article"})
    dispatch.admit_events(tasks, events.EventBatch(connector.id, generation, event.kind, "base", "next", (event,)), NOW)
    return tasks, runner._begin(tasks, NOW), runner._begin(tasks, NOW)


def test_two_runs_merge_one_visible_finding_and_preserve_provenance_after_expiry():
    tasks, first, second = _event_runs()
    for claim, title in ((first, "First title"), (second, "Other title")):
        findings.stage(tasks, claim.id, title, "Read the update", source_ref="event")
        runner._finish(tasks, claim.id, NOW, None)
    (wait,) = findings.pending_waits(tasks)
    assert wait.title == "First title"
    assert wait_actions.inbox_kind(wait) == "proposal"
    assert wait.finding["origin"]["source"] == "Industry news"
    with tasks.connect() as db:
        assert db.execute("SELECT count(*) FROM finding_origins").fetchone()[0] == 2
    runner.prune(tasks, NOW + timedelta(days=31))
    (expired,) = findings.pending_waits(tasks)
    assert expired.id == wait.id and expired.finding["run_id"] is None
    assert proposal_actions.dismiss(expired, "cli:owner")[0] == 0
    assert findings.pending_waits(tasks) == []


def test_invalid_source_and_failed_turn_never_publish():
    tasks, first, second = _event_runs()
    with pytest.raises(store.Refused, match="source_item_unavailable"):
        findings.stage(tasks, first.id, "Invented", "No evidence", source_ref="fabricated")
    findings.stage(tasks, second.id, "Real", "Verified", source_ref="event")
    runner._finish(tasks, second.id, NOW, "adapter_disconnected")
    assert findings.pending_waits(tasks) == []


def test_desktop_and_phone_find_the_same_reservation(repo, world):
    from concurrent.futures import ThreadPoolExecutor

    from sakura.cockpit import api
    from sakura.flow import store as lanes
    from test_cockpit_proposal_start import DESKTOP, PHONE, _act, _lane

    _, controller, fresh = world
    tasks, first, _ = _event_runs()
    findings.stage(tasks, first.id, "Scheduled finding", "Inspect the article", source_ref="event")
    runner._finish(tasks, first.id, NOW, None)
    wait = next(wait for wait in fresh().waits if wait.title == "Scheduled finding")
    row = next(row for row in api.snapshot_document(fresh())["waits"] if row["id"] == wait.id)
    assert row["proposal"]["origin"]["title"] == "one"
    assert row["proposal"]["run_id"] == first.id
    face = (wait, controller, fresh)
    with ThreadPoolExecutor(2) as callers:
        desktop = callers.submit(_act, face, "start", DESKTOP, op=row["proposal"]["op"])
        phone = callers.submit(_act, face, "start", PHONE, op=row["proposal"]["op"])
        assert _lane(desktop.result()) == _lane(phone.result())
    assert len(lanes.task_stores(repo)) == 1
    assert len([row for row in fresh().decided if row["id"] == wait.id]) == 1
