import json
from datetime import UTC, datetime

import pytest
from sakura.scheduled import store

NOW = datetime(2026, 11, 22, 8, tzinfo=UTC)
DEFINITION = {"title": "Morning summary", "prompt": "Summarize changes.",
              "connector_ids": [], "when": {"kind": "interval", "minutes": 15},
              "project": None, "preset": None, "account": None, "enabled": True}


def test_create_is_repeat_safe_and_edits_are_revision_bound():
    tasks = store.Store()
    first = tasks.create(DEFINITION, "create-1", NOW)
    assert tasks.create(DEFINITION, "create-1", NOW) == first
    with pytest.raises(store.Refused, match="operation_conflict"):
        tasks.create({**DEFINITION, "title": "Different"}, "create-1", NOW)
    second = tasks.change(first.id, first.revision, "scheduledpause", {}, NOW)
    assert not second.definition["enabled"]
    with pytest.raises(store.Refused, match="stale_task"):
        tasks.change(first.id, first.revision, "scheduledupdate", {"definition": json.dumps(DEFINITION)}, NOW)
    assert tasks.tasks() == [second]


@pytest.mark.parametrize("extra", [{"secret": True}, {"when": {"kind": "interval", "minutes": True}},
                                     {"connector_ids": ["removed"]}, {"may_start_lanes": True}])
def test_invalid_definition_cannot_be_saved(extra):
    tasks = store.Store()
    with pytest.raises(store.Refused):
        tasks.create({**DEFINITION, **extra}, "invalid-1", NOW)
    assert tasks.tasks() == []


def test_proposal_binds_exact_definition_and_is_consumed_once():
    tasks = store.Store()
    pending = tasks.propose({**DEFINITION, "prompt": "First line\nSecond line"}, "master:session", NOW)
    assert json.loads(pending.preview) == pending.definition
    assert pending.definition["may_start_lanes"] is False
    with pytest.raises(store.Refused, match="stale_approval"):
        tasks.decide(pending.id, "0", True, NOW)
    accepted = tasks.decide(pending.id, pending.revision, True, NOW)
    with pytest.raises(store.Refused, match="stale_approval"):
        tasks.decide(pending.id, pending.revision, True, NOW)
    assert tasks.tasks() == [accepted]


def test_disconnected_connector_is_visible_without_changing_operator_setting():
    from sakura.cockpit.api import scheduled_document
    from sakura.connectors import registry

    registry._save([registry.Connector("feed-news", "feed", "News", connected=False)], {})
    task = store.Store().create({**DEFINITION, "connector_ids": ["feed-news"]}, "create-2", NOW)
    (shown,) = scheduled_document()["tasks"]
    assert (shown["id"], shown["state"], shown["connector_label"], shown["enabled"]) == (
        task.id, "connector_unavailable", "News", True,
    )


def test_unreadable_store_exposes_fixed_problem():
    from sakura.cockpit.api import scheduled_document

    path = store.Store().path
    path.parent.mkdir(mode=0o700)
    path.write_text("unreadable")
    path.chmod(0o600)
    assert scheduled_document()["problem"] == store.STORE_PROBLEM
