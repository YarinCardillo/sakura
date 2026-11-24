from datetime import UTC, datetime

import pytest
from sakura.connectors import events, kinds, registry
from sakura.scheduled import dispatch, store
from test_scheduled_store import DEFINITION

NOW = datetime(2026, 11, 23, tzinfo=UTC)


class Scheduler:
    def __init__(self, bindings):
        self.bindings = bindings
        self.runs = {}
        self.paused = {}
        self.fail = False

    def subscriptions(self):
        return self.bindings

    def admit(self, task_id, event):
        self.runs.setdefault((task_id, event.key), event)
        if self.fail:
            raise OSError("crash after admission")

    def pause(self, task_id, code, args):
        self.paused.setdefault(task_id, (code, args))


def _feed(op):
    fields = {"kind": "feed", "label": "News", "address": "https://example.test/feed"}
    return registry.mutate("add", op, kinds.Ports(), NOW, fields=fields)


def test_event_baseline_replay_and_overlapping_subscriptions():
    first, second = _feed("one"), _feed("two")
    scheduler = Scheduler([events.Subscription(task, row["id"], "new_item")
                           for task in ("morning", "triage") for row in (first, second)])
    observations = [events.Observation("old", "old", NOW.isoformat())]
    def reader(*args):
        return list(observations)

    events.poll(scheduler, NOW, reader=reader)
    assert not scheduler.runs
    observations.append(events.Observation("new", "new", NOW.isoformat()))
    scheduler.fail = True
    with pytest.raises(OSError):
        events.poll(scheduler, NOW, reader=reader)
    scheduler.fail = False
    events.poll(scheduler, NOW, reader=reader)
    events.poll(scheduler, NOW, reader=reader)
    assert len(scheduler.runs) == 2
    assert {task for task, _ in scheduler.runs} == {"morning", "triage"}
    assert {event.item_ref for event in scheduler.runs.values()} == {"new"}


def test_event_batch_from_old_revision_is_never_admitted():
    row = _feed("one")
    binding = events.Subscription("morning", row["id"], "new_item")
    scheduler = Scheduler([binding])
    events.poll(scheduler, NOW, reader=lambda *args: [])

    def read(*args):
        registry.mutate("revoke", "revoke", kinds.Ports(), NOW, connector_id=row["id"], revision=1)
        return [events.Observation("not-admitted", "item", NOW.isoformat())]

    events.poll(scheduler, NOW, reader=read)
    assert not scheduler.runs
    assert scheduler.paused == {"morning": ("connector_disconnected", {"label": "News"})}
    assert events.claim(scheduler, "morning", {row["id"]: 0}, lambda: pytest.fail("must pause")) is None


def test_event_assignment_transition_requires_a_new_occurrence():
    row = _feed("one")
    scheduler = Scheduler([events.Subscription("morning", row["id"], "new_item")])
    observed = [events.Observation("occurrence-1", "same", NOW.isoformat(), True, "issue")]
    events.poll(scheduler, NOW, reader=lambda *args: list(observed))
    observed[:] = [events.Observation("occurrence-2", "same", NOW.isoformat(), True, "issue")]
    events.poll(scheduler, NOW, reader=lambda *args: list(observed))
    assert not scheduler.runs
    observed[:] = [events.Observation("occurrence-3", "same", NOW.isoformat(), False, "issue")]
    events.poll(scheduler, NOW, reader=lambda *args: list(observed))
    observed[:] = [events.Observation("occurrence-4", "same", NOW.isoformat(), True, "issue")]
    events.poll(scheduler, NOW, reader=lambda *args: list(observed))
    assert len(scheduler.runs) == 1


def test_reconnect_does_not_release_a_queued_task_until_explicit_resume():
    row = _feed("one")
    scheduler = Scheduler([])
    connector = row["id"]
    registry.mutate("revoke", "disconnect", kinds.Ports(), NOW, connector_id=connector, revision=1)
    registry.mutate("edit", "reconnect", kinds.Ports(), NOW, connector_id=connector, revision=2,
                    fields={"kind": "feed", "label": "News", "address": "https://example.test/feed"})
    assert events.claim(scheduler, "morning", {connector: 0}, lambda: "claimed") is None
    assert scheduler.paused == {"morning": ("connector_disconnected", {"label": "News"})}
    assert events.claim(scheduler, "morning", {connector: 2}, lambda: "claimed") == "claimed"



def test_event_baseline_fanout_and_duplicate_batch():
    connector = registry.Connector("feed-news", "feed", "News", "https://news.example/feed")
    registry._save([connector], {})
    tasks = store.Store()
    when = {"kind": "connector_event", "connector_id": connector.id, "event_kind": "entry_added"}
    first = tasks.create({**DEFINITION, "when": when}, "event-1", NOW)
    second = tasks.create({**DEFINITION, "when": when}, "event-2", NOW)
    event = events.ConnectorEvent(connector.id, events.revision(connector), "entry_added", "source-occurrence",
                                  "source-item", store.stamp(NOW), {"title": "New article"})
    assert dispatch.admit_events(tasks, events.EventBatch(connector.id, event.revision, event.kind, None, "base", ()), NOW) == []
    batch = events.EventBatch(connector.id, event.revision, event.kind, "base", "next", (event,))
    assert len(dispatch.admit_events(tasks, batch, NOW)) == 2
    assert dispatch.admit_events(tasks, batch, NOW) == []
    assert len(dispatch.runs(tasks, first.id)) == len(dispatch.runs(tasks, second.id)) == 1


def test_disconnect_skips_accepted_queue_and_requires_explicit_resume():
    from datetime import timedelta

    from sakura.cockpit.api import scheduled_document
    from sakura.connectors.kinds import Ports

    connector = registry.Connector("feed-news", "feed", "News", "https://news.example/feed")
    registry._save([connector], {})
    tasks = store.Store()
    task = tasks.create({**DEFINITION, "connector_ids": [connector.id]}, "disconnect", NOW)
    dispatch.claim_clock(tasks, NOW + timedelta(minutes=15))
    registry.disconnect(connector.id)
    paused = tasks.get(task.id)
    assert paused.connector_pause["label"] == "News"
    assert paused.definition["enabled"]
    assert dispatch.runs(tasks, task.id)[0]["state"] == "skipped"
    assert scheduled_document()["tasks"][0]["state"] == "connector_unavailable"
    registry.connect("feed", "News", connector.target, Ports(), NOW)
    assert scheduled_document()["tasks"][0]["reason"] == "connector_reconnected"
    assert tasks.change(task.id, paused.revision, "scheduledresume", {}, NOW).definition["enabled"]


def test_scheduled_poll_baselines_scoped_feed_and_claims_each_new_item_once():
    ports = kinds.Ports()
    receipt = registry.mutate("add", "scoped-feed", ports, NOW, fields={
        "kind": "feed", "label": "Scoped news", "address": "https://news.example/feed",
    })
    tasks = store.Store()
    when = {"kind": "connector_event", "connector_id": receipt["id"], "event_kind": "new_item"}
    task = tasks.create({**DEFINITION, "when": when}, "scoped-task", NOW)
    items = ["first"]

    def fetch(url, timeout):
        entries = "".join(f"<item><guid>{item}</guid><title>{item}</title></item>" for item in items)
        return f"<rss><channel>{entries}</channel></rss>".encode()

    dispatch.poll_events(tasks, NOW, fetch)
    assert dispatch.runs(tasks, task.id) == []
    items.append("second")
    dispatch.poll_events(tasks, NOW, fetch)
    dispatch.poll_events(tasks, NOW, fetch)
    [run] = dispatch.runs(tasks, task.id)
    assert run["state"] == "queued" and run["trigger"]["event_kind"] == "new_item"
