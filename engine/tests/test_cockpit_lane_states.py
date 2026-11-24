"""What the cockpit offers for a lane in each state is what the engine accepts (proof defect D7)."""

import hashlib
import json

import pytest

from flow_support import make_context
from sakura import actors
from sakura.cockpit import actions, items
from sakura.flow import cli, exits, lane_ceiling, resume, store, tasks
from sakura.flow.phases import cleanup

PERSON = actors.Actor("cli", "owner")
STATE_KEYS = ("R", "s", "C", "T")
IDLE_PLAN = {"phase": "plan"}


def _lane(repo, status, phase="build", contract=False, **more):
    """A lane in `status`; `contract` saves one (a recontract needs it)."""
    state = {"status": status, "phase": phase, "reason": f"{status} for a test", **more}
    ctx = make_context(repo, state=state)
    if contract:
        digest = "c" * 64
        ctx.store.update_state(lambda current: current.update(contract_sha256=digest))
        ctx.store.contract.write_text(json.dumps({"rows": []}))
    return ctx


def _offered(repo, task="demo"):
    item = items.flow_item(store.TaskStore(repo, task), lambda key: None)
    return item, [action.key for action in actions.applicable(item) if action.key in STATE_KEYS]


@pytest.mark.parametrize(("status", "phase", "expected"), [
    ("running", "build", ["R", "s"]),  # no engine drives it: the person may resume it
    ("stopped", "build", ["R", "s", "C"]),
    ("blocked", "build", ["R", "s"]),
    ("failed", "build", ["s", "C"]),
    ("awaiting_approval", "plan", ["s"]),
    ("report_ready", "report", []),
    ("cleaned", "cleanup", []),
])
def test_Offers_EachState_ListsExactlyWhatTheEngineAccepts(repo, status, phase, expected):
    _lane(repo, status, phase)
    assert _offered(repo)[1] == expected


def test_Offers_RunningLaneWithALiveRun_OffersOnlyStop(repo):
    _lane(repo, "running", "build", runs={"build-1": {"state": "running"}})
    assert _offered(repo)[1] == ["s"]


@pytest.mark.parametrize("status", ["blocked", "failed"])
def test_Offers_BlockedOrFailedAtPlanWithASavedContract_AlsoOffersRecontract(repo, status):
    _lane(repo, status, "plan", contract=True)
    assert "T" in _offered(repo)[1]


def test_Items_CarryTheStatusPhaseAndWhetherAContractIsSaved(repo):
    _lane(repo, "failed", "plan", contract=True)
    item = _offered(repo)[0]
    assert (item.status, item.phase, item.has_contract) == ("failed", "plan", True)


# -- Resume: a blocked lane resumes through the cockpit exactly as `flow resume` does ---------------------------------


def test_Resume_BlockedLane_IsPlannedWithItsRevision(repo):
    _lane(repo, "blocked", "build")
    item = _offered(repo)[0]
    invocation = actions.plan("R", item, {}, PERSON)
    assert f"--stopped-revision={item.resume_revision}" in invocation.argv and "--detach" in invocation.argv


def test_Resume_FailedLane_IsRefusedNamingTheWayOut(repo):
    _lane(repo, "failed", "build")
    with pytest.raises(actions.ActionRefused, match="failed.*clean"):
        actions.plan("R", _offered(repo)[0], {}, PERSON)


def test_Resume_EngineAdmitsABlockedLaneAtItsRevisionAndRefusesAStaleOneOrAFailedOne(repo):
    ctx = _lane(repo, "blocked", "build")
    state = ctx.store.read_state()
    resume.require_stopped(state, resume.revision(state))
    with pytest.raises(tasks.TaskStartError, match="has changed"):
        resume.require_stopped(state, "0" * 64)
    ctx.store.update_state(lambda current: current.update(status="failed"))
    failed = ctx.store.read_state()
    with pytest.raises(tasks.TaskStartError, match="has changed"):
        resume.require_stopped(failed, resume.revision(failed))


def test_Resume_BlockedLaneWithALiveRun_IsRefusedByTheEngine(repo):
    ctx = _lane(repo, "blocked", "build", runs={"build-1": {"state": "running"}})
    state = ctx.store.read_state()
    with pytest.raises(tasks.TaskStartError):
        resume.require_stopped(state, resume.revision(state))


# -- Clean: a failed or stopped lane is discarded ---------------------------------------------------------------------------


def test_Clean_ActionRunsFlowCleanupDiscardAfterAConfirmation(repo):
    _lane(repo, "failed")
    item = _offered(repo)[0]
    assert actions.ACTIONS["C"].confirm is True
    argv = actions.plan("C", item, {}, PERSON).argv
    assert argv[1:4] == ["flow", "cleanup", "--discard"] and argv[-1] == "demo"


@pytest.mark.parametrize("status", ["failed", "stopped"])
def test_Clean_DiscardRemovesTheLaneAndMarksItCleaned(repo, monkeypatch, status):
    _lane(repo, status)
    monkeypatch.chdir(repo)
    called = []
    monkeypatch.setattr(cleanup, "_remove_task", lambda ctx, state: called.append(ctx.store.task_id))
    assert cli.main(["cleanup", "--discard", "--actor=cli:owner", "demo"]) == exits.OK
    state = store.TaskStore(repo, "demo").read_state()
    assert called == ["demo"] and state["status"] == "cleaned"
    assert "discarded" in store.TaskStore(repo, "demo").ledger.read_text()


@pytest.mark.parametrize("status", ["running", "blocked", "awaiting_approval", "report_ready"])
def test_Clean_DiscardOfAnyOtherState_IsRefusedAndNothingIsRemoved(repo, monkeypatch, capsys, status):
    _lane(repo, status)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cleanup, "_remove_task", lambda ctx, state: pytest.fail("removed"))
    assert cli.main(["cleanup", "--discard", "--actor=cli:owner", "demo"]) == exits.USAGE
    assert "discard" in capsys.readouterr().out


def test_Clean_DiscardWithALiveRun_IsRefused(repo, monkeypatch, capsys):
    _lane(repo, "failed", active_runs=[{"run": "build-1", "pid": 1}])
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cleanup, "_remove_task", lambda ctx, state: pytest.fail("removed"))
    assert cli.main(["cleanup", "--discard", "--actor=cli:owner", "demo"]) == exits.USAGE


def test_Clean_PlainCleanupStillNeedsTheCleanupPhase(repo, monkeypatch, capsys):
    _lane(repo, "failed", "build")
    monkeypatch.chdir(repo)
    assert cli.main(["cleanup", "--actor=cli:owner", "demo"]) == exits.USAGE
    assert "not ready for cleanup" in capsys.readouterr().out


# -- Recontract ---------------------------------------------------------------------------------------------------------------


def test_Recontract_FailedLaneAtPlan_PassesTheEnginesStatusCheck(repo):
    ctx = _lane(repo, "failed", "plan", contract=True)
    ctx.store.spec.write_text("the approved spec\n")
    digest = hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    ctx.store.update_state(lambda current: current["approvals"].update(spec={"sha256": digest}))
    try:
        cli._validated_recontract_inputs(ctx.store, "demo")
    except tasks.TaskStartError as error:  # past the status check; the contract file of this fixture is not a real one
        assert "outside a blocked or failed plan" not in str(error)


@pytest.mark.parametrize("status", ["running", "stopped", "report_ready"])
def test_Recontract_OtherStates_AreRefusedByTheEngineAndNotOffered(repo, status):
    ctx = _lane(repo, status, "plan", contract=True)
    with pytest.raises(tasks.TaskStartError, match="outside a blocked or failed plan"):
        cli._validated_recontract_inputs(ctx.store, "demo")
    assert "T" not in _offered(repo)[1]


# -- Stop: any lane that is not finished ----------------------------------------------------------------------------------------


@pytest.mark.parametrize("status", ["blocked", "failed", "awaiting_approval", "stopped"])
def test_Stop_AnIdleLaneInAnyOpenState_BecomesStopped(repo, monkeypatch, status):
    _lane(repo, status, "build")
    monkeypatch.chdir(repo)
    assert cli.main(["stop", "--actor=cli:owner", "demo"]) == exits.OK
    state = store.TaskStore(repo, "demo").read_state()
    assert state["status"] == "stopped"
    if status != "stopped":
        assert f"it was {lane_ceiling.state_word(status)}" in state["reason"]


def test_Stop_ALaneAnEngineIsDriving_KeepsItsStatusAndGetsTheStopFile(repo, monkeypatch):
    ctx = _lane(repo, "running", "build")
    monkeypatch.chdir(repo)
    with ctx.store.lock():
        assert cli.main(["stop", "--actor=cli:owner", "demo"]) == exits.OK
    assert store.TaskStore(repo, "demo").read_state()["status"] == "running" and ctx.store.stop_file.exists()


# -- Approve only while a gate waits for that approval (proof defect D7b) ---------------------------------------------------


def _approve_item(repo):
    return items.flow_item(store.TaskStore(repo, "demo"), lambda key: None)


@pytest.mark.parametrize("status", ["blocked", "failed", "stopped", "cleaned", "report_ready", "running"])
def test_Approve_IsNotOfferedOutsideAGate(repo, status):
    _lane(repo, status, "build")
    assert "a" not in [action.key for action in actions.applicable(_approve_item(repo))]


def test_Approve_IsOfferedWhileTheGateWaits(repo):
    _lane(repo, "awaiting_approval", "plan", reason="awaiting approval: sakura flow approve demo spec (read x)")
    item = _approve_item(repo)
    assert item.agent_state == "gate" and "a" in [action.key for action in actions.applicable(item)]


@pytest.mark.parametrize("status", ["blocked", "failed", "stopped", "cleaned", "report_ready", "running"])
def test_Approve_EngineRefusesEveryOtherStateAndChangesNothing(repo, monkeypatch, capsys, status):
    ctx = _lane(repo, status, "build")
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    before = ctx.store.read_state()
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "--actor=cli:owner", "demo", "spec"]) == exits.USAGE
    assert "no longer waiting" in capsys.readouterr().out
    fresh = store.TaskStore(repo, "demo")
    assert fresh.read_state() == before and "spec approved" not in fresh.ledger.read_text()


def test_Approve_ADifferentGateThanTheOneWaiting_IsRefused(repo, monkeypatch, capsys):
    ctx = _lane(repo, "awaiting_approval", "plan", reason="awaiting approval: sakura flow approve demo checks (read x)")
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "--actor=cli:owner", "demo", "spec"]) == exits.USAGE
    assert "no longer waiting" in capsys.readouterr().out
    assert "spec" not in store.TaskStore(repo, "demo").read_state()["approvals"]


def test_Approve_TheGateThatWaits_IsAccepted(repo, monkeypatch):
    ctx = _lane(repo, "awaiting_approval", "intake", reason="awaiting approval: sakura flow approve demo spec (read x)")
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "--actor=cli:owner", "demo", "spec"]) == exits.OK
    assert "spec" in store.TaskStore(repo, "demo").read_state()["approvals"]


# -- a delivered lane shows as delivered, with what it delivered (proof defect D9) -------------------------------------------

BRANCH = "feat/x"
HEAD40 = "c" * 40
PUSHED = {"url": "", "head": HEAD40, "note": "branch feat/x pushed to /srv/remote.git; open the pull request yourself"}
OPENED = {"url": "https://example.test/pull/7", "head": HEAD40}


def _delivered_lane(repo, pr, status="report_ready"):
    return _lane(repo, status, "report", branch=BRANCH, pr=pr)


@pytest.mark.parametrize("pr", [PUSHED, OPENED], ids=["push", "pull request"])
def test_Delivered_SnapshotItemSaysDeliveredWithTheBranchHeadOrThePullRequest(repo, pr):
    _delivered_lane(repo, pr)
    item = _offered(repo)[0]
    assert item.agent_state == "delivered" and item.delivery["head"] == HEAD40 and item.delivery["branch"] == BRANCH
    assert item.delivery["url"] == pr["url"]


def test_Delivered_ApiDocumentCarriesTheDelivery(repo):
    from sakura.cockpit import api

    _delivered_lane(repo, OPENED)
    document = api._item(_offered(repo)[0])
    assert document["agent_state"] == "delivered"
    assert document["delivery"] == {"kind": "pull_request", "head": HEAD40, "branch": BRANCH, "url": OPENED["url"]}


def test_Delivered_ALaneThatDeliveredNothing_StaysUnknownWithNoDelivery(repo):
    _lane(repo, "report_ready", "report", branch=BRANCH, pr={})
    item = _offered(repo)[0]
    assert item.agent_state == "unknown" and item.delivery is None


@pytest.mark.parametrize("status", ["report_ready", "cleaned"])
def test_Delivered_FlowStatusSaysDeliveredWithWhatWasDelivered(repo, monkeypatch, capsys, status):
    _delivered_lane(repo, PUSHED, status)
    monkeypatch.chdir(repo)
    assert cli.main(["status", "demo"]) == exits.OK
    out = capsys.readouterr().out
    assert "status: delivered" in out and f"branch {BRANCH} pushed" in out and HEAD40[:12] in out
    assert cli.main(["status", "demo", "--json"]) == exits.OK
    shown = json.loads(capsys.readouterr().out)
    assert shown["status"] == "delivered" and shown["engine_status"] == status and shown["delivery"]["head"] == HEAD40


def test_Delivered_FlowStatusOfAPullRequest_NamesIt(repo, monkeypatch, capsys):
    _delivered_lane(repo, OPENED)
    monkeypatch.chdir(repo)
    assert cli.main(["status", "demo"]) == exits.OK
    assert f"delivered: pull request {OPENED['url']}" in capsys.readouterr().out


def test_Delivered_ProjectListSaysDelivered(repo, monkeypatch, capsys):
    _delivered_lane(repo, PUSHED)
    monkeypatch.chdir(repo)
    assert cli.main(["status"]) == exits.OK
    assert "status: delivered" in capsys.readouterr().out


def test_Delivered_TheMastersViewSaysDeliveredWithTheDelivery(repo):
    from sakura.master import tools

    _delivered_lane(repo, PUSHED)
    row = tools._lane_row(_offered(repo)[0], [])
    assert row["state"] == "delivered" and row["delivery"]["head"] == HEAD40 and row["delivery"]["branch"] == BRANCH


# -- a cleaned and a stopped lane have their own words, and the master counts open lanes as the ceiling does (proof defect D12) ---


@pytest.mark.parametrize(("status", "word"), [("cleaned", "cleaned"), ("stopped", "stopped")])
def test_OwnWords_CleanedAndStoppedLanes_AreNeverUnknownInTheSnapshotFlowStatusOrTheMaster(repo, monkeypatch, capsys, status, word):
    from sakura.master import tools

    _lane(repo, status, "build")
    item = _offered(repo)[0]
    assert item.agent_state == word and tools._lane_row(item, [])["state"] == word
    monkeypatch.chdir(repo)
    assert cli.main(["status", "demo"]) == exits.OK
    assert f"status: {word}" in capsys.readouterr().out


@pytest.mark.parametrize("status", ["cleaned", "stopped"])
def test_OwnWords_TheApiDocumentCarriesCleanedAndStoppedAsAgentStateValues(repo, status):
    from sakura.cockpit import api

    _lane(repo, status, "build")
    assert api._item(_offered(repo)[0])["agent_state"] == status


def test_OwnWords_ADeliveredCleanedLane_StillSaysDelivered(repo):
    _lane(repo, "cleaned", "cleanup", branch=BRANCH, pr=OPENED)
    assert _offered(repo)[0].agent_state == "delivered"


@pytest.fixture
def master_world(repo, tmp_path):
    from test_master_server import World

    return World(repo, tmp_path)


def test_MasterLanes_SaysHowManyLanesAreOpenAndWhichCountAgainstTheCeiling(master_world, repo):
    for task, status in (("a", "running"), ("b", "stopped"), ("c", "blocked"), ("d", "failed"), ("e", "cleaned"),
                         ("f", "report_ready")):
        make_context(repo, task=task, state={"status": status, "phase": "build"})
    document = json.loads(master_world.tools.lanes({})[0])
    rows = {row["lane"].split("/")[1]: row for row in document["lanes"]}
    counted = [task for task, row in rows.items() if row["open"]]
    assert sorted(counted) == sorted(name.split("/")[1] for name in lane_ceiling.open_lanes([repo]))
    assert document["open_lanes"] == len(counted) == 4 and rows["e"]["open"] is False and rows["b"]["open"] is True
    assert rows["d"]["open"] is True, "a failed lane shows as blocked and is open"


def test_MasterLanes_TwoCleanedLanesAtTheCeiling_AreNotOpenAndANewLaneIsAdmitted(master_world, repo):
    for task in ("a", "b"):
        make_context(repo, task=task, state={"status": "cleaned", "phase": "cleanup"})
    document = json.loads(master_world.tools.lanes({})[0])
    assert document["open_lanes"] == 0 and all(row["open"] is False for row in document["lanes"])
    with lane_ceiling.admit(actors.Actor(actors.MASTER_SOURCE, "s1"), 2, [repo]):
        pass


def test_MasterInstructions_NameTheOpenCountAsTheOnlyThingThatCountsAgainstTheLimit():
    from sakura.master import spec

    text = " ".join(spec.BASE_INSTRUCTIONS.read_text().split())
    assert "`open_lanes`" in text and "cleaned" in text and "does not count" in text


# -- the first call of `cockpit act a` refuses when nothing is pending (no confirmation token) ----------------------------------


def _controller(repo, tmp_path, spawns=None):
    from datetime import datetime, timezone

    from sakura.cockpit import snapshot
    from sakura.cockpit.controller import Controller
    from sakura.flow import registry

    registry.remember(repo)
    collect = lambda: snapshot.collect(registry.repos(), datetime(2026, 11, 17, 3, 0, tzinfo=timezone.utc))  # noqa: E731
    return Controller(b"s" * 32, collect, collect, run=spawns or (lambda *a, **k: None), log_path=tmp_path / "log.jsonl")


@pytest.mark.parametrize("status", ["blocked", "failed", "stopped", "cleaned", "report_ready", "running"])
def test_ActApprove_FirstCallRefusesWhenNothingIsPending_NoTokenIsHandedOut(repo, tmp_path, status):
    from sakura.cockpit.controller import PROOF_SOURCE_CLI, Refused

    _lane(repo, status, "build")
    controller = _controller(repo, tmp_path)
    item = controller.fresh_snapshot().items[0]
    outcome = controller.perform({"key": "a", "target": json.dumps(list(item.key))}, PERSON, proof_field="confirm",
                                 proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Refused) and "no longer waiting" in outcome.reason and not outcome.denied


def test_ActApprove_FirstCallAtAGate_HandsOutTheToken(repo, tmp_path):
    from sakura.cockpit.controller import PROOF_SOURCE_CLI, Confirm

    ctx = _lane(repo, "awaiting_approval", "plan", reason="awaiting approval: sakura flow approve demo spec (read x)")
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    controller = _controller(repo, tmp_path)
    item = controller.fresh_snapshot().items[0]
    outcome = controller.perform({"key": "a", "target": json.dumps(list(item.key))}, PERSON, proof_field="confirm",
                                 proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Confirm)


# -- the confirmation says the ask in words: no gate name, no revision token, the command in its own field -----------------------


def _approval_confirmation(repo, tmp_path, target_of):
    from sakura.cockpit import api
    from sakura.cockpit.controller import PROOF_SOURCE_CLI, Confirm

    controller = _controller(repo, tmp_path)
    snapshot = controller.fresh_snapshot()
    outcome = controller.perform({"key": "a", "target": target_of(snapshot)}, PERSON, proof_field="confirm",
                                 proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Confirm)
    return api.action_document(outcome)


def test_ActApprove_ConfirmationOfAWait_SaysTheAskInWordsAndKeepsTheCommandInItsOwnField(repo, tmp_path):
    ctx = _lane(repo, "awaiting_approval", "intake", reason="awaiting approval: sakura flow approve demo spec (read x)")
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    document = _approval_confirmation(repo, tmp_path, lambda snapshot: snapshot.waits[0].id)
    assert document["summary"] == "Approve: Read the spec and approve it"
    assert "@" not in document["summary"] and "spec" in document["command"] and "--revision=" in document["command"]


def test_ActApprove_ConfirmationOfALaneAtAGate_NamesTheLaneByItsTitle(repo, tmp_path):
    ctx = _lane(repo, "awaiting_approval", "intake", reason="awaiting approval: sakura flow approve demo spec (read x)")
    ctx.store.update_state(lambda state: state.update({tasks.TITLE_KEY: "Add a greeting"}))
    document = _approval_confirmation(repo, tmp_path, lambda snapshot: json.dumps(list(snapshot.items[0].key)))
    assert document["summary"] == "Approve: Add a greeting"
    assert "demo" not in document["summary"] and "flow approve" in document["command"]


def test_ActApprove_RefusalWhenNothingWaits_SaysSoWithoutTheGateOrTheLaneId(repo, tmp_path):
    from sakura.cockpit.controller import PROOF_SOURCE_CLI, Refused

    _lane(repo, "running", "build")
    controller = _controller(repo, tmp_path)
    item = controller.fresh_snapshot().items[0]
    outcome = controller.perform({"key": "a", "target": json.dumps(list(item.key))}, PERSON, proof_field="confirm",
                                 proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Refused) and "no longer waiting" in outcome.reason
    assert "gate" not in outcome.reason and "demo" not in outcome.reason
