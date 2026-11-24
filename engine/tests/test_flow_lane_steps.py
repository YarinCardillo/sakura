"""Turn one optional step on or off for one lane: queued like a dial, applied from the lane's next step, kept in the lane's own record."""

from flow_support import make_context
from sakura.cockpit import actions, lane_facts
from sakura.flow import config, engine, exits, inbox, pipeline
from sakura.flow.cli import main as flow_main
from test_cockpit_actions import flow_item, invoke

ALICE = "cli:alice"


def _lane(repo, task="a", overrides=None, **state):
    return make_context(repo, task=task, overrides=overrides, state={"overrides": overrides or {}, **state})


def _turn(ctx, step, value, actor=ALICE):
    """What the engine does at a boundary: take the queued command in."""
    inbox.enqueue(ctx.store, "step", value, role=step, actor=actor)
    return inbox.consume(ctx.store, ctx.repo, "build")


def _lane_values(ctx):
    return config.load(ctx.repo, overrides=ctx.store.read_state().get("overrides") or None).values


def _refusal(repo, capsys, monkeypatch, task, step, value):
    monkeypatch.chdir(repo)
    assert flow_main(["step", "--", task, step, value]) == exits.USAGE
    return capsys.readouterr().out


def test_Consume_StepOn_IsKeptInTheLanesOverridesAndLedgeredWithTheActor(repo):
    ctx = _lane(repo, phase="build")

    assert _turn(ctx, "final_review", "on") == 1

    overrides = ctx.store.read_state()["overrides"]
    assert overrides["knobs"]["final_review"] is True and "knobs.final_review" in overrides["relax"]
    assert _lane_values(ctx)["knobs"]["final_review"] is True
    ledger = ctx.store.ledger.read_text()
    assert "step final_review turned on" in ledger and f"actor={ALICE}" in ledger


def test_Consume_StepOff_TurnsTheStepOffForTheLane(repo):
    ctx = _lane(repo, phase="plan")

    assert _turn(ctx, "task_review", "off") == 1

    assert pipeline.of_values(_lane_values(ctx)).review("task_review").on is False


def test_Consume_StepOn_LeavesTheGlobalConfigAndAnotherLaneUntouched(repo):
    first, second = _lane(repo, task="a", phase="build"), _lane(repo, task="b", phase="build")

    _turn(first, "final_review", "on")

    assert config.load(repo).values["knobs"]["final_review"] is False
    assert _lane_values(second)["knobs"]["final_review"] is False
    assert not second.store.read_state()["overrides"].get("knobs")


def test_Consume_ReviewOnWithTheReviewerOff_TurnsOnThatReviewOnly(repo):
    ctx = _lane(repo, overrides={"preset": "vanilla", "relax": ["preset"]}, phase="plan")

    _turn(ctx, "task_review", "on")

    built = pipeline.of_values(_lane_values(ctx))
    assert [review.id for review in built.reviews if review.on] == ["task_review"]


def test_Cli_FlowStep_QueuesTheStepForTheEngineToApply(repo, monkeypatch):
    monkeypatch.chdir(repo)
    ctx = _lane(repo, phase="build")

    assert flow_main(["step", "--", "a", "final_review", "on"]) == exits.OK
    inbox.consume(ctx.store, repo, "build")

    assert _lane_values(ctx)["knobs"]["final_review"] is True


def test_Cli_FlowStep_PlanReviewAfterThePlanIsDone_IsRefusedInWords(repo, monkeypatch, capsys):
    _lane(repo, phase="build")

    out = _refusal(repo, capsys, monkeypatch, "a", "plan_review", "off")

    assert "plan review is already done" in out


def test_Cli_FlowStep_AStepThatIsRunning_IsRefusedInWords(repo, monkeypatch, capsys):
    _lane(repo, phase="evaluate")

    out = _refusal(repo, capsys, monkeypatch, "a", "checks", "off")

    assert "checks is running" in out


def test_Cli_FlowStep_Build_IsRefusedAsAlwaysOn(repo, monkeypatch, capsys):
    _lane(repo, phase="plan")

    assert "build is always on" in _refusal(repo, capsys, monkeypatch, "a", "build", "off")


def test_Cli_FlowStep_UnknownStepOrValue_IsRefused(repo, monkeypatch, capsys):
    _lane(repo, phase="plan")

    assert "unknown step" in _refusal(repo, capsys, monkeypatch, "a", "polish", "on")
    assert "on or off" in _refusal(repo, capsys, monkeypatch, "a", "checks", "maybe")


def test_Cli_FlowStep_AReviewWithNoReviewerModel_IsRefusedWithTheReason(repo, monkeypatch, capsys):
    _lane(repo, overrides={"roles": {"code_reviewer": "none"}, "relax": ["roles.code_reviewer"]}, phase="plan")

    out = _refusal(repo, capsys, monkeypatch, "a", "task_review", "on")

    assert "code_reviewer has no model" in out


def test_Cli_FlowStep_PlanReviewOnWithThePlanOff_IsRefusedWithTheReason(repo, monkeypatch, capsys):
    _lane(repo, overrides={"preset": "solo", "relax": ["preset"]}, phase="intake")

    assert "needs the plan step on" in _refusal(repo, capsys, monkeypatch, "a", "plan_review", "on")


def test_Consume_AStepTheLaneReachedWhileTheCommandWaited_IsRejectedAndKeepsNothing(repo):
    ctx = _lane(repo, phase="plan")
    inbox.enqueue(ctx.store, "step", "off", role="plan_review", actor=ALICE)
    ctx.store.update_state(lambda state: state.update(phase="build"))

    assert inbox.consume(ctx.store, repo, "build") == 0

    assert not ctx.store.read_state()["overrides"].get("knobs")


def test_Advance_ChecksTurnedOnAtBuild_RunsChecksBeforeDeliver(repo, monkeypatch):
    monkeypatch.setattr(engine, "_HANDLERS", {})
    ctx = _lane(repo, overrides={"preset": "solo", "relax": ["preset"]}, phase="build")
    ran = []

    def build(running):
        _turn(running, "checks", "on")
        return engine.PhaseOutcome("next")

    def evaluate(running):
        ran.append(("evaluate", running.config.values["knobs"]["checks"]))
        return engine.PhaseOutcome("next")

    engine.register("build", build)
    engine.register("evaluate", evaluate)
    engine.register("pr", lambda running: ran.append(("pr", None)) or engine.PhaseOutcome("await", gate="pr"))

    engine.advance(ctx)

    assert ran == [("evaluate", True), ("pr", None)]


def test_CurrentSteps_ReviewTurnedOnMidBuild_ReachesTheReviewsStillToCome(repo):
    ctx = _lane(repo, phase="build")
    assert engine.current_steps(ctx).final_review is False

    _turn(ctx, "final_review", "on")

    assert engine.current_steps(ctx).final_review is True


def _states(facts):
    return {step["id"]: (step["state"], step["can_toggle"]) for step in facts["pipeline"]}


def _facts(ctx):
    return lane_facts.lane_facts(ctx.store.root, ctx.store.read_state(), _lane_values(ctx))


def test_LaneFacts_AtBuild_ListsEveryStepWithItsStateAndWhetherItCanBeToggled(repo):
    ctx = _lane(repo, phase="build")

    facts = _facts(ctx)

    assert [step["id"] for step in facts["pipeline"]] == [
        "plan", "plan_review", "build", "task_review", "final_review", "checks", "evaluator", "deliver", "reporter"]
    assert _states(facts) == {
        "plan": ("done", False), "plan_review": ("done", False), "build": ("running", False),
        "task_review": ("next", True), "final_review": ("off", True), "checks": ("next", True),
        "evaluator": ("off", True), "deliver": ("next", False), "reporter": ("next", True)}


def test_LaneFacts_ACannotToggleStep_SaysWhy(repo):
    facts = _facts(_lane(repo, phase="build"))

    reasons = {step["id"]: step["reason"] for step in facts["pipeline"]}
    assert "already done" in reasons["plan_review"] and "always on" in reasons["build"]
    assert reasons["checks"] == ""


def test_LaneFacts_AfterTurningAStepOnAndOff_TheSnapshotFollows(repo):
    ctx = _lane(repo, phase="plan")

    _turn(ctx, "final_review", "on")
    _turn(ctx, "task_review", "off")

    states = _states(_facts(ctx))
    assert states["final_review"][0] == "next" and states["task_review"][0] == "off"


def test_LaneFacts_ATaskReviewThatAlreadyRan_IsRunningAndCannotBeToggled(repo):
    ctx = _lane(repo, phase="build", runs={"review-1": {"role": "code_reviewer", "step": "task_review", "state": "finished"}})

    assert _states(_facts(ctx))["task_review"] == ("running", False)


def test_Action_ToggleStep_BuildsTheFlowCommandAndIsLocalOnly():
    _, spawns = invoke("t", flow_item("/work/repo", "t1"), {"step": "checks", "state": "on"})

    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["flow", "step", "--", "t1", "checks", "on"]
    assert "t" in actions.LOCAL_ONLY_KEYS


def test_LaneFacts_AStepThatRan_IsOnForTheLaneWhateverThePresetSaysNow(repo):
    runs = {"plan-1": {"role": "planner", "step": "plan", "state": "finished"},
            "plan_review-1": {"role": "plan_reviewer", "step": "plan_review", "state": "finished"},
            "build-1": {"role": "builder", "step": "build", "state": "finished"},
            "review-1": {"role": "code_reviewer", "step": "task_review", "state": "finished"}}
    ctx = _lane(repo, overrides={"preset": "lean", "relax": ["preset"]}, phase="cleanup", runs=runs)
    assert pipeline.of_values(_lane_values(ctx)).review("plan_review").on is False  # the preset now has no plan review

    facts = {step["id"]: step for step in _facts(ctx)["pipeline"]}

    assert (facts["plan_review"]["state"], facts["plan_review"]["on"]) == ("done", True)
    assert (facts["task_review"]["state"], facts["task_review"]["on"]) == ("done", True)
    assert facts["plan_review"]["can_toggle"] is False and "already done" in facts["plan_review"]["reason"]
    assert (facts["checks"]["state"], facts["checks"]["on"]) == ("off", False)  # never ran, still off


def test_LaneFacts_ADeliveredLaneWithNoPlanReviewRun_ShowsItOff(repo):
    ctx = _lane(repo, overrides={"preset": "lean", "relax": ["preset"]}, phase="cleanup", runs={})

    assert {step["id"]: step["state"] for step in _facts(ctx)["pipeline"]}["plan_review"] == "off"


# -- the lane's current step and a step that waits: one source for the header and the step bar -------------------------

def _final_review_running():
    return {"plan-1": {"role": "planner", "step": "plan", "state": "finished"},
            "build-1": {"role": "builder", "step": "build", "state": "finished"},
            "review-1": {"role": "code_reviewer", "step": "task_review", "state": "finished"},
            "review-2": {"role": "code_reviewer", "step": "final_review", "state": "running"}}


def test_LaneFacts_TheCurrentStep_IsTheOneTheStepBarShows_NotTheEnginesCoarserPhase(repo):
    ctx = _lane(repo, overrides={"preset": "lean", "relax": ["preset"]}, phase="build", runs=_final_review_running(),
                final_review={"asked_rounds": 0})
    facts = _facts(ctx)

    assert facts["current_step"] == {"id": "final_review", "name": "Final review", "waiting": False, "note": ""}


def test_LaneFacts_ALaneBlockedAtThePullRequest_SaysDeliverWaitsInWords(repo):
    ctx = _lane(repo, phase="pr", status="blocked", reason="GitHub CLI is not logged in", runs={})

    facts = _facts(ctx)

    deliver = next(step for step in facts["pipeline"] if step["id"] == "deliver")
    assert deliver["state"] == "waiting" and deliver["note"] == "Waiting here: the lane needs you before it goes on."
    assert facts["current_step"] == {"id": "deliver", "name": "Deliver", "waiting": True, "note": deliver["note"]}
    assert all(step["state"] != "waiting" for step in facts["pipeline"] if step["id"] != "deliver")


def test_LaneFacts_AFinalReviewThatFinishedOnALaneAtDelivery_IsDoneNotRunning(repo):
    runs = {**_final_review_running(), "review-2": {"role": "code_reviewer", "step": "final_review", "state": "finished"}}
    ctx = _lane(repo, overrides={"preset": "lean", "relax": ["preset"]}, phase="pr", status="running", runs=runs,
                final_review={"asked_rounds": 0})

    facts = _facts(ctx)

    assert {step["id"]: step["state"] for step in facts["pipeline"]}["final_review"] == "done"
    assert facts["current_step"]["id"] == "deliver"


def test_LaneFacts_ARunningLane_HasNoWaitingStepAndAnEmptyNote(repo):
    facts = _facts(_lane(repo, phase="build", status="running"))
    assert all(step["state"] != "waiting" and step["note"] == "" for step in facts["pipeline"])
