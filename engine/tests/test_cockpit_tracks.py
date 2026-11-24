import json
from dataclasses import replace
from datetime import UTC, datetime, timedelta

from flow_support import make_context
from sakura import engine_home
from sakura.actors import Actor
from sakura.cockpit import api, snapshot
from sakura.flow import registry
from sakura.tracks import TrackService
from sakura.tracks.starts import LaunchInputs, TrackStarts

OWNER = Actor("app", "owner")
PHONE = Actor("overlay", "owner", "phone")
NOW = datetime(2026, 11, 23, 10, tzinfo=UTC)


def document(*repos, now=NOW):
    return api.snapshot_document(snapshot.collect(repos, now))


def test_empty_tracks_keeps_v1_project_and_lane_controls(repo):
    task = make_context(repo).store
    row = document(repo)
    assert row["api"] == 1 and row["tracks"] == []
    assert [action["key"] for action in row["track_actions"]] == ["tracknew"]
    [project] = row["projects"]
    [start] = [action for action in project["actions"] if action["key"] == "L"]
    assert start["needs"] == ["text"] and start["optional"] == ["preset", "level"]
    assert project["track_choices"] == []
    assert row["items"][0]["key"] == ["flow", str(repo), task.task_id]
    assert row["items"][0]["track"] is None
    assert not (engine_home.root() / "tracks").exists()


def test_members_keep_identity_when_missing_gone_or_interrupted(repo):
    registry.remember(repo)
    project = engine_home.project_id(repo)
    task = make_context(repo, task="live", state={"title": "Live lane"}).store
    gone = make_context(repo, task="gone", state={"title": "Gone lane"}).store
    service = TrackService()
    track = service.create("Delivery", "Ship", actor=OWNER, op_id="new")
    service.link_lane(track.id, repo, task.task_id, actor=OWNER, op_id="link-live")
    service.link_lane(track.id, repo, gone.task_id, actor=OWNER, op_id="link-gone")
    reservation = service.reserve_start(track.id, project, LaunchInputs("Try this"), OWNER, "start")
    TrackStarts(service).execute(project, reservation.task, OWNER, lambda _: (1, ""))
    gone.root.rename(gone.root.parent / ".retired")
    row = document(repo)
    [found] = row["tracks"]
    ref = {"id": track.id, "name": track.name}
    assert row["projects"][0]["track_choices"] == [ref]
    assert next(item for item in row["items"] if item["key"][-1] == "live")["track"] == ref
    members = {lane["task"]: lane for lane in found["lanes"]}
    assert members["live"]["key"] == ["flow", str(repo), "live"]
    assert members["live"]["state"] == "live"
    assert members["gone"]["gone"] and members["gone"]["title"] == "Gone lane"
    pending = members[reservation.task]
    assert pending["did_not_start"] and pending["state"] == "interrupted" and not pending["gone"]
    assert {action["key"] for action in pending["actions"]} == {"trackretry", "trackremove"}
    repo.rename(repo.with_name("moved"))
    [missing] = document(repo)["tracks"]
    assert missing["projects"][0] == {"id": project, "folder": str(repo), "name": repo.name, "folder_missing": True}
    members = {lane["task"]: lane for lane in missing["lanes"]}
    assert members["live"]["folder_missing"] and members["live"]["state"] == "live"
    assert [action["key"] for action in members[reservation.task]["actions"]] == ["trackremove"]
    repo.with_name("moved").rename(repo)
    assert document(repo)["tracks"][0]["lanes"] == found["lanes"]


def test_closed_track_decisions_and_done_keep_the_same_wait_and_first_place():
    service = TrackService(clock=lambda: NOW)
    track = service.create("Delivery", "Ship", actor=OWNER, op_id="new")
    decision = service.add_decision(track.id, "Which approach?\nKeep it reversible.", actor=OWNER, op_id="question")
    service.update(track.id, status="closed", actor=OWNER, op_id="close")
    wire = document()
    [wait] = wire["waits"]
    [row] = wire["tracks"]
    assert row["status"] == "closed" and row["decisions"][0]["wait_id"] == wait["id"]
    assert json.loads(wait["id"]) == ["wait", "track", track.id, "track_decision", "1"]
    assert (wait["kind"], wait["subkind"], wait["source"], wait["section"]) == (
        "question", "track_decision", "track", "needs_you")
    assert wait["group"]["label"] == "Delivery" and wait["track"] == {"id": track.id, "name": track.name}
    service.settle_decision(track.id, decision.number, "The first approach", actor=PHONE, op_id="settle")
    service.settle_decision(track.id, decision.number, "The first approach", actor=OWNER, op_id="settle")
    resolved = document()
    assert resolved["waits"] == []
    [entry] = resolved["decided"]
    assert (entry["id"], entry["title"], entry["text"], entry["actor"], entry["place"]) == (
        wait["id"], wait["title"], "The first approach", "overlay:owner:phone", "phone")
    [resolved_decision] = resolved["tracks"][0]["decisions"]
    assert resolved_decision["wait_id"] is None and resolved_decision["answer"] == "The first approach"
    assert not (engine_home.state_dir() / "cockpit" / "decided.jsonl").exists()
    assert document(now=NOW + timedelta(days=8))["decided"] == []


def test_track_read_failure_leaves_cockpit_available(repo):
    service = TrackService()
    service.create("Delivery", "Ship", actor=OWNER, op_id="new")
    (engine_home.root() / "tracks" / "tracks.db").write_bytes(b"Private broken store")
    doc = document(repo)
    assert doc["tracks"] is None and "track_actions" not in doc
    assert "could not be read" in doc["track_problem"]
    assert "Private" not in doc["track_problem"] and str(engine_home.root()) not in doc["track_problem"]
    assert doc["projects"][0]["folder"] == str(repo)


def test_no_tracks_preserves_the_phone_views(repo):
    from sakura.cockpit import pages, remote
    from sakura.cockpit.gate import Identity

    make_context(repo, state={"status": "awaiting_approval"})
    current = snapshot.collect([repo], NOW)
    legacy = replace(current, tracks=None)
    new_wire = api.snapshot_document(current)
    old_wire = api.snapshot_document(legacy)
    ids = remote.Ids(b"test")
    assert remote.document(new_wire, ids) == remote.document(old_wire, ids)
    identity = Identity("owner@example.test", "node", "Phone")
    assert pages.index(current, identity, "csrf") == pages.index(legacy, identity, "csrf")
