"""Model routing: the cheapest model that fits a task class, escalation and explicit-role precedence."""

import pytest

from sakura.budget import routing
from sakura.flow import config


def _cfg(tmp_path, **overrides):
    return config.load(tmp_path, home=tmp_path / "nohome", overrides=overrides or None)


def test_mechanical_task_routes_to_haiku(tmp_path):
    chosen = routing.route("builder", "mechanical", 0, _cfg(tmp_path))
    assert (chosen.model, chosen.needs_approval) == ("haiku", False)
    assert "mechanical" in chosen.reason


def test_design_task_routes_to_sonnet_high_effort(tmp_path):
    chosen = routing.route("builder", "design", 0, _cfg(tmp_path))
    assert (chosen.model, chosen.effort, chosen.needs_approval) == ("sonnet", "high", False)


def test_integration_task_routes_to_sonnet_at_the_dial_effort(tmp_path):
    chosen = routing.route("builder", "integration", 0, _cfg(tmp_path))
    assert (chosen.model, chosen.effort) == ("sonnet", "")


def test_two_failures_step_up_once(tmp_path):
    cfg = _cfg(tmp_path)
    assert routing.route("builder", "mechanical", 1, cfg).model == "haiku"
    for failures in (2, 3):
        stepped = routing.route("builder", "mechanical", failures, cfg)
        assert (stepped.model, stepped.needs_approval) == ("sonnet", False)
        assert "2" in stepped.reason or "3" in stepped.reason


def test_opus_for_a_builder_needs_approval(tmp_path):
    cfg = _cfg(tmp_path)
    escalated = routing.route("builder", "integration", 2, cfg)
    assert (escalated.model, escalated.needs_approval) == ("opus", True)
    assert routing.route("builder", "mechanical", 4, cfg).needs_approval is True
    assert routing.route("builder", "mechanical", 40, cfg).model == "opus"


@pytest.mark.parametrize("role", ["planner", "evaluator"])
def test_planner_and_evaluator_keep_their_role(tmp_path, role):
    chosen = routing.route(role, None, 5, _cfg(tmp_path))
    assert (chosen.model, chosen.effort, chosen.needs_approval) == ("", "", False)


@pytest.mark.parametrize("role,key,model", [("plan_reviewer", "reviewer", "sonnet"), ("code_reviewer", "reviewer", "sonnet"),
                                            ("reporter", "reporter", "sonnet"), ("intake", "intake", "haiku")])
def test_reviewers_reporter_and_intake_follow_their_routes(tmp_path, role, key, model):
    chosen = routing.route(role, None, 0, _cfg(tmp_path))
    assert (chosen.model, chosen.key) == (model, key)


def test_old_plan_without_class_is_integration(tmp_path):
    chosen = routing.route("builder", None, 0, _cfg(tmp_path))
    assert (chosen.model, chosen.key) == ("sonnet", "integration")


def test_unknown_task_class_is_rejected(tmp_path):
    with pytest.raises(ValueError, match="trivial"):
        routing.route("builder", "trivial", 0, _cfg(tmp_path))


def test_explicitly_configured_builder_model_is_kept(tmp_path):
    cfg = _cfg(tmp_path, roles={"builder": "claude:sonnet"})
    chosen = routing.route("builder", "mechanical", 0, cfg)
    assert (chosen.model, chosen.key) == ("", "")
    assert routing.route("builder", "mechanical", 9, cfg).needs_approval is False


def test_explicit_planner_setting_wins_over_the_intake_route(tmp_path):
    cfg = _cfg(tmp_path, roles={"planner": "claude:sonnet"})
    assert routing.route("intake", None, 0, cfg).model == ""


def test_codex_roles_are_never_rerouted(tmp_path):
    cfg = _cfg(tmp_path, roles={"code_reviewer": "codex"})
    assert routing.route("code_reviewer", "mechanical", 0, cfg).model == ""


def test_route_table_can_be_changed_by_a_config_layer(tmp_path):
    cfg = _cfg(tmp_path, budget={"routes": {"mechanical": "sonnet:low"}})
    chosen = routing.route("builder", "mechanical", 0, cfg)
    assert (chosen.model, chosen.effort) == ("sonnet", "low")


@pytest.mark.parametrize("bad", ["gpt-5", "sonnet:turbo", "opus", "", "sonnet:high:x"])
def test_invalid_route_values_fail_config_validation(tmp_path, bad):
    with pytest.raises(config.FlowConfigError, match="budget.routes.design"):
        _cfg(tmp_path, budget={"routes": {"design": bad}})


# -- the dial caps the route (Ruling 26): a routed model is the cheaper of the two ---------------------------------

@pytest.mark.parametrize("dial, expected", [("save", "haiku"), ("lean", "sonnet"), ("balanced", "sonnet"),
                                            ("strong", "sonnet"), ("power", "sonnet")])
def test_an_integration_builder_never_costs_more_than_the_dial_or_the_route(tmp_path, dial, expected):
    chosen = routing.route("builder", "integration", 0, _cfg(tmp_path, dial=dial))
    assert (chosen.model, chosen.needs_approval) == (expected, False)


@pytest.mark.parametrize("dial", ["save", "lean", "balanced", "strong", "power"])
def test_a_mechanical_builder_stays_on_haiku_under_every_dial(tmp_path, dial):
    assert routing.route("builder", "mechanical", 0, _cfg(tmp_path, dial=dial)).model == "haiku"


def test_the_save_dial_puts_reviewers_and_the_reporter_on_haiku(tmp_path):
    cfg = _cfg(tmp_path, dial="save")
    assert [routing.route(role, None, 0, cfg).model for role in ("code_reviewer", "reporter", "intake")] == ["haiku"] * 3


def test_a_capped_route_says_so_and_escalation_still_steps_up_from_the_cap(tmp_path):
    cfg = _cfg(tmp_path, dial="save")
    assert "dial save" in routing.route("builder", "integration", 0, cfg).reason
    stepped = routing.route("builder", "integration", 2, cfg)
    assert (stepped.model, stepped.needs_approval) == ("sonnet", False)
    top = routing.route("builder", "integration", 4, cfg)
    assert (top.model, top.needs_approval) == ("opus", True)
