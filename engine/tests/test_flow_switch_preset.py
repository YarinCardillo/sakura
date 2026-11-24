"""Change a running lane's preset on the fly: queued like a dial, applied at the next step, recorded with the actor."""

import pytest

from sakura.cockpit import actions, api, lane_facts
from sakura import engine_home
from sakura.flow import config, engine, exits, inbox, presets, runner, step_accounts, store
from sakura.flow.cli import main as flow_main
from sakura.flow.phases import build

from flow_support import make_context
from test_cockpit_actions import flow_item, invoke
from test_flow_build import APPROVE, _commit, _ctx, _task, _use, no_hook_install  # noqa: F401  (the fixture is autouse)

ALICE = "cli:alice"


def _lane(repo, task="a", **state):
    return make_context(repo, task=task, state=state or None)


def _switch(ctx, preset, actor=ALICE):
    """What the runner or the engine does at a boundary: take the queued command in."""
    inbox.enqueue(ctx.store, "preset", preset, actor=actor)
    return inbox.consume(ctx.store, ctx.repo, "build")


def test_Validate_UnknownPreset_IsRefusedBeforeItIsQueued(repo):
    cfg = config.load(repo)

    with pytest.raises(inbox.TaskStartError, match="unknown preset"):
        inbox.validate(cfg, "preset", "careless", "", repo)


def test_Consume_Preset_IsKeptInTheLanesOverridesAndLedgeredWithTheActor(repo):
    ctx = _lane(repo)

    assert _switch(ctx, "lean") == 1

    overrides = ctx.store.read_state()["overrides"]
    assert overrides["preset"] == "lean" and "preset" in overrides["relax"]
    assert "preset changed to lean" in ctx.store.ledger.read_text()
    assert f"actor={ALICE}" in ctx.store.ledger.read_text()


def test_Consume_Preset_KeepsTheLanesOwnPaceAndDial(repo):
    ctx = _lane(repo)
    inbox.enqueue(ctx.store, "pace", "slow", actor=ALICE)
    inbox.consume(ctx.store, repo, "build")

    _switch(ctx, "lean")

    assert ctx.store.read_state()["overrides"]["knobs"]["lane_pace"] == "slow"


def test_ApplyInbox_AfterTheRunnerAlreadyTookThePresetIn_StillRebuildsTheSteps(repo):
    ctx = _lane(repo)
    _switch(ctx, "lean")

    rebuilt = engine.apply_inbox(ctx, "evaluate")

    assert rebuilt.config.values["preset"] == "lean" and rebuilt.steps.final_review is True
    assert rebuilt.steps.code_review is False


def test_Cli_FlowPreset_QueuesThePresetForTheEngineToApply(repo, monkeypatch):
    monkeypatch.chdir(repo)
    ctx = _lane(repo)

    assert flow_main(["preset", "--", "a", "lean"]) == exits.OK
    inbox.consume(ctx.store, repo, "build")

    assert ctx.store.read_state()["overrides"]["preset"] == "lean"


def test_Cli_FlowPreset_UnknownPreset_IsRefused(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    _lane(repo)

    assert flow_main(["preset", "--", "a", "careless"]) == exits.USAGE
    assert "unknown preset" in capsys.readouterr().out


def test_Build_SwitchedToLeanAfterTheFirstTaskReview_SkipsTheRestOfTheTaskReviewsAndRunsTheFinalReview(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2", ["T1"])], full=None)
    double = _use(ctx, _commit, [APPROVE, APPROVE])
    switched = []

    def switch_after_first_review(request):
        if not switched:
            switched.append(_switch(ctx, "lean"))

    double.on_review = switch_after_first_review

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    task_reviews = [r for r in double.of("code_reviewer") if r.template == runner.load_template("review")]
    assert [r.values["task_id"] for r in task_reviews] == ["T1"]
    assert len([r for r in double.of("code_reviewer") if r.template == runner.load_template("final_review")]) == 1


def test_Build_SwitchBackToStandard_ReviewsTheTasksNotYetBuilt(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2", ["T1"])])
    _switch(ctx, "lean")
    double = _use(ctx, _commit, [APPROVE, APPROVE])
    switched = []

    def switch_back_when_the_second_task_starts(request):
        if request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T2" and not switched:
            switched.append(_switch(ctx, "standard"))
        _commit(request)

    double.builder = switch_back_when_the_second_task_starts

    assert build.run(ctx).kind == "next"
    reviewed = [r.values["task_id"] for r in double.of("code_reviewer") if r.template == runner.load_template("review")]
    assert reviewed == ["T2"]


def test_LaneFacts_AfterTheSwitch_TheSnapshotShowsTheNewPresetAndKeepsTheFinishedPlanReview(repo):
    ctx = _lane(repo, phase="build", runs={"plan-review-1": {"role": "plan_reviewer", "phase": "plan_review",
                                                              "state": "finished"}})
    _switch(ctx, "lean")
    cfg = config.load(repo, overrides=ctx.store.read_state()["overrides"])

    facts = lane_facts.lane_facts(ctx.store.root, ctx.store.read_state(), cfg.values)

    assert facts["preset"] == "lean"
    reviews = {review["id"]: (review["on"], review["state"]) for review in facts["reviews"]}
    assert reviews == {"plan_review": (True, "done"), "task_review": (False, "skipped"), "final_review": (True, "next")}


def test_Action_SwitchPreset_BuildsTheFlowCommandAndIsLocalOnly():
    _, spawns = invoke("S", flow_item("/work/repo", "t1"), {"preset": "lean"})

    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["flow", "preset", "--", "t1", "lean"]
    assert "S" in actions.LOCAL_ONLY_KEYS


def test_Consume_Preset_PinningAStepsAccount_DropsTheAccountPaceMovedThatStepTo(repo):
    folder = engine_home.root() / presets.PRESETS_DIR
    folder.mkdir(exist_ok=True)
    (folder / "pinned.toml").write_text('base = "standard"\n[accounts]\nbuild = "new"\n')
    ctx = _lane(repo)
    ctx.store.update_state(lambda state: state.setdefault("budget", {}).update(pace_account={"build": "old"}))

    _switch(ctx, "pinned")

    cfg = config.load(repo, overrides=ctx.store.read_state()["overrides"])
    account = step_accounts.account_of(cfg.values, ctx.store.read_state(), "builder")
    assert account == "new"
