"""A step runs with exactly the model and effort chosen for it: Claude with --model and --effort, Codex with -m and
model_reasoning_effort; the step's own effort wins over the dial's and the route's."""

import pytest

from sakura import repository
from sakura.budget import routing
from sakura.flow import config, roles
from sakura.flow.runner import AgentRunner as Runner

from test_flow_roles import _cage, _trusted_repo


def _cfg(**efforts):
    return config.load(None, overrides={"efforts": efforts} if efforts else None)


def _route(effort=""):
    return routing.Route("sonnet", effort, "why", False, "key")


def _argv(spec_text, role, repo, dial, **kwargs):
    return roles.argv_for(roles.parse_role(spec_text), role, dial, repo, repository.common_dir(repo), repo / "last.txt",
                          cage=_cage(repo, role), **kwargs)


def _after(argv, flag):
    return argv[argv.index(flag) + 1]


def test_PlanReviewOnCodexGpt62SolHigh_BuildsAnArgvWithThoseValues(repo):
    _trusted_repo(repo)
    cfg = _cfg(plan_reviewer="high")
    dial = Runner._dial_for(cfg, _route(), "plan_reviewer")

    argv = _argv("codex:gpt-6.2-sol", "plan_reviewer", repo, dial)

    assert _after(argv, "-m") == "gpt-6.2-sol"
    assert "model_reasoning_effort=high" in argv


def test_AClaudeStepWithItsOwnEffort_RunsWithThatModelAndEffort(repo):
    cfg = _cfg(planner="max")
    dial = Runner._dial_for(cfg, _route(), "planner")

    argv = _argv("claude:claude-sonnet-5-5", "planner", repo, dial)

    assert _after(argv, "--model") == "claude-sonnet-5-5" and _after(argv, "--effort") == "max"


def test_AStepWithNoEffortOfItsOwn_KeepsTheDialsOrTheRoutes(repo):
    cfg = _cfg()

    assert Runner._dial_for(cfg, _route(), "builder").effort == config.DIAL["balanced"].effort
    assert Runner._dial_for(cfg, _route("high"), "builder").effort == "high"


def test_TheStepsOwnEffort_WinsOverTheRoutes(repo):
    cfg = _cfg(builder="low")

    assert Runner._dial_for(cfg, _route("high"), "builder").effort == "low"
    assert Runner._dial_for(cfg, _route("high"), "planner").effort == "high", "another role keeps its own"


@pytest.mark.parametrize("role", ["planner", "builder", "evaluator", "reporter"])
def test_ACodexModelOnAStepThatIsNotAReview_RunsOnTheWorkProfile(repo, role):
    _trusted_repo(repo)

    argv = _argv("codex:gpt-6.2-sol", role, repo, config.DIAL["balanced"])

    assert argv[argv.index("-m") + 1] == "gpt-6.2-sol" and 'default_permissions="sakura-work"' in argv
