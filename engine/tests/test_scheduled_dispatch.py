from datetime import UTC, date, datetime, timedelta
from zoneinfo import ZoneInfo

from sakura.scheduled import dispatch, store
from test_scheduled_store import DEFINITION, NOW


def test_wake_claims_latest_slot_once_and_records_older_slots():
    tasks = store.Store()
    task = tasks.create(DEFINITION, "clock", NOW)
    moment = NOW + timedelta(minutes=45)
    assert len(dispatch.claim_clock(tasks, moment)) == 1
    assert dispatch.claim_clock(store.Store(), moment) == []
    rows = dispatch.runs(tasks, task.id)
    assert [row["state"] for row in rows] == ["queued", "skipped", "skipped"]
    assert rows[0]["trigger"]["at"] == store.stamp(moment)
    assert dispatch.next_at(task, moment) == store.stamp(moment + timedelta(minutes=15))


def test_daily_uses_first_overlap_and_rolls_gap_forward():
    zone = ZoneInfo("Europe/Rome")
    assert dispatch.daily_at(date(2026, 3, 29), "02:30", zone) == datetime(2026, 3, 29, 1, tzinfo=UTC)
    assert dispatch.daily_at(date(2026, 10, 25), "02:30", zone) == datetime(2026, 10, 25, 0, 30, tzinfo=UTC)


def test_pause_skips_queued_and_resume_does_not_backfill():
    tasks = store.Store()
    task = tasks.create(DEFINITION, "pause", NOW)
    moment = NOW + timedelta(minutes=15)
    assert dispatch.claim_clock(tasks, moment)
    paused = tasks.change(task.id, task.revision, "scheduledpause", {}, moment)
    assert dispatch.runs(tasks, task.id)[0]["state"] == "skipped"
    resumed_at = moment + timedelta(hours=2)
    tasks.change(paused.id, paused.revision, "scheduledresume", {}, resumed_at)
    assert dispatch.claim_clock(tasks, resumed_at) == []


def test_cockpit_run_retry_survives_a_new_task_revision(monkeypatch):
    import json

    from sakura.actors import Actor
    from sakura.cockpit import actions
    from sakura.cockpit.controller import Controller, Done
    from test_cockpit_snapshot import make_snapshot

    tasks = store.Store()
    task = tasks.create(DEFINITION, "retry-task", NOW)
    kicks = []
    monkeypatch.setattr(actions.scheduled_launchd, "kick", lambda: kicks.append("kick"))
    controller = Controller(b"test", make_snapshot, make_snapshot)
    fields = {"key": "scheduledrun", "target": json.dumps(task.key), "input.op": "manual-retry"}
    actor = Actor("app", "owner")
    first = controller.perform(fields, actor, proof_field="confirm", proof_source="cli")
    tasks.change(task.id, task.revision, "scheduledpause", {}, NOW)
    replay = controller.perform(fields, actor, proof_field="confirm", proof_source="cli")
    assert isinstance(first, Done) and isinstance(replay, Done)
    assert first.result.output == replay.result.output and kicks == ["kick"]
