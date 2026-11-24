"""The engine keeper: lanes whose runs died with a restart are resumed, the person's waits and stops are respected."""

import io
import json
import threading
from dataclasses import replace
from datetime import datetime, timedelta, timezone

import pytest

from sakura import keeper, keeper_cli
from sakura.flow import cli as flow_cli
from sakura.flow import lane_questions, registry, run_records, store, surface
from tmux_support import needs_tmux, private_tmux, wait_until

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)


class Stopper:
    """A stand-in for the keeper's sleep: it lets `rounds` loops run, or stops at once when set."""

    def __init__(self, rounds=1):
        self.rounds = rounds

    def set(self):
        self.rounds = 0

    def wait_for(self, seconds):
        self.rounds -= 1
        return self.rounds <= 0


class Machine:
    """What the keeper's ports see: which runs still have a process, and the resumes it asked for."""

    def __init__(self, repo, lanes=3):
        self.repo, self.limit = repo, lanes
        self.alive_runs: set[str] = set()
        self.resumed: list[str] = []

    def ports(self) -> keeper.Ports:
        return keeper.Ports(repos=lambda: [self.repo], is_alive=lambda entry: entry["run"] in self.alive_runs,
                            resume=lambda repo, task: self.resumed.append(task.task_id), lanes=lambda: self.limit)


@pytest.fixture
def machine(repo):
    return Machine(repo)


def open_lane(repo, task_id, *, status="running", open_run=True):
    """A lane whose last run record has no end; `status` is the engine's own word for the lane."""
    task = store.TaskStore(repo, task_id)
    task.create({"task": task_id, "status": status, "workspaces": {}})
    if open_run:
        run_records.begin(task, "build-1", "builder", "build", "claude", "sonnet", "low", "local", repo, {})
        run_records.mark_running(task, "build-1", {"pid": 1})
    return task


def test_RunOnce_RestartedMachine_ResumesRunningLanesAndLeavesTheWaitingOne(repo, machine):
    for name in ("a", "b", "c"):
        open_lane(repo, name)
    waiting = open_lane(repo, "d")
    lane_questions.ask(waiting, "builder", "build-1", "Which database?", [], NOW)
    report = keeper.run_once(NOW, machine.ports())
    assert sorted(machine.resumed) == ["a", "b", "c"]
    assert sorted(report.resumed) == [f"{repo.name}/{name}" for name in "abc"]
    assert report.waiting == (f"{repo.name}/d",)


def test_RunOnce_StoppedLane_IsNotResumed(repo, machine):
    task = open_lane(repo, "a", status="stopped")
    stopped_by_file = open_lane(repo, "b")
    stopped_by_file.stop_file.write_text("")
    report = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == [] and report == keeper.KeeperReport()
    assert task.read_state()["status"] == "stopped"


def test_RunOnce_LaneWithALiveProcess_IsLeftAlone(repo, machine):
    open_lane(repo, "a")
    machine.alive_runs.add("build-1")
    assert keeper.run_once(NOW, machine.ports()) == keeper.KeeperReport() and machine.resumed == []


def test_RunOnce_LaneAtAGateOrFinished_IsNotResumed(repo, machine):
    open_lane(repo, "gate", status="awaiting_approval")
    open_lane(repo, "done", status="report_ready")
    report = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == [] and report.waiting == (f"{repo.name}/gate",)


def test_RunOnce_RunningLaneWithNoOpenRunAndNoDriver_IsResumed(repo, machine):
    open_lane(repo, "between-runs", open_run=False)
    report = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == ["between-runs"] and report.resumed == (f"{repo.name}/between-runs",)


def test_RunOnce_RunningLaneWithNoOpenRunButADriver_IsLeftAlone(repo, machine):
    task = open_lane(repo, "driven", open_run=False)
    with task.lock():
        report = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == [] and report == keeper.KeeperReport()


def test_RunOnce_StoppedOrWaitingLaneWithNoOpenRun_IsNotResumed(repo, machine):
    open_lane(repo, "stopped", status="stopped", open_run=False)
    open_lane(repo, "gate", status="awaiting_approval", open_run=False)
    asking = open_lane(repo, "asking", open_run=False)
    lane_questions.ask(asking, "builder", "build-1", "Which database?", [], NOW)
    report = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == [] and report == keeper.KeeperReport()


def test_RunOnce_MoreDeadLanesThanTheCeiling_QueuesTheRestForTheNextLoop(repo):
    machine = Machine(repo, lanes=2)
    for name in ("a", "b", "c"):
        open_lane(repo, name)
    first = keeper.run_once(NOW, machine.ports())
    assert machine.resumed == ["a", "b"] and first.queued == (f"{repo.name}/c",)
    machine.alive_runs = set()
    machine.limit = 3
    keeper.run_once(NOW + timedelta(minutes=10), machine.ports())
    assert "c" in machine.resumed


@needs_tmux
def test_RunOnce_NoTmuxServerAfterAReboot_ResumesTheLaneThroughTheDetachedLauncher(repo, tmp_path, monkeypatch):
    open_lane(repo, "a")
    registry.remember(repo)
    probe = tmp_path / "probe"
    probe.write_text(f"#!/bin/sh\ntouch {tmp_path}/resumed\n")
    probe.chmod(0o755)
    with private_tmux() as client:
        monkeypatch.setattr(flow_cli, "_new_tmux_client", lambda: client)
        shown = surface.TmuxSurface(client, "flow-", environ={"HOME": str(tmp_path), "PATH": "/usr/bin:/bin"})
        ports = replace(keeper.real_ports(), is_alive=lambda entry: False, resume=lambda repo, task: shown.start_engine(
            task, [str(probe)], {"PATH": "/usr/bin:/bin"}))
        assert client.has_server() is False

        report = keeper.run_once(NOW, ports)

        assert report.resumed == (f"{repo.name}/a",)
        assert wait_until(lambda: (tmp_path / "resumed").exists())


def test_RunOnce_ResumedTwiceInAMinute_AsksThePersonInsteadOfAThirdResume(repo, machine):
    open_lane(repo, "a")
    keeper.run_once(NOW, machine.ports())
    keeper.run_once(NOW + timedelta(seconds=20), machine.ports())
    third = keeper.run_once(NOW + timedelta(seconds=40), machine.ports())
    assert machine.resumed == ["a", "a"]
    assert third.asked == (f"{repo.name}/a",) and third.resumed == ()
    state = store.TaskStore(repo, "a").read_state()
    assert state["status"] == "blocked" and "twice within a few minutes" in state["reason"]
    assert state["runs"]["build-1"]["state"] == "orphaned"
    assert keeper.status(NOW + timedelta(seconds=41))["needs_person"] == [f"{repo.name}/a"]


def test_Status_AfterARestart_ReportsWhatTheKeeperDid(repo, machine):
    open_lane(repo, "a")
    waiting = open_lane(repo, "b")
    lane_questions.ask(waiting, "builder", "build-1", "Which database?", [], NOW)
    keeper.run_once(NOW, machine.ports())
    found = keeper.status(NOW + timedelta(seconds=30))
    assert found["last_restart"] == {"at": "2026-11-18T09:00:00Z", "resumed": [f"{repo.name}/a"],
                                     "waiting": [f"{repo.name}/b"]}


def test_Status_HeartbeatDecidesWhetherTheKeeperIsOn(repo, machine):
    assert keeper.status(NOW)["on"] is False
    keeper.run_once(NOW, machine.ports())
    assert keeper.status(NOW + timedelta(seconds=90))["on"] is True
    assert keeper.status(NOW + timedelta(minutes=10))["on"] is False


def test_StateFile_HoldsLaneNamesAndTimesOnly(repo, machine):
    open_lane(repo, "a")
    keeper.run_once(NOW, machine.ports())
    text = keeper.state_path().read_text()
    assert "pid" not in text and json.loads(text)["resumes"] == {f"{repo.name}/a": ["2026-11-18T09:00:00Z"]}


def test_Serve_RunsOnceAndStartsThePhonePage(repo, machine):
    open_lane(repo, "a")
    started, pages = threading.Event(), []
    keeper.serve(machine.ports(), now=lambda: NOW, phone_page=lambda stop: (pages.append("up"), started.set()),
                 wait=Stopper().wait_for)
    assert started.wait(5) and machine.resumed == ["a"] and pages == ["up"]


def test_Serve_RunOnceFails_ReportedAndTheLoopContinues(repo, machine, capsys):
    calls = []

    def broken_repos():
        calls.append(1)
        if len(calls) == 1:
            raise OSError("disk went away")
        return []

    ports = keeper.Ports(repos=broken_repos, is_alive=lambda entry: False, resume=lambda *_: None, lanes=lambda: 3)
    stop = Stopper(rounds=2)
    keeper.serve(ports, now=lambda: NOW, phone_page=None, wait=stop.wait_for)
    assert len(calls) == 2
    assert "disk went away" in capsys.readouterr().err


def test_Cli_Status_PrintsTheDocument(repo, machine):
    keeper.run_once(NOW, machine.ports())
    out = io.StringIO()
    assert keeper_cli.main(["status", "--json"], stdout=out, now=lambda: NOW) == 0
    assert json.loads(out.getvalue())["on"] is True


def test_Cli_PhonePage_SettingIsKept(repo):
    out = io.StringIO()
    assert keeper_cli.main(["phone-page", "off"], stdout=out) == 0
    assert keeper.status(NOW)["phone_page"] is False
    assert keeper_cli.main(["phone-page", "on"], stdout=out) == 0
    assert keeper.status(NOW)["phone_page"] is True


def test_Serve_PhonePageTurnedOff_StopsTheRunningServerAndWaitsForIt(repo, machine):
    started, ended, rounds = threading.Event(), threading.Event(), []

    def page(stop=threading.Event()):
        started.set()
        stop.wait()
        ended.set()

    def turn_it_off_after_the_first_look(seconds):
        rounds.append(1)
        if len(rounds) == 1:
            assert started.wait(5)
            assert keeper_cli.main(["phone-page", "off"], stdout=io.StringIO()) == 0
        return len(rounds) >= 2

    keeper.serve(machine.ports(), now=lambda: NOW, phone_page=page, wait=turn_it_off_after_the_first_look)

    assert ended.is_set()
