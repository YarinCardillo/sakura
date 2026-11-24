"""Give priority to the person's own sessions: lanes wait while the person works with an agent elsewhere."""

import io
import json
import os
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace

import pytest

from sakura.flow import store, yield_person

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)


def touch(path, minutes_ago):
    moment = (NOW - timedelta(minutes=minutes_ago)).timestamp()
    os.utime(path, (moment, moment))


def claude_transcript(home, cwd, minutes_ago, name="s1"):
    folder = home / "projects" / "-encoded-folder"
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.jsonl"
    path.write_text(json.dumps({"type": "user", "cwd": str(cwd), "message": "hello"}) + "\n")
    touch(path, minutes_ago)
    return path


def codex_session(home, cwd, minutes_ago, name="rollout-1"):
    folder = home / "sessions" / "2026" / "11" / "18"
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.jsonl"
    path.write_text(json.dumps({"type": "session_meta", "payload": {"cwd": str(cwd)}}) + "\n")
    touch(path, minutes_ago)
    return path


@pytest.fixture
def places(tmp_path):
    lane = tmp_path / "workspaces" / "lane-1"
    elsewhere = tmp_path / "projects" / "mine"
    for folder in (lane, elsewhere, tmp_path / "claude", tmp_path / "codex"):
        folder.mkdir(parents=True)
    return SimpleNamespace(lane=lane, elsewhere=elsewhere, claude=tmp_path / "claude", codex=tmp_path / "codex")


def active(places, **changes):
    arguments = {"homes": [places.claude, places.codex], "lane_workspaces": {places.lane}, **changes}
    return yield_person.person_active(NOW, **arguments)


def test_PersonActive_ClaudeTranscriptTouchedTwoMinutesAgoOutsideTheLanes_IsTrue(places):
    claude_transcript(places.claude, places.elsewhere, 2)
    assert active(places) is True


def test_PersonActive_TheSameTouchInsideALaneWorkspace_IsFalse(places):
    claude_transcript(places.claude, places.lane, 2, "own")
    claude_transcript(places.claude, places.lane / "src" / "deep", 1, "deeper")
    assert active(places) is False


def test_PersonActive_TouchedLongAgo_IsFalse(places):
    claude_transcript(places.claude, places.elsewhere, 30)
    assert active(places) is False
    assert active(places, window=timedelta(hours=1)) is True


def test_PersonActive_CodexSessionOutsideTheLanes_IsTrueAndInsideIsFalse(places):
    codex_session(places.codex, places.lane, 1, "own")
    assert active(places) is False
    codex_session(places.codex, places.elsewhere, 1, "mine")
    assert active(places) is True


def test_PersonActive_AFileWhoseFolderCannotBeRead_IsNotCounted(places):
    folder = places.claude / "projects" / "x"
    folder.mkdir(parents=True)
    (folder / "odd.jsonl").write_text("not json at all\n")
    touch(folder / "odd.jsonl", 1)
    assert active(places) is False


def test_PersonActive_NoFolders_IsFalse(tmp_path, places):
    assert yield_person.person_active(NOW, homes=[tmp_path / "missing"], lane_workspaces=set()) is False


def test_PersonActive_TheLanesOwnWorktreeWhereThePersonUsesClaudeToo_NeverYieldsToItself(places):
    # Review focus 3: a session in the lane's worktree, the person's or the lane's agent, is the lane's, so no deadlock.
    claude_transcript(places.claude, places.lane, 0, "person-in-lane-worktree")
    assert active(places) is False


class World:
    def __init__(self, task, *, enabled=True, busy=()):
        self.task, self.enabled, self.busy = task, enabled, list(busy)
        self.sleeps, self.marks = [], []
        self.ports = yield_person.Ports(is_enabled=lambda: self.enabled, person_active=self._active, sleep=self._sleep,
                                        now=lambda: NOW)

    def _active(self, now):
        return self.busy.pop(0) if self.busy else False

    def _sleep(self, seconds):
        self.sleeps.append(seconds)
        self.marks.append(self.task.read_state().get("yield_wait"))


@pytest.fixture
def task(repo):
    found = store.TaskStore(repo, "a")
    found.create({"task": "a", "status": "running", "workspaces": {}})
    return found


def test_Hold_KnobOff_NothingWaitsAndNothingIsRead(task):
    world = World(task, enabled=False, busy=[True])
    assert yield_person.hold(task, world.ports) is False
    assert world.sleeps == [] and world.busy == [True]


def test_Hold_KnobOnAndThePersonActive_WaitsUntilOneCheckAfterTheyStop(task):
    world = World(task, busy=[True, True, False])
    assert yield_person.hold(task, world.ports) is True
    assert world.sleeps == [60, 60]
    assert all(mark and mark["waiting"] == "person" for mark in world.marks)
    assert "yield_wait" not in task.read_state()


def test_Hold_KnobOnAndNobodyActive_StartsAtOnce(task):
    world = World(task, busy=[False])
    assert yield_person.hold(task, world.ports) is False and world.sleeps == []


def test_Hold_AStopWhileWaiting_EndsTheWaitSoTheOperatorIsNeverIgnored(task):
    world = World(task, busy=[True] * 50)
    world.ports = yield_person.Ports(is_enabled=lambda: True, person_active=world._active,
                                     sleep=lambda seconds: task.stop_file.write_text(""), now=lambda: NOW)
    yield_person.hold(task, world.ports)
    assert task.stop_file.exists() and "yield_wait" not in task.read_state()


def test_Knob_DefaultsToOffAndIsKeptInTheEngineHome():
    assert yield_person.is_enabled() is False
    yield_person.set_enabled(True)
    assert yield_person.is_enabled() is True and yield_person.settings_path().stat().st_mode & 0o777 == 0o600
    yield_person.set_enabled(False)
    assert yield_person.is_enabled() is False


def test_Cli_OnAndOff_ChangeTheKnobAndTakeTheCockpitsActor():
    out = io.StringIO()
    assert yield_person.main(["on", "--actor=cli:owner", "--"], stdout=out) == 0 and yield_person.is_enabled()
    assert yield_person.main(["off"], stdout=out) == 0 and not yield_person.is_enabled()


def test_LaneWorkspaces_AreEveryRecordedWorkspaceOfEveryLane(repo, tmp_path):
    folder = tmp_path / "ws0"
    folder.mkdir()
    found = store.TaskStore(repo, "a")
    found.create({"task": "a", "status": "running", "workspaces": {"0": {"path": str(folder)}}})
    assert yield_person.lane_workspaces([repo]) == {folder.resolve()}


def test_Engine_TheStepStartPoint_HoldsOnlyBeforePhasesThatSpend(task, monkeypatch):
    from sakura.flow import engine

    calls = []
    monkeypatch.setattr(yield_person, "hold", lambda lane, ports=None: calls.append(lane.task_id) or True)
    monkeypatch.setattr(yield_person, "real_ports", lambda: None)
    context = SimpleNamespace(store=task)
    assert engine._yield_to_person(context, "build") is None and calls == ["a"]
    assert engine._yield_to_person(context, "pr") is None and calls == ["a"]
    task.stop_file.write_text("")
    assert engine._yield_to_person(context, "build").kind == "stopped"
