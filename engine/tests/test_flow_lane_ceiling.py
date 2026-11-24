"""The lane ceiling: how many lanes the master may have open at once."""

import fcntl
import json
import subprocess
import threading

import pytest

from flow_support import make_context
from lock_support import observe_contention
from sakura import actors, engine_home, install
from sakura.flow import cli, exits, lane_ceiling, store

MASTER = actors.Actor(actors.MASTER_SOURCE, "session1")
PERSON = actors.Actor(actors.CLI_SOURCE, "person")


def _lane(repo, task, status="running"):
    make_context(repo, task=task, state={"status": status})


def test_Ceiling_ThirdLaneAtTwo_RefusedNamingCeilingAndOpenLanes(repo):
    _lane(repo, "first")
    _lane(repo, "second")
    with pytest.raises(lane_ceiling.LaneCeilingRefused) as refused:
        with lane_ceiling.admit(MASTER, 2, [repo]):
            pytest.fail("a third lane was admitted")
    message = str(refused.value)
    assert "ceiling is 2" in message and f"{repo.name}/first" in message and f"{repo.name}/second" in message


def test_Ceiling_BelowTheCeiling_Admitted(repo):
    _lane(repo, "first")
    with lane_ceiling.admit(MASTER, 2, [repo]):
        pass


def test_Ceiling_PersonStart_NotCapped(repo):
    _lane(repo, "first")
    _lane(repo, "second")
    with lane_ceiling.admit(PERSON, 1, [repo]):
        pass


@pytest.mark.parametrize("status", ["stopped", "blocked", "failed", "awaiting_approval", "running"])
def test_Ceiling_UnfinishedLane_Counts(repo, status):
    _lane(repo, "first", status)
    assert lane_ceiling.open_lanes([repo]) == [f"{repo.name}/first"]


@pytest.mark.parametrize("status", ["report_ready", "cleaned"])
def test_Ceiling_FinishedLane_DoesNotCount(repo, status):
    _lane(repo, "first", status)
    assert lane_ceiling.open_lanes([repo]) == []


def test_Ceiling_UnreadableLane_CountsAsOpen(repo):
    _lane(repo, "first")
    store.TaskStore(repo, "first").state_file.write_text("not json")
    assert lane_ceiling.open_lanes([repo]) == [f"{repo.name}/first"]


def test_Ceiling_TwoLanesOnePerProject_BothCounted(repo, tmp_path):
    other = tmp_path / "other"
    subprocess.run(["git", "init", "-q", "-b", "main", str(other)], check=True)
    _lane(repo, "first")
    make_context(other, task="second")
    assert len(lane_ceiling.open_lanes([repo, other])) == 2


def test_Ceiling_TwoOpensAtOneBelow_ExactlyOneAdmitted(repo, monkeypatch):
    blocked = observe_contention(monkeypatch, lane_ceiling, {"second-opener"})
    answers = []

    def contend():
        try:
            with lane_ceiling.admit(MASTER, 1, [repo]):
                _lane(repo, "two")
                answers.append("admitted")
        except lane_ceiling.LaneCeilingRefused:
            answers.append("refused")

    second = threading.Thread(name="second-opener", target=contend)
    with lane_ceiling.admit(MASTER, 1, [repo]):
        second.start()
        assert blocked.acquire(timeout=10)
        _lane(repo, "one")
        answers.append("admitted")
    second.join(10)
    assert not second.is_alive()
    assert sorted(answers) == ["admitted", "refused"]
    assert len(lane_ceiling.open_lanes([repo])) == 1


# -- `flow start` as the master ---------------------------------------------------------------------------------


@pytest.fixture
def project(repo, monkeypatch):
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: exits.OK)
    return repo


def test_Start_MasterOverTheCeiling_RefusedWithNoTaskCreated(project, capsys, monkeypatch):
    monkeypatch.setenv(actors.TRUSTED_ENV, "1")  # started by the master's server, the only process that may carry its identity
    _lane(project, "first")
    code = cli.main(["start", "-m", "second thing", "--preset", "solo", "--relax", "preset", "--actor=master:session1"])
    assert code == exits.USAGE and "ceiling is 1" in capsys.readouterr().out
    assert sorted(path.name for path in store.flow_root(project).iterdir()) == ["first"]


def test_Start_MasterBelowTheCeiling_CreatesTheLaneAndReleasesTheLock(project, monkeypatch):
    monkeypatch.setenv(actors.TRUSTED_ENV, "1")
    assert cli.main(["start", "-m", "a thing", "--actor=master:session1"]) == exits.OK
    assert len(lane_ceiling.open_lanes([project])) == 1
    with open(engine_home.root() / lane_ceiling.LOCK_NAME, "a") as held:
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)


def test_Start_PersonOverTheCeiling_Starts(project):
    _lane(project, "first")
    assert cli.main(["start", "-m", "second thing", "--preset", "solo", "--relax", "preset"]) == exits.OK
    assert len(lane_ceiling.open_lanes([project])) == 2


def test_Start_MasterIdentityFromATerminal_Refused(project, capsys, monkeypatch):
    monkeypatch.delenv(actors.TRUSTED_ENV, raising=False)
    assert cli.main(["start", "-m", "a thing", "--actor=master:session1"]) == exits.USAGE
    assert "cannot claim" in capsys.readouterr().out and lane_ceiling.open_lanes([project]) == []


# -- one word for a lane that cannot continue (proof wording defect)


@pytest.mark.parametrize("status", ["blocked", "failed"])
def test_Word_LaneThatCannotContinue_IsBlockedInFlowStatusTheSnapshotAndTheMastersView(project, capsys, status):
    from sakura.cockpit import items
    from sakura.master import tools

    _lane(project, "first", status)
    assert lane_ceiling.state_word(status) == "blocked"
    assert cli.main(["status", "first"]) == exits.OK
    assert "status: blocked" in capsys.readouterr().out
    assert cli.main(["status"]) == exits.OK
    assert "status: blocked" in capsys.readouterr().out
    item = items.flow_item(store.TaskStore(project, "first"), lambda key: None)
    assert item.agent_state == "blocked" and tools._lane_row(item, [])["state"] == "blocked"


def test_Word_OtherStatuses_AreUnchanged():
    for status in ("running", "stopped", "awaiting_approval", "report_ready", "cleaned"):
        assert lane_ceiling.state_word(status) == status


def test_Word_FlowStatusJson_KeepsTheEnginesOwnStatusBesideTheWord(project, capsys):

    _lane(project, "first", "failed")
    assert cli.main(["status", "first", "--json"]) == exits.OK
    shown = json.loads(capsys.readouterr().out)
    assert shown["status"] == "blocked" and shown["engine_status"] == "failed"
