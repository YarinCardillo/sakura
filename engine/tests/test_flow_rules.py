import json

import pytest

from sakura.flow import brakes, config, exits, pipeline, preset_cli, presets, rules, rules_cli


def _by_id(described):
    return {rule["id"]: rule for node in described["nodes"] for rule in node["rules"]}


def _nodes(described):
    return [node["node"] for node in described["nodes"]]


def test_Describe_Nodes_FollowTheLaneFlow():
    assert _nodes(rules.describe(None)) == ["start", "plan", "plan_review", "build", "task_review", "final_review",
                                            "checks", "deliver", "guard"]


def test_Describe_Standard_ShowsTheReviewsAndIsNotEdited():
    described = rules.describe(None)
    found = _by_id(described)
    assert described["preset"] == "standard" and described["edited"] is False
    assert found["reviews.plan_review.on"]["value"] is True
    assert found["reviews.final_review.on"]["value"] is False
    assert found["reviews.task_review.rounds"]["min"] == 0 and found["reviews.task_review.rounds"]["max"] == 6


def test_Describe_EveryRuleHasItsFields():
    for rule in _by_id(rules.describe(None)).values():
        assert {"id", "label", "type", "value", "choices", "min", "max", "origin", "read_only"} <= set(rule)


def test_Describe_Guard_IsReadOnly():
    guard = [node for node in rules.describe(None)["nodes"] if node["node"] == "guard"][0]
    assert guard["rules"] and all(rule["read_only"] for rule in guard["rules"])


def test_Describe_Build_IsAlwaysOnAndReadOnly():
    build_on = _by_id(rules.describe(None))["steps.build.on"]
    assert build_on["value"] is True and build_on["read_only"] is True


def test_Describe_OneAccount_ListsNoAccountRule():
    found = _by_id(rules.describe(None, account_ids=["personal"]))
    assert not [rule for rule in found if rule.endswith(".account")]


def test_Describe_SeveralAccounts_ListsAnAccountChoicePerStep():
    described = rules.describe(None, account_ids=["personal", "work"])
    found = _by_id(described)
    assert described["accounts_count"] == 2
    assert found["steps.build.account"]["choices"] == ["project", "personal", "work"]
    assert found["steps.build.account"]["value"] == "project"


def test_Set_PlanReviewOff_NextLaneSkipsItAndDescribeShowsEdited():
    rules.set("reviews.plan_review.on", "false", None)
    assert config.steps(config.load(None).values).plan_review is False
    described = rules.describe(None)
    assert described["edited"] is True and _by_id(described)["reviews.plan_review.on"]["origin"] == "rules"


def test_Set_PlanOff_TurnsPlanReviewOffAndKeepsTheTaskReview():
    rules.set("steps.plan.on", "false", None)
    values = config.load(None).values
    steps = config.steps(values)
    assert (pipeline.of_values(values).step("plan").on, steps.plan_review, steps.code_review) == (False, False, True)


def test_Set_BuildOff_IsRefusedBecauseBuildIsAlwaysOn():
    with pytest.raises(rules.RuleRefused, match="always on"):
        rules.set("steps.build.on", "false", None)
    assert rules.describe(None)["edited"] is False


def test_Set_LanesAboveTheMaximum_IsRefusedWithTheRange():
    with pytest.raises(rules.RuleRefused, match="from 1 to 6"):
        rules.set("lanes", "99", None)
    assert rules.describe(None)["edited"] is False


def test_Set_LanesMinAboveLanes_IsRefusedAndNothingIsKept():
    with pytest.raises(rules.RuleRefused, match="lanes_min"):
        rules.set("lanes_min", "5", None)
    assert rules.describe(None)["edited"] is False and config.load(None).values["knobs"]["lanes_min"] == 1


def test_Set_UnknownRule_IsRefused():
    with pytest.raises(rules.RuleRefused, match="unknown rule"):
        rules.set("reviews.nothing.on", "true", None)


def test_Set_ReadOnlyRule_IsRefused():
    with pytest.raises(rules.RuleRefused, match="read-only"):
        rules.set("guard.cage", "default", None)


def test_Set_WrongType_IsRefusedWithTheExpectedType():
    with pytest.raises(rules.RuleRefused, match="true or false"):
        rules.set("reviews.plan_review.on", "maybe", None)
    with pytest.raises(rules.RuleRefused, match="whole number"):
        rules.set("subagents", "two", None)


def test_Set_ModelOutsideTheChoices_IsRefused():
    with pytest.raises(rules.RuleRefused, match="claude:opus"):
        rules.set("steps.build.model", "gpt", None)
    rules.set("steps.build.model", "claude:haiku", None)
    assert config.load(None).values["roles"]["builder"] == "claude:haiku"


def test_Set_AccountWithOneAccount_IsRefused():
    with pytest.raises(rules.RuleRefused, match="more than one account"):
        rules.set("steps.build.account", "work", None, account_ids=["personal"])


def test_Set_Account_IsKeptForTheStep():
    rules.set("steps.build.account", "work", None, account_ids=["personal", "work"])
    assert config.load(None).values["step_accounts"]["build"] == "work"
    with pytest.raises(rules.RuleRefused, match="unknown account"):
        rules.set("steps.build.account", "other", None, account_ids=["personal", "work"])


def test_Set_ASecondEdit_KeepsTheFirst():
    rules.set("reviews.plan_review.on", "false", None)
    rules.set("subagents", "4", None)
    knobs = config.load(None).values["knobs"]
    assert (knobs["plan_review"], knobs["subagents"]) == (False, 4)


def test_Set_EnablingAReviewWhileReviewersAreOff_TurnsOnThatReviewOnly(tmp_path):
    presets.choose("solo")
    rules.set("reviews.final_review.on", "true", None)
    steps = config.steps(config.load(None).values)
    assert (steps.plan_review, steps.code_review, steps.final_review) == (False, False, True)


def test_Save_ThenUse_RestoresTheSamePipeline():
    rules.set("reviews.plan_review.on", "false", None)
    rules.set("reviews.final_review.on", "true", None)
    rules.set("subagents", "1", None)
    before = rules.describe(None)
    rules.save("fast-fix", None)
    rules.clear(None)
    presets.choose("fast-fix")
    after = rules.describe(None)
    assert after["preset"] == "fast-fix" and after["edited"] is False
    assert _by_id(after)["reviews.plan_review.on"]["value"] is False
    assert _by_id(after)["reviews.final_review.on"]["value"] is True
    assert _by_id(after)["subagents"]["value"] == 1
    assert {k: v["value"] for k, v in _by_id(before).items()} == {k: v["value"] for k, v in _by_id(after).items()}


def test_Save_AnExistingOrBuiltInName_IsRefused():
    with pytest.raises(rules.RuleRefused, match="built-in"):
        rules.save("standard", None)
    rules.save("mine", None)
    with pytest.raises(rules.RuleRefused, match="already"):
        rules.save("mine", None)


def test_Save_BadName_IsRefused():
    with pytest.raises(rules.RuleRefused, match="name"):
        rules.save("Not A Name", None)


def test_Clear_DropsTheEdits():
    rules.set("subagents", "5", None)
    rules.clear(None)
    assert rules.describe(None)["edited"] is False and config.load(None).values["knobs"]["subagents"] == 2


def test_Set_ReportLanguage_IsAPreferenceAndLeavesThePresetUnedited():
    done = rules.set("report_language", "en", None)
    assert done["edited"] is False
    assert rules.describe(None)["edited"] is False
    assert config.load(None).values["knobs"]["report_language"] == "en"


def test_Set_ReportLanguage_AfterAnEdit_KeepsTheEditAndTheLanguage():
    rules.set("subagents", "5", None)
    rules.set("report_language", "en", None)
    values = config.load(None).values["knobs"]
    assert rules.describe(None)["edited"] is True
    assert values["subagents"] == 5 and values["report_language"] == "en"


def test_Clear_KeepsTheReportLanguage():
    rules.set("report_language", "en", None)
    rules.set("subagents", "5", None)
    rules.clear(None)
    assert rules.describe(None)["edited"] is False
    assert config.load(None).values["knobs"]["report_language"] == "en"


def test_IsEdited_ALanguageLeftInTheRulesLayerByAnOlderEngine_IsNotAnEdit():
    from sakura import engine_home
    (engine_home.root(None) / config.RULES_FILE).write_text(
        'relax = ["knobs.report_language"]\n[knobs]\nreport_language = "en"\n')
    assert rules.describe(None)["edited"] is False


def test_Set_ReportLanguage_MovesALanguageLeftInTheRulesLayerToThePreferences():
    from sakura import engine_home
    layer = engine_home.root(None) / config.RULES_FILE
    layer.write_text('relax = ["knobs.report_language"]\n[knobs]\nreport_language = "en"\n')
    rules.set("report_language", "it", None)
    assert not layer.exists()
    assert config.load(None).values["knobs"]["report_language"] == "it"


# -- the command line -------------------------------------------------------------------------------------------

def test_Cli_EngineSet_ChangesTheRuleAndRulesJsonShowsIt(capsys):
    assert rules_cli.main(["set", "--actor=cli:me", "--", "subagents", "5"]) == 0
    assert rules_cli.main(["rules", "--json"]) == 0
    described = json.loads(capsys.readouterr().out.split("\n", 1)[1])
    assert described["edited"] is True and _by_id(described)["subagents"]["value"] == 5


def test_Cli_EngineSet_ARefusalPrintsTheReasonAndExitsWithUsage(capsys):
    assert rules_cli.main(["set", "--", "lanes", "99"]) == exits.USAGE
    assert "from 1 to 6" in capsys.readouterr().out


def test_Cli_EngineSet_InsideARole_IsRefused(monkeypatch, capsys):
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    assert rules_cli.main(["set", "--", "subagents", "5"]) == exits.USAGE
    assert rules.is_edited() is False


def test_Cli_PresetSave_KeepsTheRulesAndChoosesThePreset(capsys):
    rules.set("subagents", "4", None)
    assert preset_cli.main(["save", "--actor=cli:me", "--", "mine"]) == exits.OK
    assert presets.chosen() == "mine" and rules.is_edited() is False
    assert presets.resolve("mine").subagents == 4


def test_Cli_PresetSave_ATakenName_IsRefused(capsys):
    assert preset_cli.main(["save", "--", "standard"]) == exits.USAGE
    assert "built-in" in capsys.readouterr().out


def test_Cli_PresetUse_DropsTheEditsOfTheRules(capsys):
    rules.set("subagents", "4", None)
    assert preset_cli.main(["use", "--actor=cli:me", "--", "lean"]) == exits.OK
    assert rules.is_edited() is False and config.load(None).values["knobs"]["subagents"] == 2


def test_Describe_GivesTheLabelOfThePresetInForce():
    from sakura.flow import presets

    presets.choose("lean")
    described = rules.describe(None)
    assert (described["preset"], described["preset_label"]) == ("lean", "Lean")
