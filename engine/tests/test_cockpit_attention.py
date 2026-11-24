import json
from datetime import UTC, datetime, timedelta

from flow_support import make_context
from sakura import engine_home
from sakura.actors import Actor
from sakura.cockpit import api, attention, snapshot
from sakura.cockpit.controller import PROOF_SOURCE_CLI, Controller, Done, Refused

OWNER = Actor("app", "owner")


def _collect(repo, now=None):
    return snapshot.collect([repo], now or datetime.now(UTC))


def _perform(repo, wait, key, **inputs):
    def fresh():
        return _collect(repo)

    controller = Controller(b"test", fresh, fresh)
    fields = {"key": key, "target": wait.id, **{f"input.{name}": value for name, value in inputs.items()}}
    return controller.perform(fields, OWNER, proof_field="confirm", proof_source=PROOF_SOURCE_CLI)


def _act(repo, wait, key, **inputs):
    outcome = _perform(repo, wait, key, **inputs)
    assert isinstance(outcome, Done), outcome
    assert outcome.result.exit_code == 0


def _lane(repo, name="demo"):
    return make_context(repo, task=name, state={"status": "awaiting_approval"}).store


def _row(repo, now=None):
    [row] = api.snapshot_document(_collect(repo, now))["waits"]
    return row


def _file():
    return engine_home.state_dir() / "cockpit" / "attention.json"


def test_read_mark_round_trip(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "read")
    assert _row(repo)["is_read"] is True
    assert _file().stat().st_mode & 0o777 == 0o600
    _act(repo, wait, "unread")
    assert _row(repo)["is_read"] is False


def test_snapshot_keeps_snoozed_wait_with_its_deadline_until_it_wakes(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    until = datetime.now(UTC).replace(microsecond=0) + timedelta(hours=1)
    _act(repo, wait, "snooze", until=until.isoformat())
    current = _collect(repo)
    assert current.visible_waits == []
    [row] = api.snapshot_document(current)["waits"]
    assert row["id"] == wait.id
    assert row["is_snoozed"] is True
    assert row["snoozed_until"] == until.strftime("%Y-%m-%dT%H:%M:%SZ")
    assert _row(repo, until)["is_snoozed"] is False
    assert [awake.id for awake in _collect(repo, until).visible_waits] == [wait.id]


def test_unsnooze_returns_wait_to_inbox_without_marking_it_unread_or_resolving_it(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "read")
    _act(repo, wait, "snooze", until="1h")
    _act(repo, wait, "unsnooze")
    current = _collect(repo)
    [row] = api.snapshot_document(current)["waits"]
    assert (row["id"], row["is_read"], row["is_snoozed"], row["snoozed_until"]) == (wait.id, True, False, None)
    assert "unsnooze" not in {action["key"] for action in row["actions"]}
    assert [awake.id for awake in current.visible_waits] == [wait.id]
    assert current.decided == []


def test_snoozed_wait_offers_unsnooze_without_inputs_or_confirmation(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "snooze", until="1h")
    [action] = [action for action in api._wait(_collect(repo).waits[0])["actions"] if action["key"] == "unsnooze"]
    assert action == {"key": "unsnooze", "label": "Unsnooze", "needs": [], "optional": [], "confirm": False}


def test_unsnooze_refuses_a_wait_that_is_already_awake(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    outcome = _perform(repo, wait, "unsnooze")
    assert isinstance(outcome, Refused)


def test_new_wait_on_same_lane_not_snoozed(repo):
    task = _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "snooze", until="nextweek")
    task.update_state(lambda state: state.update(status="failed", reason="failed"))
    row = _row(repo)
    assert (row["kind"], row["is_read"], row["snoozed_until"]) == ("blocked", False, None)


def test_marks_never_resolve(repo):
    _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "snooze", until="tomorrow")
    _act(repo, wait, "read")
    current = _collect(repo)
    assert [row.id for row in current.waits] == [wait.id]
    assert current.decided == []


def test_marks_are_pruned_by_age_in_a_separate_pass(repo):
    task = _lane(repo)
    [wait] = _collect(repo).waits
    _act(repo, wait, "read")
    task.update_state(lambda state: state.update(status="report_ready"))
    _lane(repo, "next")
    [next_wait] = _collect(repo).waits
    _act(repo, next_wait, "read")
    assert set(json.loads(_file().read_text())) == {wait.id, next_wait.id}
    now = datetime.now(UTC)
    attention.update(wait.id, "read", now - attention.RETENTION - timedelta(days=1), None)
    attention.prune(now)
    assert set(json.loads(_file().read_text())) == {next_wait.id}


def test_a_later_blockage_does_not_inherit_a_blocked_waits_snooze(repo):
    task = _lane(repo)
    first = {"phase": "build", "status": "blocked", "at": "2026-11-21T10:00:00+00:00"}
    repeated = {"phase": "build", "status": "blocked", "at": "2026-11-21T11:00:00+00:00"}
    task.update_state(lambda state: state.update(status="blocked", reason="blocked", history=[first]))
    [wait] = _collect(repo).waits
    _act(repo, wait, "snooze", until="1h")
    task.update_state(lambda state: state.update(history=[first, repeated]))
    row = _row(repo)
    assert row["id"] != wait.id and row["snoozed_until"] is None


def test_an_older_snapshot_update_preserves_a_new_waits_snooze(repo):
    _lane(repo)
    old = _collect(repo)
    [first] = old.waits
    _lane(repo, "new")
    new = next(wait for wait in _collect(repo).waits if wait.task == "new")
    _act(repo, new, "snooze", until="1h")

    controller = Controller(b"test", lambda: old, lambda: old)
    fields = {"key": "read", "target": first.id}
    outcome = controller.perform(fields, OWNER, proof_field="confirm", proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Done) and outcome.result.exit_code == 0
    marks = {wait.id: wait for wait in _collect(repo).waits}
    assert marks[first.id].is_read and marks[new.id].is_snoozed


def test_track_marks_survive_rename_and_partial_reads(repo, monkeypatch):
    import sqlite3

    from sakura.tracks import TrackService
    from sakura.tracks.database import Database

    service = TrackService()
    track = service.create("Delivery", "Ship", actor=OWNER, op_id="new")
    service.add_decision(track.id, "Which approach?", actor=OWNER, op_id="question")
    [wait] = _collect(repo).waits
    _act(repo, wait, "read")
    _act(repo, wait, "snooze", until="1h")
    service.update(track.id, name="Release", status="paused", actor=OWNER, op_id="rename")
    service.add_decision(track.id, "What next?", actor=OWNER, op_id="another-question")
    _lane(repo)
    lane_wait = next(found for found in _collect(repo).waits if found.source == "flow")
    with monkeypatch.context() as partial:
        def unavailable(*args, **kwargs):
            raise sqlite3.OperationalError("fixture read failure")

        partial.setattr(Database, "connect", unavailable)
        _act(repo, lane_wait, "read")
    found = {row.detail: row for row in _collect(repo).waits}
    first = found["Which approach?"]
    assert first.id == wait.id and first.is_read and first.is_snoozed
    assert not found["What next?"].is_read
    assert _collect(repo).decided == []
