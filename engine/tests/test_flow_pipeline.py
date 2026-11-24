import dataclasses
import tomllib

import pytest

from sakura.flow import config, pipeline, presets


def _of(name):
    return pipeline.of_knobs(presets.resolve(name).knobs())


def _on(built):
    return {item.id: item.on for item in (*built.steps, *built.reviews)}


def test_Pipeline_Standard_PlanAndTaskReviewOnFinalReviewOffChecksOn():
    on = _on(_of("standard"))
    assert (on["plan"], on["plan_review"], on["task_review"], on["final_review"], on["checks"]) == (
        True, True, True, False, True)


def test_Pipeline_Lean_OnlyTheFinalReviewRuns():
    on = _on(_of("lean"))
    assert (on["plan"], on["plan_review"], on["task_review"], on["final_review"], on["checks"]) == (
        True, False, False, True, False)


def test_Pipeline_Solo_IsBuildAndDeliverOnly():
    on = _on(_of("solo"))
    assert [step for step, value in on.items() if value] == ["build", "deliver"]


def test_Pipeline_PlanOff_TurnsThePlanReviewOffWithIt():
    knobs = {**presets.resolve("standard").knobs(), "plan": False}
    built = pipeline.of_knobs(knobs)
    assert not built.step("plan").on and not built.review("plan_review").on and built.review("task_review").on


def test_Pipeline_Build_IsAlwaysOn():
    knobs = presets.resolve("solo").knobs()
    assert pipeline.of_knobs(knobs).step("build").on


def test_Pipeline_ReviewerOff_TurnsEveryReviewOff():
    knobs = {**presets.resolve("lean").knobs(), "reviewer": False}
    assert not any(review.on for review in pipeline.of_knobs(knobs).reviews)


def test_Pipeline_CarriesModelsAccountsAndRounds():
    knobs = {**presets.resolve("standard").knobs(), "fix_rounds": 4, "final_review_rounds": 3}
    roles = {"planner": "claude:opus", "builder": "claude:sonnet", "plan_reviewer": "codex", "code_reviewer": "codex",
             "evaluator": "claude:opus"}
    built = pipeline.of_knobs(knobs, roles, {"build": "work"})
    assert built.step("plan").model == "claude:opus" and built.step("build").account == "work"
    assert built.step("plan").account == pipeline.PROJECT_ACCOUNT
    assert built.review("plan_review").model == "codex"
    assert (built.review("task_review").rounds, built.review("final_review").rounds) == (4, 3)


def test_Pipeline_IsFrozen():
    built = _of("standard")
    with pytest.raises(dataclasses.FrozenInstanceError):
        built.steps = ()


def test_Pipeline_ToToml_RoundTripsThroughAPresetFile(tmp_path):
    original = _of("lean")
    path = tmp_path / "copy.toml"
    path.write_text('base = "solo"\n' + pipeline.to_toml(original))
    knobs = presets._apply(path, presets.resolve("solo"), tomllib.loads(path.read_text())).knobs()
    assert _on(pipeline.of_knobs(knobs)) == _on(original)


def test_Steps_FromTheKnobs_FollowThePipeline():
    values = config.load(None).values
    assert config.steps(values) == config.PresetSteps(True, True, False)
    values["knobs"].update(plan_review=False, final_review=True)
    steps = config.steps(values)
    assert (steps.plan_review, steps.code_review, steps.final_review) == (False, True, True)


def test_Steps_PlanOff_TurnsPlanReviewOffAndThePipelineSkipsThePlan():
    values = config.load(None).values
    values["knobs"]["plan"] = False
    assert config.steps(values).plan_review is False
    assert pipeline.of_values(values).step("plan").on is False


def test_Steps_FinalReviewWithRoleNone_IsOff():
    values = config.load(None).values
    values["knobs"]["final_review"] = True
    values["roles"]["code_reviewer"] = "none"
    assert not config.steps(values).final_review
