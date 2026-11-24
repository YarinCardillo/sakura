"""A lane's goal, steps, opener, preset, pace and account in the snapshot (`cockpit/lane_facts.py`)."""

from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura.cockpit import api, lane_facts
from sakura.flow import config, store
from sakura.outside import records
from sakura.outside import store as outside_store
from test_cockpit_actions import flow_item
from test_cockpit_snapshot import collect, item_for, make_snapshot

OUTSIDE_NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)


def knobs_of(preset="verified"):
    return config.load(None, overrides={"preset": preset, "relax": ["preset"]}).values


def task_in(repo, task_id="demo", *, goal="Add a --help flag\nKeep it short\n", opener="cli:alice", **state):
    """A task as `flow start` leaves it: input, state and the first start line of the ledger."""
    task = store.TaskStore(repo, task_id)
    base = {"task": task_id, "ref": f"specs/{task_id}.md", "phase": "build", "status": "running", "reason": "",
            "history": [], "approvals": {}, "completed": {}, "active_run": None, "overrides": {}}
    task.create({**base, **state}, goal.encode())
    task.append_ledger("operator", f"start requested actor={opener}")
    return task


def facts_of(task, **state):
    return lane_facts.lane_facts(task.root, task.read_state(), knobs_of(**state))


def steps_of(facts):
    return [(step["id"], step["state"]) for step in facts["steps"]]


def test_LaneFacts_BuildPhase_PlanDoneBuildNowChecksAndDeliverNext(repo):
    facts = facts_of(task_in(repo, phase="build"))

    assert steps_of(facts) == [("plan", "done"), ("build", "now"), ("checks", "next"), ("deliver", "next")]


@pytest.mark.parametrize("phase", ["intake", "contract", "plan"])
def test_LaneFacts_AnyPlanGroupPhase_PlanIsNow(repo, phase):
    facts = facts_of(task_in(repo, phase=phase))

    assert steps_of(facts)[0] == ("plan", "now")
    assert steps_of(facts)[1] == ("build", "next")


@pytest.mark.parametrize("phase", ["pr", "report"])
def test_LaneFacts_DeliverPhases_DeliverIsNow(repo, phase):
    facts = facts_of(task_in(repo, phase=phase))

    assert steps_of(facts) == [("plan", "done"), ("build", "done"), ("checks", "done"), ("deliver", "now")]


def test_LaneFacts_PresetWithoutEvaluatorButWithChecks_ChecksStillRun(repo):
    facts = facts_of(task_in(repo, phase="build"), preset="standard")

    assert steps_of(facts) == [("plan", "done"), ("build", "now"), ("checks", "next"), ("deliver", "next")]


def test_LaneFacts_PresetWithChecksOff_ChecksAreSkipped(repo):
    facts = facts_of(task_in(repo, phase="build"), preset="solo")

    assert ("checks", "skipped") in steps_of(facts)


def test_LaneFacts_PlanSkippedInHistory_PlanIsSkipped(repo):
    history = [{"phase": "plan", "status": "skipped", "at": "2026-11-18T10:00:00+00:00", "detail": ""}]

    facts = facts_of(task_in(repo, phase="build", history=history))

    assert steps_of(facts)[0] == ("plan", "skipped")


def test_LaneFacts_FinishedLane_AllStepsDone(repo):
    facts = facts_of(task_in(repo, phase="cleanup", status="report_ready"))

    assert steps_of(facts) == [("plan", "done"), ("build", "done"), ("checks", "done"), ("deliver", "done")]


def test_LaneFacts_BlockedLane_TheStepItStoppedAtStaysNow(repo):
    facts = facts_of(task_in(repo, phase="build", status="blocked", reason="tests failed"))

    assert steps_of(facts)[1] == ("build", "now")


def test_LaneFacts_StepModels_ComeFromTheRoles(repo):
    facts = facts_of(task_in(repo))

    roles = knobs_of()["roles"]
    assert [step["model"] for step in facts["steps"]] == [roles["planner"], roles["builder"], roles["evaluator"],
                                                          roles["reporter"]]


def test_LaneFacts_Account_IsNullUntilLeewayAccountsArrive(repo):
    facts = facts_of(task_in(repo))

    assert [step["account"] for step in facts["steps"]] == [None] * 4
    assert all(review["account"] is None for review in facts["reviews"])


def test_LaneFacts_PaceIsNormalUntilPaceArrives(repo):
    assert facts_of(task_in(repo))["pace"] == "normal"


def test_LaneFacts_PresetAndDial_AreTheLanesEffectiveOnes(repo):
    facts = facts_of(task_in(repo), preset="solo")

    assert (facts["preset"], facts["dial"]) == ("solo", "balanced")


def test_LaneFacts_Reviews_AreReportedSeparatelyWithOnAndState(repo):
    facts = facts_of(task_in(repo, phase="build"))

    roles = knobs_of()["roles"]
    assert facts["reviews"] == [
        {"id": "plan_review", "on": True, "model": roles["plan_reviewer"], "account": None, "state": "done"},
        {"id": "task_review", "on": True, "model": roles["code_reviewer"], "account": None, "state": "now"},
        {"id": "final_review", "on": False, "model": None, "account": None, "state": "skipped"}]


def test_LaneFacts_PresetWithoutReviewer_ReviewsAreOffAndSkipped(repo):
    facts = facts_of(task_in(repo), preset="solo")

    assert [(review["id"], review["on"], review["state"]) for review in facts["reviews"]] == [
        ("plan_review", False, "skipped"), ("task_review", False, "skipped"), ("final_review", False, "skipped")]


def test_LaneFacts_PlanSkipped_PlanReviewIsSkippedWithIt(repo):
    history = [{"phase": "plan", "status": "skipped", "at": "2026-11-18T10:00:00+00:00", "detail": ""}]

    facts = facts_of(task_in(repo, history=history))

    assert facts["reviews"][0]["state"] == "skipped"


def test_LaneFacts_ReviewerRunLive_InReviewIsTrue(repo):
    runs = {"review-1": {"name": "review-1", "role": "code_reviewer", "state": "running"}}

    assert facts_of(task_in(repo, runs=runs))["in_review"] is True


def test_LaneFacts_BuilderRunLive_InReviewIsFalse(repo):
    runs = {"build-1": {"name": "build-1", "role": "builder", "state": "running"}}

    assert facts_of(task_in(repo, runs=runs))["in_review"] is False


def test_LaneFacts_FinishedReviewerRun_InReviewIsFalse(repo):
    runs = {"review-1": {"name": "review-1", "role": "code_reviewer", "state": "exited"}}

    assert facts_of(task_in(repo, runs=runs))["in_review"] is False


def test_LaneFacts_Goal_KeepsEveryLineOfTheInput(repo):
    goal = "# Add a flag\n\nLine two\n\n  indented line three\nlast\n"

    assert facts_of(task_in(repo, goal=goal))["goal"] == goal


def test_LaneFacts_LongGoal_IsCappedAtEightThousandCharacters(repo):
    facts = facts_of(task_in(repo, goal="x" * 9000))

    assert facts["goal"] == "x" * lane_facts.GOAL_LIMIT
    assert lane_facts.GOAL_LIMIT == 8000


def test_LaneFacts_NoInputFile_GoalIsEmpty(repo):
    task = store.TaskStore(repo, "bare")
    task.create({"task": "bare", "ref": "specs/bare.md", "phase": "build", "status": "running", "reason": "",
                 "history": [], "approvals": {}, "completed": {}, "active_run": None, "overrides": {}})

    facts = lane_facts.lane_facts(task.root, task.read_state(), knobs_of())

    assert facts["goal"] == ""


@pytest.mark.parametrize("actor, expected", [("master:coordinator", "master"), ("cli:alice", "person"),
                                              ("app:alice", "person"), ("overlay:alice@example.com:nPHONE", "person")])
def test_LaneFacts_Opener_IsMasterOrPerson(repo, actor, expected):
    assert facts_of(task_in(repo, opener=actor))["opened_by"] == expected


def test_LaneFacts_NoStartLine_OpenerIsPerson(repo):
    task = task_in(repo)
    task.ledger.write_text("# Ledger: demo\n")

    assert facts_of(task)["opened_by"] == "person"


def test_Snapshot_FlowLane_ItemCarriesTheFacts(repo):
    task_in(repo, "demo", phase="build", opener="master:coordinator")

    item = item_for(collect([repo]), "demo")
    document = next(row for row in api.snapshot_document(collect([repo]))["items"] if row["key"][2] == "demo")

    assert item.facts["opened_by"] == "master"
    assert document["opened_by"] == "master"
    assert document["goal"].startswith("Add a --help flag")
    assert [step["id"] for step in document["steps"]] == ["plan", "build", "checks", "deliver"]
    assert document["pace"] == "normal"


def test_Snapshot_UnreadableTask_ItemHasNoFacts(repo):
    task = task_in(repo)
    task.state_file.write_text("{not json")

    item = item_for(collect([repo]), "demo")

    assert item.facts == {}


def test_Snapshot_OutsideLane_HasNoStepsAndGoalFromItsReport(repo):
    root = outside_store.path()
    agent = records.Agent("claude", repo)
    outside_store.open_lane(root, agent, "measure-help", title="Add --help", about="Prints usage on --help\nand -h",
                            now=OUTSIDE_NOW)

    found = collect([repo])
    document = next(row for row in api.snapshot_document(found)["items"] if row["kind"] == "outside")

    assert document["steps"] == []
    assert document["goal"] == "Prints usage on --help\nand -h"
    assert document["opened_by"] == "outside"
    assert document["reviews"] == []


def test_Snapshot_OutsideLaneWithoutAbout_GoalIsTheTitle(repo):
    root = outside_store.path()
    outside_store.open_lane(root, records.Agent("claude", repo), "measure-help", title="Add --help", now=OUTSIDE_NOW)

    document = next(row for row in api.snapshot_document(collect([repo]))["items"] if row["kind"] == "outside")

    assert document["goal"] == "Add --help"


def test_Document_ItemWithoutFacts_OmitsTheFactFields():
    document = api.snapshot_document(make_snapshot(items=[flow_item(Path("/work/repo"), "t1")]))

    assert "goal" not in document["items"][0]
    assert "steps" not in document["items"][0]


def values_with(*, knobs=None, step_accounts=None):
    values = knobs_of()
    return {**values, "knobs": {**values["knobs"], **(knobs or {})}, "step_accounts": step_accounts or {}}


def test_LaneFacts_LanePaceOverride_IsTheLanesPace(repo):
    task = task_in(repo, overrides={"knobs": {"lane_pace": "slow"}})

    assert lane_facts.lane_facts(task.root, task.read_state(), knobs_of())["pace"] == "slow"


def test_LaneFacts_PlanSwitchedOffByThePreset_PlanIsSkipped(repo):
    task = task_in(repo, phase="build")

    facts = lane_facts.lane_facts(task.root, task.read_state(), values_with(knobs={"plan": False}))

    assert steps_of(facts)[0] == ("plan", "skipped")


def test_LaneFacts_StepAccounts_AreTheAccountsTheStepsRunOn(repo):
    task = task_in(repo, budget={"pace_account": {"build": "spare"}})
    values = values_with(step_accounts={"build": "work", "checks": "work"})

    facts = lane_facts.lane_facts(task.root, task.read_state(), values)

    assert {step["id"]: step["account"] for step in facts["steps"]} == {
        "plan": None, "build": "spare", "checks": "work", "deliver": None}
