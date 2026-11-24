import json
import subprocess
from datetime import UTC, datetime

import pytest
from flow_support import make_context
from sakura import engine_home
from sakura.actors import Actor
from sakura.cockpit import actions, api, cli, snapshot
from sakura.cockpit.controller import Controller, Done, Refused
from sakura.flow import exits, registry
from sakura.tracks import TrackService
from sakura.tracks.starts import TrackStarts, task_id_for
from test_track_starts import materialize

DESKTOP = Actor("app", "owner")


def perform(controller, key, target, actor=DESKTOP, **inputs):
    fields = {"key": key, "target": json.dumps(target), **{f"input.{name}": value for name, value in inputs.items()}}
    return controller.perform(fields, actor, proof_field="confirm", proof_source="cli")


@pytest.fixture
def cockpit(repo):
    registry.remember(repo)

    def fresh():
        return snapshot.collect(registry.repos(include_missing=True), datetime.now(UTC))

    def run(argv, *, cwd, **kwargs):
        task = next(word.partition("=")[2] for word in argv if word.startswith("--inbox-op="))
        project = engine_home.project_id(cwd)

        def launch(saved):
            materialize(saved)
            return exits.OK, saved.task

        code, created = TrackStarts().execute(project, task, DESKTOP, launch)
        return subprocess.CompletedProcess(argv, code, f"lane {created}\n", "")

    return Controller(b"test", fresh, fresh, run=run)


def new_track(cockpit):
    outcome = perform(cockpit, "tracknew", ["tracks"], op="create", name="Release 1.0", goal="Ship the beta\nKeep rollback.")
    assert isinstance(outcome, Done), outcome
    return json.loads(outcome.result.output)["id"]


def test_registry_parser_and_bridge_accept_track_fields(cockpit, repo, capsys):
    track = new_track(cockpit)
    target = json.dumps(["track", track])
    args = cli.act_parser().parse_args(["trackset", target, "--op=edit", "--summary=First\nSecond",
                                       "--goal=New goal", "--status=paused", "--json"])
    assert cli._act_fields(args)["input.summary"] == "First\nSecond"
    assert actions.ACTIONS["trackset"].optional == ("name", "goal", "summary", "status", "clear")
    assert cli.main(["act", "trackset", target, "--op=edit", "--summary=First\nSecond", "--goal=New goal",
                     "--status=paused", "--json"]) == exits.OK
    assert json.loads(capsys.readouterr().out)["result"] == "done"
    [edited] = cockpit.fresh_snapshot().tracks.rows
    assert (edited["goal"], edited["summary"], edited["status"]) == ("New goal", "First\nSecond", "paused")
    assert isinstance(perform(cockpit, "trackset", ["track", track], op="clear", clear="summary"), Done)
    assert cockpit.fresh_snapshot().tracks.rows[0]["summary"] == ""
    [project] = api.snapshot_document(cockpit.fresh_snapshot())["projects"]
    [start] = [action for action in project["actions"] if action["key"] == "L"]
    assert start["optional"] == ["preset", "level", "track", "op"]
    assert project["track_choices"] == [{"id": track, "name": "Release 1.0"}]


def test_link_move_unlink_and_close_refusal_preserve_values(cockpit, repo):
    track = new_track(cockpit)
    target = ["track", track]
    task = make_context(repo, state={"title": "Verify builds"}).store
    assert isinstance(perform(cockpit, "tracklink", target, op="link", project=str(repo), task=task.task_id), Done)
    refused = perform(cockpit, "trackset", target, op="close", status="closed", name="Uncommitted")
    assert api.action_document(refused) == {
        "result": "refused", "reason": "Finish or remove these linked lanes before closing: Verify builds",
        "reason_code": "open_lanes", "reason_args": {"labels": ["Verify builds"]},
    }
    assert cockpit.fresh_snapshot().tracks.rows[0]["name"] == "Release 1.0"
    assert isinstance(perform(cockpit, "trackunlink", target, op="unlink", project=str(repo), task=task.task_id), Done)
    assert isinstance(perform(cockpit, "trackset", target, op="close", status="closed"), Done)


def test_track_validation_refuses_invalid_fields_and_target(cockpit, repo):
    track = new_track(cockpit)
    conversation = json.dumps({"provider": "provider", "account": "a", "session": "a" * 257})
    invalid = [
        ("trackdecision", {"text": "x" * 501}),
        ("trackset", {"summary": "x" * 4001}),
        ("trackset", {"clear": "name"}),
        ("trackset", {"summary": "yes", "clear": "summary"}),
        ("tracklink", {"conversation": conversation, "title": "Meeting"}),
        ("trackunlink", {"project": str(repo), "task": "demo", "conversation": conversation}),
        ("tracksettle", {"decision": "1", "answer": "x" * 2001}),
    ]
    for key, inputs in invalid:
        outcome = perform(cockpit, key, ["track", track], op="bad", **inputs)
        assert isinstance(outcome, Refused) and outcome.reason_code == "invalid_input", outcome
    assert isinstance(perform(cockpit, "trackset", ["track", "t-01234567"], op="edit", goal="Change"), Refused)
    assert len(TrackService().read_audit(track)) == 1


def test_tracked_start_returns_created_id_and_replays_after_folder_loss(cockpit, repo):
    track = new_track(cockpit)
    target = ["project", str(repo)]
    inputs = {"track": track, "op": "start", "text": "Open a lane\nKeep the checks"}
    started = perform(cockpit, "L", target, **inputs)
    assert isinstance(started, Done) and started.result.output == task_id_for("start")
    repo.rename(repo.with_name("moved"))
    refused = perform(cockpit, "L", target, **{**inputs, "op": "another"})
    assert isinstance(refused, Refused) and "missing" in refused.reason.lower()
    assert perform(cockpit, "L", target, **inputs).result.output == started.result.output
    assert len(cockpit.fresh_snapshot().tracks.rows[0]["lanes"]) == 1


def test_recovery_is_scoped_to_track_and_operation(cockpit, repo, monkeypatch):
    track = new_track(cockpit)
    other = TrackService().create("Other", "Goal", actor=DESKTOP, op_id="other")
    target = ["track", track]
    project = engine_home.project_id(repo)
    from sakura.tracks.starts import LaunchInputs

    service = TrackService()
    first = service.reserve_start(track, project, LaunchInputs("First"), DESKTOP, "first")
    second = service.reserve_start(track, project, LaunchInputs("Second"), DESKTOP, "second")
    assert isinstance(perform(cockpit, "trackretry", ["track", other.id], startop=first.op_id, op="retry"), Refused)
    started = perform(cockpit, "trackretry", target, startop=first.op_id, op="retry")
    assert isinstance(started, Done) and started.result.output == first.task
    assert perform(cockpit, "trackretry", target, startop=first.op_id, op="retry").result.output == first.task
    conflict = perform(cockpit, "trackretry", target, startop=second.op_id, op="retry")
    assert isinstance(conflict, Refused) and conflict.reason_code == "operation_conflict"
    assert isinstance(perform(cockpit, "trackremove", target, startop=second.op_id, op="remove"), Done)
    monkeypatch.setattr(cockpit, "_run", lambda *args, **kwargs: pytest.fail("removed start relaunched"))
    assert isinstance(perform(cockpit, "trackretry", target, startop=second.op_id, op="late"), Refused)
    assert isinstance(perform(cockpit, "trackremove", target, startop=second.op_id, op="remove"), Done)


def test_phone_and_desktop_race_and_cross_target_replay(cockpit):
    from concurrent.futures import ThreadPoolExecutor

    track = TrackService().create("Release", "Ship", actor=DESKTOP, op_id="new")
    decision = TrackService().add_decision(track.id, "Ready?", actor=DESKTOP, op_id="question")
    [wait] = cockpit.fresh_snapshot().waits
    phone = Actor("overlay", "owner", "phone")
    fields = {"key": "tracksettle", "target": wait.id, "input.op": "phone-answer", "input.answer": "Ship"}

    with ThreadPoolExecutor(2) as clients:
        from_phone = clients.submit(cockpit.perform, fields, phone, proof_field="proof", proof_source="overlay")
        from_desktop = clients.submit(perform, cockpit, "tracksettle", ["track", track.id],
                                      op="desktop-answer", answer="Hold", decision=str(decision.number))
        outcomes = [from_phone.result(), from_desktop.result()]

    assert sum(isinstance(outcome, Done) for outcome in outcomes) == 1
    assert [outcome.reason_code for outcome in outcomes if isinstance(outcome, Refused)] == ["decision_settled"]
    [done] = cockpit.fresh_snapshot().decided
    assert done["id"] == wait.id
    op = f"{done['place']}-answer"
    replay = perform(cockpit, "tracksettle", json.loads(wait.id), op=op, answer=done["text"])
    assert isinstance(replay, Done)
    assert cockpit.fresh_snapshot().decided == [done]
    changed = perform(cockpit, "tracksettle", json.loads(wait.id), op=op, answer="Changed")
    assert isinstance(changed, Refused) and changed.reason_code == "operation_conflict"
    unauthorized = perform(cockpit, "tracksettle", json.loads(wait.id), Actor("master", "session"),
                           op=op, answer=done["text"])
    assert isinstance(unauthorized, Refused) and unauthorized.reason_code == "forbidden"
