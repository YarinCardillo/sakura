from sakura.quota import models
from sakura.quota.config import QuotaConfig


def test_Choose_BuilderByTaskClass_FollowsTheRoutingTable():
    cfg = QuotaConfig()
    assert models.choose("claude", "builder", "mechanical", cfg) == models.Choice("haiku", "")
    assert models.choose("claude", "builder", "integration", cfg) == models.Choice("sonnet", "")
    assert models.choose("claude", "builder", "design", cfg) == models.Choice("sonnet", "high")


def test_Choose_ReviewerAndExplorer_NeverReachOpus():
    cfg = QuotaConfig()
    assert models.choose("claude", "reviewer", "design", cfg).model == "sonnet"
    assert models.choose("claude", "explorer", "mechanical", cfg).model == "haiku"
    assert models.choose("claude", "explorer", "design", cfg).model == "sonnet"


def test_Choose_PlannerAndEvaluator_GetTheStrongestModel():
    for role in ("planner", "evaluator"):
        assert models.choose("claude", role, "integration", QuotaConfig()).model == "opus"


def test_Choose_RoleOverrideInConfig_ReplacesTheTier():
    cfg = QuotaConfig(roles={"planner": "sonnet:high"})
    assert models.choose("claude", "planner", "design", cfg) == models.Choice("sonnet", "high")


def test_Choose_Codex_KeepsItsConfiguredModelUnlessSet():
    assert models.choose("codex", "builder", "design", QuotaConfig()) == models.Choice("", "high")
    assert models.choose("codex", "reviewer", "design", QuotaConfig(codex_model="x")).model == "x"
    assert models.choose("codex", "explorer", "mechanical", QuotaConfig()).effort == "medium"
