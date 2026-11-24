import sqlite3
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from datetime import UTC, datetime

import pytest
from sakura import actors, engine_home
from sakura.flow import registry, store
from sakura.tracks import TrackService
from sakura.tracks.database import Database
from sakura.tracks.domain import (
    ANSWER_LIMIT,
    DECISION_LIMIT,
    GOAL_LIMIT,
    NAME_LIMIT,
    SUMMARY_LIMIT,
    ConversationKey,
    LaneState,
    Refusal,
    RefusalCode,
)
from sakura.tracks.service import read_lane, resolve_project

PERSON = actors.Actor("app", "owner")
PHONE = actors.Actor("overlay", "owner", "phone")
MASTER = actors.Actor("master", "session-1")
OTHER_MASTER = actors.Actor("master", "session-2")
ROLE = actors.Actor("builder", "role")
ENGINE = actors.Actor("engine", "flow")
NOW = datetime(2026, 11, 23, tzinfo=UTC)


@pytest.fixture
def db(tmp_path):
    return Database(tmp_path / "tracks" / "tracks.db")


@pytest.fixture
def tracks(db):
    return TrackService(db.connect, clock=lambda: NOW)


def create(tracks, name="First", op_id="new", actor=PERSON, **fields):
    return tracks.create(name, "Ship", actor=actor, op_id=op_id, **fields)


def new_lane(repo, task="lane", status="running", opener=MASTER):
    lane = store.TaskStore(repo, task)
    lane.create({"task": task, "status": status, "title": "Live title"})
    lane.append_ledger("operator", f"start requested actor={actors.format(opener)}")
    return lane


def test_person_replay_preserves_first_result_place_and_inputs(tracks):
    track = create(tracks)
    decision = tracks.add_decision(track.id, "Proceed?", actor=MASTER, op_id="question")
    first = tracks.settle_decision(track.id, decision.number, "Yes", actor=PHONE, op_id="answer")
    assert tracks.settle_decision(track.id, decision.number, "Yes", actor=PERSON, op_id="answer") == first
    with pytest.raises(Refusal) as changed:
        tracks.settle_decision(track.id, decision.number, "No", actor=PERSON, op_id="answer")
    assert changed.value.code == RefusalCode.OPERATION_CONFLICT
    with pytest.raises(Refusal):
        tracks.settle_decision(track.id, decision.number, "Yes", actor=PERSON, op_id="other-answer")
    audit = tracks.read_audit(track.id)
    assert len(audit) == 3
    assert (audit[-1].actor, audit[-1].place, audit[-1].at) == ("overlay:owner:phone", "phone", NOW.isoformat())
    assert audit[-1].after["answer"] == "Yes"
    assert tracks.read_all()[0].decisions == (first,)
    assert create(tracks, actor=PHONE) == track


def test_failed_audit_rolls_back_record_and_replay(tracks, db):
    track = create(tracks)
    with closing(db.connect(True)) as connection, connection:
        connection.execute("""CREATE TRIGGER reject_audit BEFORE INSERT ON track_audit
                              BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END""")
    with pytest.raises(sqlite3.IntegrityError, match="audit unavailable"):
        tracks.update(track.id, summary="New", actor=PERSON, op_id="edit")
    assert tracks.read_all()[0].summary == ""
    assert len(tracks.read_audit(track.id)) == 1
    with closing(db.connect(True)) as connection, connection:
        connection.execute("DROP TRIGGER reject_audit")
    assert tracks.update(track.id, summary="New", actor=PERSON, op_id="edit").summary == "New"


def test_concurrent_edits_and_numbering(tracks):
    track = create(tracks)
    with ThreadPoolExecutor(4) as callers:
        results = [
            callers.submit(tracks.update, track.id, goal="Changed", actor=PERSON, op_id="goal"),
            callers.submit(tracks.update, track.id, summary="Updated", actor=MASTER, op_id="summary"),
            *(callers.submit(tracks.add_decision, track.id, str(i), actor=PERSON, op_id=f"ask-{i}") for i in range(2)),
        ]
        for result in results:
            result.result()
    [current] = tracks.read_all()
    assert (current.goal, current.summary) == ("Changed", "Updated")
    assert [decision.number for decision in current.decisions] == [1, 2]
    edits = [entry for entry in tracks.read_audit(track.id) if entry.operation == "update"]
    assert {frozenset(entry.after) for entry in edits} == {frozenset({"goal"}), frozenset({"summary"})}


def test_closed_name_reuse_and_conflicting_reopen(tracks):
    old = create(tracks, "  Straße  ")
    decision = tracks.add_decision(old.id, "Proceed?", actor=PERSON, op_id="ask")
    with pytest.raises(Refusal):
        create(tracks, "STRASSE", op_id="duplicate")
    assert tracks.update(old.id, status="closed", actor=PERSON, op_id="close").status == "closed"
    assert tracks.settle_decision(old.id, decision.number, "Yes", actor=PERSON, op_id="answer").state == "settled"
    create(tracks, "STRASSE", op_id="replacement")
    with pytest.raises(Refusal) as reopen:
        tracks.update(old.id, status="open", actor=PERSON, op_id="reopen")
    assert reopen.value.code == RefusalCode.NAME_IN_USE


def test_master_permissions_and_replay_use_bound_session(tracks, repo):
    track = create(tracks, actor=MASTER)
    assert create(tracks, actor=MASTER) == track
    for actor in (PERSON, OTHER_MASTER, ROLE, ENGINE):
        with pytest.raises(Refusal):
            create(tracks, actor=actor)
    tracks.update(track.id, goal="Plan", status="paused", actor=MASTER, op_id="pause")
    for changes in ({"name": "New"}, {"status": "closed"}):
        with pytest.raises(Refusal):
            tracks.update(track.id, actor=MASTER, op_id="edit", **changes)
    decision = tracks.add_decision(track.id, "Ready?", actor=MASTER, op_id="ask")
    tracks.settle_decision(track.id, decision.number, "Yes", actor=PERSON, op_id="answer")
    with pytest.raises(Refusal):
        tracks.settle_decision(track.id, decision.number, "Yes", actor=MASTER, op_id="answer")
    new_lane(repo)
    new_lane(repo, "other", opener=OTHER_MASTER)
    with pytest.raises(Refusal):
        tracks.link_lane(track.id, repo, "other", actor=MASTER, op_id="foreign")
    tracks.link_lane(track.id, repo, "lane", actor=MASTER, op_id="own")
    with pytest.raises(Refusal):
        tracks.unlink_lane(track.id, repo, "lane", actor=MASTER, op_id="unlink")
    tracks.unlink_lane(track.id, repo, "lane", actor=PERSON, op_id="unlink")
    tracks.update(track.id, status="closed", actor=PERSON, op_id="close")
    with pytest.raises(Refusal):
        tracks.update(track.id, status="open", actor=MASTER, op_id="reopen")
    with pytest.raises(Refusal):
        tracks.link_conversation(track.id, ConversationKey("provider", "account", "session"), "Chat",
                                 actor=MASTER, op_id="chat")
    with pytest.raises(Refusal):
        tracks.link_lane(track.id, repo, "lane", actor=PERSON, op_id="closed-link")


def test_moves_audit_both_sides_and_keep_project(tracks, repo):
    first, second = create(tracks), create(tracks, "Second", "new-2")
    new_lane(repo)
    tracks.link_lane(first.id, repo, "lane", actor=PERSON, op_id="link")
    link = tracks.link_lane(second.id, repo, "lane", actor=PERSON, op_id="move")
    assert link.title == "Live title"
    key = ConversationKey("provider", "account", "session")
    tracks.link_conversation(first.id, key, "Chat", actor=PERSON, op_id="chat")
    tracks.link_conversation(second.id, key, "Moved chat", actor=PERSON, op_id="move-chat")
    for other in (ConversationKey("other", "account", "session"), ConversationKey("provider", "other", "session")):
        tracks.link_conversation(first.id, other, "Other chat", actor=PERSON, op_id=str(other))
    current = {track.id: track for track in tracks.read_all()}
    assert current[first.id].projects == current[second.id].projects
    assert current[first.id].lanes == () and current[second.id].lanes == (link,)
    assert len(current[first.id].conversations) == 2 and len(current[second.id].conversations) == 1
    for op_id in ("move", "move-chat"):
        [left] = [entry for entry in tracks.read_audit(first.id) if entry.op_id == op_id]
        [right] = [entry for entry in tracks.read_audit(second.id) if entry.op_id == op_id]
        assert left.before and right.after
    tracks.unlink_conversation(second.id, key, actor=PERSON, op_id="unlink-chat")
    assert not next(track for track in tracks.read_all() if track.id == second.id).conversations


def test_missing_project_preserves_lifecycle_and_only_verified_gone_allows_close(tracks, repo):
    import shutil

    track = create(tracks)
    registry.remember(repo)
    project = resolve_project(repo)
    live = new_lane(repo)
    finished = new_lane(repo, "finished", "report_ready")
    gone = new_lane(repo, "gone")
    unreadable = new_lane(repo, "unreadable")
    for lane in (live, finished, gone, unreadable):
        tracks.link_lane(track.id, repo, lane.task_id, actor=PERSON, op_id=lane.task_id)
    shutil.rmtree(gone.root)
    unreadable.state_file.write_text("broken")
    moved = repo.rename(repo.with_name("moved"))
    assert project.id == engine_home.project_id_for_path(repo)
    assert read_lane(project, "lane").state == LaneState.LIVE
    assert read_lane(project, "finished").state == LaneState.FINISHED
    assert read_lane(project, "gone").state == LaneState.GONE
    assert read_lane(project, "unreadable").state == LaneState.UNREADABLE
    [current] = tracks.read_all()
    assert all(link.title == "Live title" for link in current.lanes)
    with pytest.raises(Refusal) as blocked:
        tracks.update(track.id, status="closed", actor=PERSON, op_id="close")
    assert blocked.value.labels == ("Live title", "Live title")
    moved.rename(repo)
    live.update_state(lambda state: state.update(status="report_ready"))
    shutil.rmtree(unreadable.root)
    assert tracks.update(track.id, status="closed", actor=PERSON, op_id="close").status == "closed"


def test_linked_worktree_and_main_repo_share_project_identity(tracks, repo):
    from conftest import git

    alias = repo.with_name("alias")
    git(repo, "worktree", "add", "-b", "alias", str(alias))
    track = create(tracks, project=alias)
    new_lane(repo)
    tracks.link_lane(track.id, alias, "lane", actor=PERSON, op_id="link")
    [current] = tracks.read_all()
    [project] = current.projects
    assert (project.id, project.folder) == (engine_home.project_id(repo), str(repo))
    assert current.lanes[0].project_id == project.id


def test_close_uses_saved_title_when_live_title_is_unavailable(tracks, repo):
    track = create(tracks)
    lane = new_lane(repo)
    tracks.link_lane(track.id, repo, lane.task_id, actor=PERSON, op_id="link")
    lane.update_state(lambda state: state.pop("title"))
    with pytest.raises(Refusal) as refused:
        tracks.update(track.id, status="closed", actor=PERSON, op_id="close")
    assert refused.value.labels == ("Live title",)


def test_exact_replay_survives_a_project_path_that_can_no_longer_be_verified(tracks, repo):
    track = create(tracks, project=repo)
    new_lane(repo)
    first = tracks.link_lane(track.id, repo, "lane", actor=PERSON, op_id="link")
    repo.rename(repo.with_name("moved"))
    repo.write_text("the registered path currently holds a file")
    assert create(tracks, actor=PHONE, project=repo) == track
    assert tracks.link_lane(track.id, repo, "lane", actor=PHONE, op_id="link") == first
    assert tracks.unlink_lane(track.id, repo, "lane", actor=PERSON, op_id="unlink") == first


@pytest.mark.parametrize("field,limit", [("name", NAME_LIMIT), ("goal", GOAL_LIMIT), ("summary", SUMMARY_LIMIT),
                                          ("text", DECISION_LIMIT), ("answer", ANSWER_LIMIT)])
def test_validation_uses_domain_limits(tracks, field, limit):
    track = create(tracks)
    decision = tracks.add_decision(track.id, "Question", actor=PERSON, op_id="question")
    calls = {
        "name": lambda value: tracks.update(track.id, name=value, actor=PERSON, op_id="edit"),
        "goal": lambda value: tracks.update(track.id, goal=value, actor=PERSON, op_id="edit"),
        "summary": lambda value: tracks.update(track.id, summary=value, actor=PERSON, op_id="edit"),
        "text": lambda value: tracks.add_decision(track.id, value, actor=PERSON, op_id="edit"),
        "answer": lambda value: tracks.settle_decision(track.id, decision.number, value, actor=PERSON, op_id="edit"),
    }
    with pytest.raises(Refusal):
        calls[field]("x" * (limit + 1))
    if field != "summary":
        with pytest.raises(Refusal):
            calls[field](" \t\n")
    calls[field]("x" * limit)
