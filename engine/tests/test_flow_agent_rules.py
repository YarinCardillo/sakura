"""A step's agent is chosen as on Master: the account, then the model of that account's program, then the effort the model takes.
Every account is offered on every step."""

import json
import tomllib
from pathlib import Path

import pytest

from sakura.accounts.registry import Account
from sakura.flow import config, model_catalog, rules

FIXTURES = Path(__file__).parent / "fixtures" / "models"
CODEX_LIST = json.loads((FIXTURES / "codex-model-list.json").read_text())
CLAUDE_LIST = json.loads((FIXTURES / "claude-initialize.json").read_text())


def _account(provider, identifier, label):
    return Account(provider, identifier, label, Path("/nonexistent") / identifier, True, None, (), ())


ACCOUNTS = [_account("claude", "claude-default", "Claude"), _account("claude", "claude-work", "Claude Work"),
            _account("codex", "codex", "Codex")]


@pytest.fixture
def models(tmp_path):
    """Every account's models kept, as `sakura engine models refresh` leaves them."""
    def exchange(argv, env, requests, is_answer):
        if argv[1] == "app-server":
            return [{"id": 2, "result": CODEX_LIST}]
        return [{"type": "control_response", "response": {"response": {"models": CLAUDE_LIST["models"]}}}]

    for account in ACCOUNTS:
        model_catalog.refresh(account, exchange=exchange)


def _rules(**options):
    described = rules.describe(None, accounts=ACCOUNTS, **options)
    return described, {rule["id"]: rule for node in described["nodes"] for rule in node["rules"]}


# -- reading

def test_Describe_EveryAccountIsOneFlatEntryUnderItsNameWithItsModelsAndEfforts(models):
    described, _ = _rules()

    agents = {agent["id"]: agent for agent in described["agents"]}

    assert [(agent["id"], agent["label"], agent["provider"]) for agent in described["agents"]] == [
        ("claude-default", "Claude", "claude"), ("claude-work", "Claude Work", "claude"), ("codex", "Codex", "codex")]
    sol = [model for model in agents["codex"]["models"] if model["id"] == "gpt-6.2-sol"][0]
    assert (sol["label"], sol["default_effort"], sol["is_default"]) == ("GPT-6.2-Sol", "low", True)
    assert sol["efforts"] == ["low", "medium", "high", "xhigh", "max", "ultra"]
    sonnet = [model for model in agents["claude-default"]["models"] if model["id"] == "sonnet"][0]
    assert sonnet["resolved"] == "claude-sonnet-5-5" and "max" in sonnet["efforts"]
    assert agents["codex"]["fetched_at"] and agents["codex"]["source"] == "Codex app-server model/list"


def test_Describe_CodexAccountsAreOfferedOnTheReviewStepsOnly(models):
    _, found = _rules()

    review = found["reviews.plan_review.model"]["agent"]["accounts"]
    build = found["steps.build.model"]["agent"]["accounts"]
    plan = found["steps.plan.model"]["agent"]["accounts"]

    assert [account["id"] for account in review] == ["claude-default", "claude-work", "codex"]
    assert [account["id"] for account in build] == ["claude-default", "claude-work", "codex"]
    assert [account["id"] for account in plan] == ["claude-default", "claude-work", "codex"]
    assert "codex:gpt-6.2-sol" in found["reviews.plan_review.model"]["choices"]
    assert "codex:gpt-6.2-sol" in found["steps.build.model"]["choices"]


def test_Describe_AStepShowsItsAccountItsModelAndItsEffort_TheDefaultBeingThePresetsTierModel(models):
    _, found = _rules()

    agent = found["reviews.plan_review.model"]["agent"]

    assert (agent["account"], agent["provider"], agent["model"]) == ("project", "claude", "sonnet")
    assert agent["effort"] == config.DIAL["balanced"].effort and agent["effort_is_set"] is False
    assert agent["default_model"] == "sonnet" and agent["resolved"] == "claude-sonnet-5-5"


def test_Describe_AStepSetToACodexModel_ShowsItsEffortsAndItsOwnEffortWhenSet(models):
    rules.set("reviews.plan_review.model", "codex:gpt-6.2-sol", None, accounts=ACCOUNTS)
    rules.set("reviews.plan_review.effort", "high", None, accounts=ACCOUNTS)

    _, found = _rules()

    agent = found["reviews.plan_review.model"]["agent"]
    assert (agent["provider"], agent["model"], agent["effort"], agent["effort_is_set"]) == ("codex", "gpt-6.2-sol", "high", True)
    assert found["reviews.plan_review.effort"]["choices"] == ["low", "medium", "high", "xhigh", "max", "ultra"]
    assert found["reviews.plan_review.effort"]["value"] == "high"


def test_Describe_WithNoKeptModels_TheAccountsAreThereWithNoModelsAndTheOldChoicesStay():
    described, found = _rules()

    assert all(agent["models"] == [] and agent["fetched_at"] == "" for agent in described["agents"])
    assert found["steps.build.model"]["choices"] == ["claude:haiku", "claude:sonnet", "claude:opus", "codex"]
    assert found["reviews.plan_review.model"]["choices"][-1] == "codex"


# -- changing

def test_Set_APlanReviewOnACodexModelAtAnEffort_IsTheRolesValueAndTheStepsEffort(models):
    rules.set("reviews.plan_review.model", "codex:gpt-6.2-sol", None, accounts=ACCOUNTS)
    rules.set("reviews.plan_review.effort", "high", None, accounts=ACCOUNTS)

    values = config.load(None).values

    assert values["roles"]["plan_reviewer"] == "codex:gpt-6.2-sol" and values["efforts"]["plan_reviewer"] == "high"


def test_Set_ABuildStepTakesACodexAccountAndACodexModel(models):
    rules.set("steps.build.account", "codex", None, accounts=ACCOUNTS)
    assert config.load(None).values["roles"]["builder"] == "codex"
    rules.set("steps.build.model", "codex:gpt-6.2-sol", None, accounts=ACCOUNTS)
    assert config.load(None).values["roles"]["builder"] == "codex:gpt-6.2-sol"


@pytest.mark.parametrize("node", ["plan", "plan_review", "build", "task_review", "checks"])
def test_Describe_EveryStepOffersEveryAccount(models, node):
    _, found = _rules()
    key = {"plan": "steps.plan.model", "plan_review": "reviews.plan_review.model", "build": "steps.build.model",
           "task_review": "reviews.task_review.model", "checks": "steps.checks.model"}[node]
    assert [account["id"] for account in found[key]["agent"]["accounts"]] == ["claude-default", "claude-work", "codex"]


def test_Set_AReviewStepOnACodexAccount_TakesTheProgramAndAReviewStepBackOnAClaudeAccountTakesItsTierModel(models):
    rules.set("reviews.plan_review.account", "codex", None, accounts=ACCOUNTS)
    assert config.load(None).values["roles"]["plan_reviewer"] == "codex"

    rules.set("reviews.plan_review.account", "claude-work", None, accounts=ACCOUNTS)

    values = config.load(None).values
    assert values["roles"]["plan_reviewer"] == "claude:sonnet", "the preset's model of the tier"
    assert values["step_accounts"]["plan_review"] == "claude-work" and values["efforts"]["plan_reviewer"] == ""


def test_Set_AModelTheAccountsProgramDoesNotList_IsRefusedWithWhatItListsAndKeepsNothing(models):
    with pytest.raises(rules.RuleRefused, match="gpt-6.2-sol"):
        rules.set("reviews.plan_review.model", "codex:gpt-9", None, accounts=ACCOUNTS)

    assert config.load(None).values["roles"]["plan_reviewer"] == "claude:sonnet" and rules.is_edited() is False


def test_Set_AnEffortTheModelDoesNotTake_IsRefusedWithTheOnesItTakes(models):
    rules.set("reviews.plan_review.model", "claude:sonnet", None, accounts=ACCOUNTS)

    with pytest.raises(rules.RuleRefused, match="low, medium, high, xhigh, max"):
        rules.set("reviews.plan_review.effort", "ultra", None, accounts=ACCOUNTS)


def test_Set_AModelOfTheOtherProgramThanTheStepsAccount_IsRefusedUntilTheAccountChanges(models):
    rules.set("reviews.plan_review.account", "codex", None, accounts=ACCOUNTS)

    with pytest.raises(rules.RuleRefused, match="Codex"):
        rules.set("reviews.plan_review.model", "claude:opus", None, accounts=ACCOUNTS)


def test_Set_AClaudeModelByItsFullName_IsAcceptedWhenClaudeCodeListsIt(models):
    rules.set("steps.build.model", "claude:claude-sonnet-5-5", None, accounts=ACCOUNTS)

    assert config.load(None).values["roles"]["builder"] == "claude:claude-sonnet-5-5"


def test_Set_TheOldModelValuesStillWorkWithNoKeptList():
    rules.set("steps.build.model", "claude:haiku", None, accounts=ACCOUNTS)

    assert config.load(None).values["roles"]["builder"] == "claude:haiku"


# -- back to the preset

def test_Clear_AStepSetToASpecificModelIsAnEdit_AndUsingThePresetAgainDropsIt(models):
    from sakura.flow import preset_cli

    rules.set("reviews.plan_review.model", "codex:gpt-6.2-sol", None, accounts=ACCOUNTS)
    rules.set("reviews.plan_review.effort", "high", None, accounts=ACCOUNTS)
    assert rules.is_edited() is True

    assert preset_cli.main(["use", "standard"]) == 0

    values = config.load(None).values
    assert rules.is_edited() is False
    assert values["roles"]["plan_reviewer"] == "claude:sonnet" and values["efforts"]["plan_reviewer"] == ""


def test_Clear_RulesClearDropsAStepsModelAndEffortAtOnce(models):
    rules.set("steps.plan.model", "claude:opus", None, accounts=ACCOUNTS)
    rules.set("steps.plan.effort", "max", None, accounts=ACCOUNTS)
    assert rules.is_edited() is True

    rules.clear(None)

    values = config.load(None).values
    assert rules.is_edited() is False and values["roles"]["planner"] == "claude:opus"  # the standard preset's own tier model
    assert values["efforts"]["planner"] == ""


def test_Clear_TheAppsActionForItIsUsePresetWithTheCurrentPreset(models):
    from sakura.cockpit import host_actions
    from test_cockpit_actions import invoke

    rules.set("reviews.plan_review.model", "codex:gpt-6.2-sol", None, accounts=ACCOUNTS)
    current = rules.describe(None, accounts=ACCOUNTS)["preset"]

    _, spawns = invoke("U", host_actions.ENGINE, {"preset": current})

    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["preset", "use", "--", current]


def test_Describe_AStepWhoseProviderHasNoAccount_SaysWhichAccountItRunsOnInstead():
    described = rules.describe(None, accounts=[_account("codex", "codex", "Codex")])
    found = {rule["id"]: rule for node in described["nodes"] for rule in node["rules"]}
    assert found["steps.build.model"]["agent"]["note"] == "No Claude account exists, so the builder runs on the Codex account (Codex)."


def test_Describe_AStepWhoseProviderHasAnAccount_HasNoNote(models):
    _, found = _rules()
    assert found["steps.build.model"]["agent"]["note"] == ""


def test_Set_AStepOfAPresetThatNamesCodexModels_MovedToAClaudeAccountTakesClaudeNotThePresetsCodexModel(models):
    from sakura import engine_home
    from sakura.flow import presets
    folder = engine_home.root() / "presets"
    folder.mkdir(exist_ok=True)
    (folder / "allcodex.toml").write_text('base = "standard"\n[models]\nlead = "codex:gpt-6.2-sol"\nworker = "codex:gpt-6.2-sol"\nreview = "codex:gpt-6.2-sol"\n')
    presets.choose("allcodex")
    assert config.load(None).values["roles"]["builder"] == "codex:gpt-6.2-sol"

    rules.set("steps.build.account", "claude-default", None, accounts=ACCOUNTS)

    assert config.load(None).values["roles"]["builder"] == "claude"


# -- a rule set back to what the preset gives is no edit

def _rules_file():
    from sakura import engine_home
    return engine_home.root() / config.RULES_FILE


def test_Set_ABuildModelTurnedAndTurnedBackToThePresetsOwn_IsNoEditAndLeavesNoRulesFile(models):
    own = config.load(None).values["roles"]["builder"]
    rules.set("steps.build.model", "claude:opus", None, accounts=ACCOUNTS)
    assert rules.is_edited() is True and _rules_file().exists()

    done = rules.set("steps.build.model", own, None, accounts=ACCOUNTS)

    assert done["edited"] is False and rules.is_edited() is False and not _rules_file().exists()
    assert config.load(None).values["roles"]["builder"] == own


def test_Set_BackToThePresetsValue_DropsOnlyThatKeyAndItsRelaxEntry(models):
    own = config.load(None).values["roles"]["builder"]
    rules.set("steps.build.model", "claude:opus", None, accounts=ACCOUNTS)
    rules.set("subagents", "4", None)
    rules.set("steps.build.model", own, None, accounts=ACCOUNTS)

    layer = tomllib.loads(_rules_file().read_text())
    assert "roles" not in layer and layer["knobs"]["subagents"] == 4
    assert "roles.builder" not in layer["relax"] and rules.is_edited() is True


def test_Set_AnEffortGivenBackToTheDialsOwn_AndAnAccountBackToTheProject_AreNoEdit(models):
    rules.set("steps.build.effort", "high", None, accounts=ACCOUNTS)
    rules.set("steps.build.account", "claude-work", None, accounts=ACCOUNTS)
    assert rules.is_edited() is True

    rules.set("steps.build.effort", "default", None, accounts=ACCOUNTS)
    rules.set("steps.build.account", "project", None, accounts=ACCOUNTS)

    assert rules.is_edited() is False and not _rules_file().exists()


def test_Set_AValueThatDiffersFromThePreset_StaysAnEdit(models):
    rules.set("steps.build.model", "claude:opus", None, accounts=ACCOUNTS)
    assert tomllib.loads(_rules_file().read_text())["roles"]["builder"] == "claude:opus"
