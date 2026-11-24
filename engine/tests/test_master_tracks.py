import subprocess

import pytest
from sakura import actors, engine_home, grants
from sakura.cockpit import api, snapshot
from sakura.cockpit.controller import Controller
from sakura.flow import exits, registry
from sakura.master import server
from sakura.master.tools import MasterTools
from sakura.tracks import TrackService
from sakura.tracks.domain import Refusal, RefusalCode
from sakura.tracks.starts import TrackStarts, task_id_for
from test_cockpit_actions import Spawns
from test_cockpit_snapshot import NIGHT, create_task
from test_master_server import MASTER_ACTOR, SESSION, World, _json, _opened_by, _served
from test_track_starts import materialize

TRACK_TOOLS = {"tracks", "track_new", "track_update", "track_decision_add", "track_link_lane"}


@pytest.fixture
def world(repo, tmp_path):
    return World(repo, tmp_path)


def test_track_grants_respect_explicit_overrides_and_exclude_lane_roles(world):
    assert TRACK_TOOLS <= set(_served(world))
    path = engine_home.root() / grants.GRANTS_FILE
    path.write_text('[master]\ntools = ["waits", "track_new"]\n')
    configured = grants.load(engine_home.root(), ())
    allowed = configured.for_class("master").tools
    served = server.build({"role": "master", "tools": list(allowed)}, world.controller, SESSION)
    assert [tool.spec["name"] for tool in served] == ["waits", "track_new"]
    for role in set(grants.CLASSES) - {"master"}:
        assert not TRACK_TOOLS.intersection(configured.for_class(role).tools)
    path.write_text('[builder]\ntools = ["track_new"]\n')
    with pytest.raises(grants.GrantsError, match="unknown tool"):
        grants.load(engine_home.root(), ())


def test_permitted_master_mutations_audit_and_replay_as_the_bound_session(world):
    served = _served(world)
    create = {"op": "create", "name": "Release", "goal": "Ship the beta"}
    created, failed = _json(served["track_new"].call(create))
    assert not failed
    track = created["track"]["id"]
    edit = {"track": track, "op": "update", "goal": "Ship the release", "summary": "Tests pass.\nReady.",
            "status": "paused"}
    updated, failed = _json(served["track_update"].call(edit))
    assert not failed and updated["track"]["status"] == "paused"
    assert _json(served["track_update"].call(edit)) == (updated, False)
    assert _json(served["track_update"].call({**edit, "summary": "Different"}))[0]["reason_code"] == "operation_conflict"
    other = MasterTools(world.controller, "another")
    assert _json(other.track_update(edit))[0]["reason_code"] == "forbidden"
    decision, failed = _json(served["track_decision_add"].call({"track": track, "op": "question", "text": "Go live?"}))
    assert not failed and decision["decision"]["number"] == 1
    task = create_task(world.repo, "mine", status="running")
    _opened_by(task, MASTER_ACTOR)
    link = {"track": track, "lane": f"{world.project}/mine", "op": "link"}
    assert _json(other.track_link_lane(link))[0]["reason_code"] == "forbidden"
    linked, failed = _json(served["track_link_lane"].call(link))
    assert not failed and linked["lane"]["task"] == "mine"
    audit = TrackService().read_audit(track)
    assert [row.op_id for row in audit] == ["create", "update", "question", "link"]
    assert all(row.actor == row.actor_session == MASTER_ACTOR and row.actor_class == "master" for row in audit)
    listed, failed = _json(served["tracks"].call({}))
    assert not failed and [row["id"] for row in listed["tracks"]] == [track]
    waits, failed = _json(served["waits"].call({}))
    [question] = [row for row in waits["waits"] if row["source"] == "track"]
    assert not failed and question["track"] == {"id": track, "name": "Release"} and question["detail"] == "Go live?"
    assert _json(served["lanes"].call({}))[0]["waiting_on_person"] == 0


def test_master_arguments_and_service_refuse_person_authority(world):
    service = TrackService()
    person = actors.Actor(actors.APP_SOURCE, "person")
    track = service.create("Release", "Ship", actor=person, op_id="create")
    decision = service.add_decision(track.id, "Ship?", actor=person, op_id="question")
    service.settle_decision(track.id, decision.number, "Yes", actor=person, op_id="answer")
    change = {"track": track.id, "op": "edit"}
    for edits in ({}, {"name": "Rename"}, {"status": "closed"}, {"goal": "Ship", "actor": "app:person"}):
        assert _json(world.tools.track_update({**change, **edits}))[1]
    with pytest.raises(Refusal) as caught:
        service.settle_decision(track.id, decision.number, "Yes", actor=world.tools.actor, op_id="answer")
    assert caught.value.code == RefusalCode.FORBIDDEN
    service.update(track.id, actor=person, op_id="close", status="closed")
    assert _json(world.tools.track_update({**change, "status": "open"}))[0]["reason_code"] == "forbidden"


@pytest.mark.parametrize("tracked", [False, True])
def test_lane_open_returns_the_created_cockpit_key_with_interleaved_starts(repo, tmp_path, tracked):
    from conftest import git

    other = tmp_path / "other"
    git(tmp_path, "clone", "-q", str(repo), str(other))
    registry.remember(other)
    made = task_id_for("start") if tracked else "actual"
    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        create_task(other, made, status="running")
        create_task(repo, "interleaved", status="running")
        if tracked:
            def launch(saved):
                materialize(saved, pending=False)
                return exits.OK, saved.task

            assert TrackStarts().execute(world.project, made, world.tools.actor, launch) == (exits.OK, made)
        else:
            create_task(repo, made, status="running")
        return subprocess.CompletedProcess(argv, exits.OK, f"lane {made}\n", "")

    world = World(repo, tmp_path, run)
    arguments = {"project": world.project, "text": "Run checks"}
    if tracked:
        track = TrackService().create("Release", "Ship", actor=world.tools.actor, op_id="create")
        arguments.update(track=track.id, op="start")
    opened, failed = _json(world.tools.lane_open(arguments))
    assert not failed
    item = next(item for item in world.controller.fresh_snapshot().items if item.key == ("flow", str(repo), made))
    assert opened == {"result": "opened", "lane": f"{world.project}/{made}", "id": api.target_id(item),
                      "lanes_opened_meanwhile": []}
    if tracked:
        assert _json(world.tools.lane_open(arguments)) == (opened, False)
        assert _json(world.tools.lane_open({**arguments, "text": "Changed"}))[0]["reason_code"] == "operation_conflict"
        repo.rename(tmp_path / "moved")
        assert _json(world.tools.lane_open(arguments)) == (opened, False)
    assert len(calls) == 1


def test_missing_project_keeps_other_project_lookups_working(world, tmp_path):
    missing = tmp_path / "missing"

    def collect():
        return snapshot.collect([missing, world.repo], NIGHT)

    tools = MasterTools(Controller(b"", collect, collect, run=world.spawns), SESSION)
    refused, failed = _json(tools.lane_open({"project": engine_home.project_id_for_path(missing), "text": "Start"}))
    assert failed and "folder is missing" in refused["reason"]
    board, failed = _json(tools.board_read({"project": world.project}))
    assert not failed and board["posts"] == []


def test_tracked_lane_open_refuses_partial_inputs_and_keeps_a_blocked_start_reserved(repo, tmp_path):
    world = World(repo, tmp_path, Spawns(returncode=exits.USAGE))
    track = TrackService().create("Release", "Ship", actor=world.tools.actor, op_id="create")
    base = {"project": world.project, "text": "Run checks"}
    for partial in ({"track": track.id}, {"op": "start"}):
        document, failed = _json(world.tools.lane_open({**base, **partial}))
        assert failed and document["reason_code"] == "invalid_input"
    assert world.spawns.calls == []
    document, failed = _json(world.tools.lane_open({**base, "track": track.id, "op": "start"}))
    assert failed and document["reason_code"] == "lane_unavailable"
    assert document["reason"] == "The lane did not start. Retry the reservation"
    [lane] = world.controller.fresh_snapshot().tracks.rows[0]["lanes"]
    assert lane["did_not_start"] and lane["startop"] == "start"
