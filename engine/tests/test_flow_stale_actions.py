"""Review 17, finding 4: an answer binds the revision the person saw, and the engine checks it under the task lock."""

import json

import pytest

from sakura import engine_home
from sakura.cockpit import actions
from sakura.cockpit.controller import Refused, overlay_actor
from sakura.flow import checks, cli, contract, exits, owner_marks, store, waits
from flow_support import make_context
from test_cockpit_actions import ACTOR_OPTION, PROGRAM, OWNER, Spawns, log_rows, wait_target
from test_cockpit_controller import _perform, _waiting, form, world  # noqa: F401  (the controller fixture)
from test_flow_cli import _start, _state, project  # noqa: F401  (the fixtures)
from test_flow_owner_continue import ACTOR, owner_task

CANDIDATE_A = {"contract": "a" * 64, "commands": ["pytest -q"], "sha256": checks.digest(["pytest -q"])}
CANDIDATE_B = {**CANDIDATE_A, "contract": "b" * 64}


def _at_checks_gate(repo, candidate):
    task = store.TaskStore(repo, "add-greeting")
    task.update_state(lambda state: state.update({checks.PENDING_KEY: candidate, "status": "awaiting_approval",
                                                  "reason": "awaiting approval: sakura flow approve add-greeting checks"}))
    return task


def _revision(task, gate):
    return waits.approval_revision(task, task.read_state(), gate)


# -- the revision a wait names ---------------------------------------------------------------------------


def test_ApprovalRevision_ACandidateReplacedUnderTheSameGate_IsAnotherRevision(project):
    repo, _ = project
    _start(project)
    task = _at_checks_gate(repo, CANDIDATE_A)
    first = _revision(task, "checks")
    assert _revision(task, "checks") == first, "stable while nothing changes"

    task.update_state(lambda state: state.update({checks.PENDING_KEY: CANDIDATE_B}))

    assert _revision(task, "checks") != first


def test_Waits_AnApprovalAndAnOwnerRowNameTheRevisionInTheirId(repo):
    ctx = make_context(repo, task="add-greeting")
    rows = [contract.Row("R1", "fixture", "check", "cli", None, "pytest -q", False, False)]
    digest = contract.save(ctx.store, rows)
    task = _at_checks_gate(repo, {**CANDIDATE_A, "contract": digest})
    task.update_state(lambda state: state.pop("contract_sha256"))
    owner = owner_task(repo, ("R1",))

    found = {wait.kind: waits.split_key(json.loads(wait.id)[4]) for wait in waits.collect([repo])}

    assert found["approval"] == ("checks", _revision(task, "checks"))
    assert found["owner_row"] == ("R1", waits.owner_revision(owner.read_state(), "R1"))


def test_Waits_AnOwnerRowAtAnotherHeadOrContract_IsAnotherWait(repo):
    task = owner_task(repo, ("R1",))
    before = waits.collect([repo])
    task.update_state(lambda state: state.update(contract_sha256="0" * 64))
    after = [wait for wait in waits.collect([repo]) if wait.kind == "owner_row"]

    assert [wait.kind for wait in before if wait.kind == "owner_row"] == ["owner_row"]
    assert after == [] or all(wait.id != before[0].id for wait in after)


# -- the engine refuses a stale answer under the task lock -----------------------------------------------------


def test_Approve_ChecksCandidateReplacedByAnother_IsRefusedNothingApprovedNothingRemembered(project, capsys):
    repo, _ = project
    _start(project)
    task = _at_checks_gate(repo, CANDIDATE_A)
    seen = _revision(task, "checks")
    task.update_state(lambda state: state.update({checks.PENDING_KEY: CANDIDATE_B}))

    assert cli.main(["approve", "add-greeting", "checks", "--remember", f"--revision={seen}"]) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out
    assert checks.APPROVAL_GATE not in _state(repo)["approvals"]
    assert not engine_home.remembered_checks_file(repo).exists()


def test_Approve_TheRevisionThePersonSaw_Approves(project):
    repo, _ = project
    _start(project)
    task = _at_checks_gate(repo, CANDIDATE_A)

    assert cli.main(["approve", "add-greeting", "checks", f"--revision={_revision(task, 'checks')}"]) == exits.OK

    assert _state(repo)["approvals"][checks.APPROVAL_GATE]["sha256"] == CANDIDATE_A["sha256"]


def test_Approve_ASpecChangedAfterThePageWasDrawn_IsRefused(project, capsys):
    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    seen = _revision(task, "spec")
    task.spec.write_text(task.spec.read_text() + "\nA new journey.\n")

    assert cli.main(["approve", "add-greeting", "spec", f"--revision={seen}"]) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out and "spec" not in _state(repo)["approvals"]


def test_Approve_DetachedDispatch_CarriesTheRevisionToTheWindowAndChecksItFirst(project, monkeypatch):
    repo, _ = project
    _start(project)
    task = _at_checks_gate(repo, CANDIDATE_A)
    seen = _revision(task, "checks")
    sent = []

    def detach(config, task_store, command, before=lambda: None):
        before()
        sent.append(command)
        return exits.OK

    monkeypatch.setattr(cli, "_detach", detach)
    assert cli.main(["approve", "--continue", "--detach", f"--revision={seen}", "add-greeting", "checks"]) == exits.OK
    assert f"--revision={seen}" in sent[0]

    task.update_state(lambda state: state.update({checks.PENDING_KEY: CANDIDATE_B}))
    assert cli.main(["approve", "--continue", "--detach", f"--revision={seen}", "add-greeting", "checks"]) == exits.USAGE
    assert len(sent) == 1


def test_Mark_AnOwnerRowWhoseContractMovedOn_IsRefusedAndMarksNothing(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    seen = waits.owner_revision(task.read_state(), "R1")
    task.update_state(lambda state: state.update(contract_sha256=state["contract_sha256"][::-1]))

    assert cli.main(["mark", "demo", "R1", "ok", f"--revision={seen}", f"--actor={ACTOR}"]) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out
    assert owner_marks.MARKS_KEY not in task.read_state()


# -- the cockpit sends the revision, and a stale confirmation is refused ---------------------------------------


def test_Act_ApproveOfARevisionedWait_PassesTheRevisionToTheCommand(log_path):
    target = wait_target("approval", "checks@0123456789abcdef")
    spawns = Spawns()
    actions.run("a", target, {}, overlay_actor_for_tests(), run=spawns, log_path=log_path)

    (argv, _), = spawns.calls
    assert argv == [PROGRAM, "flow", "approve", "--continue", "--detach", "--revision=0123456789abcdef", ACTOR_OPTION,
                    "--", "t1", "checks"]


def test_Act_MarkOfARevisionedWait_PassesTheRevisionToTheCommand(log_path):
    target = wait_target("owner_row", "R1@0123456789abcdef")
    spawns = Spawns()
    actions.run("k", target, {"verdict": "ok"}, overlay_actor_for_tests(), run=spawns, log_path=log_path)

    assert spawns.calls[0][0][:4] == [PROGRAM, "flow", "mark", "--continue"]
    assert "--revision=0123456789abcdef" in spawns.calls[0][0]
    assert spawns.calls[0][0][-3:] == ["t1", "R1", "ok"]


def test_Act_AConfirmationForCandidateA_IsRefusedOnceTheGateWaitsOnCandidateB(world):
    first = wait_target("approval", "checks@aaaaaaaaaaaaaaaa")
    _waiting(world, first)
    fields = form(first, "a")
    asked = _perform(world, fields, source="cli")

    _waiting(world, wait_target("approval", "checks@bbbbbbbbbbbbbbbb"))  # the planner's retry: the same gate, new bytes
    late = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert isinstance(late, Refused) and "no longer waiting" in late.reason and world.spawns.calls == []


def test_Act_ARefusedApprovalCommand_AnswersNoLongerWaiting(world):
    target = wait_target("approval", "checks@aaaaaaaaaaaaaaaa")
    _waiting(world, target)
    world.spawns._answer = ("no longer waiting: the commands changed\n", "", exits.USAGE)
    fields = form(target, "a")
    asked = _perform(world, fields, source="cli")

    outcome = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert isinstance(outcome, Refused) and "no longer waiting" in outcome.reason


@pytest.fixture
def log_path(tmp_path):
    return tmp_path / "actions.jsonl"


def overlay_actor_for_tests():
    return OWNER


def test_Act_ItemApproveConfirmedAfterTheTaskStateMoved_IsRefusedAndNothingRuns(world):
    # An approval from the task's own card (no wait key) binds the state revision the person saw, like a resume.
    import dataclasses

    seen = dataclasses.replace(world.flow, resume_revision="state-a")
    _snapshot_with(world, seen)
    fields = form(seen, "a")
    asked = _perform(world, fields, source="cli")

    _snapshot_with(world, dataclasses.replace(world.flow, resume_revision="state-b"))
    late = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert isinstance(late, Refused) and late.denied and world.spawns.calls == []


def _snapshot_with(world, item):
    from test_cockpit_snapshot import make_snapshot

    world.fresh.snapshot = make_snapshot(items=[item, world.lane])
    return world.fresh.snapshot


def test_CardApprove_BReplacesAAfterTheControllersCheck_TheEngineRefusesAndRemembersNothing(project, monkeypatch, capsys):
    # Review 17, finding 4 (card path): the card's approval carries the revision the person saw, and the engine checks it
    # under the task lock, so a candidate B written after the controller's own check is not approved.
    from sakura.cockpit import items

    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child: the only process that may carry a viewer's identity
    repo, _ = project
    _start(project)
    task = _at_checks_gate(repo, CANDIDATE_A)
    card = items.flow_item(task, lambda key: None)
    invocation = actions.plan("a", card, {"remember": "yes"}, OWNER)
    assert any(word.startswith("--revision=") for word in invocation.argv)
    sent = []

    def detach(config, task_store, command, before=lambda: None):
        before()
        sent.append(command)
        return exits.OK

    monkeypatch.setattr(cli, "_detach", detach)
    task.update_state(lambda state: state.update({checks.PENDING_KEY: CANDIDATE_B}))  # the planner's retry

    arguments = invocation.argv[invocation.argv.index("approve"):]
    assert cli.main([*arguments]) == exits.USAGE

    assert sent == [] and "no longer waiting" in capsys.readouterr().out
    assert checks.APPROVAL_GATE not in _state(repo)["approvals"]
    assert not engine_home.remembered_checks_file(repo).exists()


def test_Act_CardApproveConfirmedAfterTheSpecWasEditedElsewhere_IsRefusedAndNothingRuns(world):
    # Review 18, finding 3: the confirmation binds the gate revision the person saw. A spec edited on another device
    # changes no persisted state (the resume revision stays) but changes that revision.
    import dataclasses

    seen = dataclasses.replace(world.flow, resume_revision="same-state", approval_revision="spec-a")
    _snapshot_with(world, seen)
    fields = form(seen, "a")
    asked = _perform(world, fields, source="cli")

    _snapshot_with(world, dataclasses.replace(seen, approval_revision="spec-b"))
    late = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert isinstance(late, Refused) and late.denied and world.spawns.calls == []


def test_Act_CardApprove_SendsTheRevisionTheConfirmationWasIssuedFor(world):
    import dataclasses

    seen = dataclasses.replace(world.flow, approval_revision="spec-a")
    _snapshot_with(world, seen)
    fields = form(seen, "a")
    asked = _perform(world, fields, source="cli")

    done = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert not isinstance(done, Refused) and "--revision=spec-a" in world.spawns.calls[0][0]


def test_Act_RestoreConfirmedAfterTheWorkspaceHeadChanged_IsRefusedAndNothingRuns(world):
    # Review 19, finding 1: the confirmation binds the workspace head and the sealed destination the person saw.
    import dataclasses

    shown = dataclasses.replace(wait_target("owner_row", "R1@rev").wait, workspace_head="M", engine_head="H")
    from sakura.cockpit import wait_actions

    _waiting(world, wait_actions.target_of(shown))
    fields = form(wait_actions.target_of(shown), "H")
    asked = _perform(world, fields)
    assert hasattr(asked, "token"), "the restore asks first"

    after = dataclasses.replace(shown, workspace_head="P")  # another device restored and the person committed P
    _waiting(world, wait_actions.target_of(after))
    late = _perform(world, {**fields, "proof": asked.token})

    assert isinstance(late, Refused) and world.spawns.calls == []


def test_Act_RestoreCommand_CarriesWhatThePersonSawForTheEngineToRecheck(log_path):
    import dataclasses
    from sakura.cockpit import wait_actions

    shown = dataclasses.replace(wait_target("owner_row", "R1@rev").wait, workspace_head="M", engine_head="H")
    spawns = Spawns()
    actions.run("H", wait_actions.target_of(shown), {}, overlay_actor_for_tests(), run=spawns, log_path=log_path)

    argv = spawns.calls[0][0]
    assert argv[1:3] == ["flow", "restore-head"] and argv[-2:] == ["--", "t1"]
    assert {"--expect-workspace=M", "--expect-head=H", "--row=R1", "--revision=rev"} <= set(argv)


# -- the waits S4 added bind their revision the same way --------------------------------------------------------------

OVERLAP_REASON = "awaiting approval: sakura flow approve add-greeting overlap"


def _at_overlap_gate(repo, lane):
    task = store.TaskStore(repo, "add-greeting")
    task.update_state(lambda state: state.update({"overlap": {"head": "a" * 40, "lane": lane, "files": ["work.txt"]},
                                                  "status": "awaiting_approval", "reason": OVERLAP_REASON}))
    return task


def test_ApprovalRevision_AnotherLanesOverlapUnderTheSameGate_IsAnotherRevision(project):
    repo, _ = project
    _start(project)
    task = _at_overlap_gate(repo, "repo/first")
    first = _revision(task, "overlap")

    task = _at_overlap_gate(repo, "repo/second")

    assert _revision(task, "overlap") != first


def test_Approve_OverlapReplacedByAnotherLanesAfterThePageWasDrawn_IsRefusedNothingApproved(project, capsys):
    repo, _ = project
    _start(project)
    task = _at_overlap_gate(repo, "repo/first")
    seen = _revision(task, "overlap")
    _at_overlap_gate(repo, "repo/second")

    assert cli.main(["approve", "add-greeting", "overlap", f"--revision={seen}"]) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out and "overlap" not in _state(repo)["approvals"]


def test_Waits_AQuestionNamesItsIdAndTheDigestThePersonSawAsItsRevision(project):
    from datetime import datetime, timezone

    from sakura.flow import lane_questions

    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    asked = lane_questions.ask(task, "builder", "build-1", "Also accept -h?", ["yes", "no"], datetime.now(timezone.utc))

    [found] = [wait for wait in waits.collect([repo]) if wait.kind == waits.QUESTION]

    assert waits.split_key(json.loads(found.id)[4]) == (asked.id, asked.digest)


def test_Act_ReplyOfARevisionedQuestion_SendsItsIdAndRevision(log_path):
    target = wait_target("question", "q0123abcd@0123456789ab")
    spawns = Spawns()
    actions.run("r", target, {"text": "yes"}, overlay_actor_for_tests(), run=spawns, log_path=log_path)

    assert spawns.calls[0][0][-3:] == ["t1", "q0123abcd", "0123456789ab"]


def test_Act_ProposalDecision_SendsItsIdAndRevision(log_path):
    target = wait_target("proposal", "p0123abcd@0123456789ab")
    spawns = Spawns()
    actions.run("y", target, {}, overlay_actor_for_tests(), run=spawns, log_path=log_path)

    assert spawns.calls[0][0][1:3] == ["proposal", "accept"] and spawns.calls[0][0][-2:] == ["p0123abcd", "0123456789ab"]


@pytest.mark.parametrize("key", ["y", "N"])
def test_Act_AConfirmationForAProposal_IsRefusedOnceItsTextChanged(world, key):
    first = wait_target("proposal", "p0123abcd@aaaaaaaaaaaa")
    _waiting(world, first)
    fields = form(first, key)
    asked = _perform(world, fields)
    assert hasattr(asked, "token"), "a proposal decision asks first"

    _waiting(world, wait_target("proposal", "p0123abcd@bbbbbbbbbbbb"))  # the same proposal id, another text
    late = _perform(world, {**fields, "proof": asked.token})

    assert isinstance(late, Refused) and world.spawns.calls == []


# -- a budget confirmation is bound to the wait it was shown for (review 25, finding 3) --------------------------------------


def _park(task, action="pause", rule="week", until="2026-11-20T08:00:00+00:00"):
    from datetime import datetime, timedelta, timezone

    from sakura.budget import guard
    from sakura.flow import budget_guard

    now = datetime(2026, 11, 17, tzinfo=timezone.utc)
    decision = guard.Decision(action, datetime.fromisoformat(until), "reserve", rule)
    budget_guard.park(task, decision, now)
    return now + timedelta(0)


def test_ApprovalRevision_AnotherBudgetWaitAtTheSameHead_IsAnotherRevision(project):
    from sakura.flow import budget_guard

    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    _park(task)
    first = _revision(task, "budget-wait")
    assert _revision(task, "budget-wait") == first, "stable while the same wait lasts"

    task.update_state(lambda state: state.pop(budget_guard.WAIT_KEY))  # released elsewhere, the lane resumed
    _park(task)  # the very same rule, action and deadline, a new wait

    assert _revision(task, "budget-wait") != first


@pytest.mark.parametrize("change", [{"rule": "pacing"}, {"action": "defer"}, {"until": "2026-11-21T08:00:00+00:00"}])
def test_ApprovalRevision_TheSameWaitWithAnotherRuleActionOrDeadline_IsAnotherRevision(project, change):
    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    _park(task)
    first = _revision(task, "budget-wait")

    _park(task, **change)  # parked again while still waiting: the record changes, the wait count does not

    assert _revision(task, "budget-wait") != first
