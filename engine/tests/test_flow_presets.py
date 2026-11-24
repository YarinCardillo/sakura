import os

import pytest

from sakura import engine_home
from sakura.flow import config, presets


def _project(repo, text):
    """The per-project host layer: a key there that loosens a gate or spends more needs relax."""
    path = engine_home.project_config_file(repo)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _preset_file(name, text):
    folder = engine_home.root() / presets.PRESETS_DIR
    folder.mkdir(exist_ok=True)
    (folder / f"{name}.toml").write_text(text)


def test_builtins_solo_standard_verified_have_independent_knobs():
    solo, standard, verified = (presets.resolve(name) for name in ("solo", "standard", "verified"))
    assert (solo.lanes, solo.subagents, solo.reviewer, solo.evaluator) == (1, 0, False, False)
    assert set(solo.models.values()) == {"claude:sonnet"}
    assert (standard.reviewer, standard.evaluator, standard.lanes) == (True, False, 3)
    assert (verified.reviewer, verified.evaluator, verified.lanes) == (True, True, 3)
    assert presets.names()[:3] == ["solo", "standard", "verified"]


def test_Presets_Lanes_IsTheMasterCeiling():
    solo, standard, verified = (presets.resolve(name) for name in ("solo", "standard", "verified"))
    assert (solo.lanes, standard.lanes, verified.lanes) == (1, 3, 3)
    assert (solo.builders, standard.builders, verified.builders) == (1, 3, 3)
    assert {"builders", "lanes"} <= set(solo.knobs())


def test_Presets_HostFile_SetsBuildersAndLanesIndependently():
    _preset_file("wide", 'base = "standard"\nbuilders = 5\n')
    _preset_file("narrow", 'base = "standard"\nlanes = 2\n')
    assert (presets.resolve("wide").builders, presets.resolve("wide").lanes) == (5, 3)
    assert (presets.resolve("narrow").builders, presets.resolve("narrow").lanes) == (3, 2)


def test_Presets_BuildersOutOfRange_IsRefused():
    _preset_file("bad", "builders = 7\n")
    with pytest.raises(presets.PresetError, match="builders must be a number from 1 to 6"):
        presets.resolve("bad")


def test_preset_layer_sets_every_role_from_its_tier():
    layer = presets.resolve("verified").layer()
    assert layer["roles"] == {"planner": "claude:opus", "evaluator": "claude:opus", "builder": "claude:sonnet",
                              "reporter": "claude:sonnet", "plan_reviewer": "claude:sonnet",
                              "code_reviewer": "claude:sonnet"}
    assert layer["knobs"]["cage"] == "default"


def test_host_preset_starts_from_its_base_and_changes_only_named_knobs():
    _preset_file("night", 'base = "verified"\nlanes = 2\npace_day_pct = 12.5\n[models]\nreview = "codex"\n')
    night = presets.resolve("night")
    assert (night.name, night.lanes, night.pace_day_pct, night.evaluator) == ("night", 2, 12.5, True)
    assert night.models == {"lead": "claude:opus", "worker": "claude:sonnet", "review": "codex"}
    assert "night" in presets.names()


def test_host_preset_without_base_starts_from_standard():
    _preset_file("quick", "fix_rounds = 0\n")
    quick = presets.resolve("quick")
    assert (quick.evaluator, quick.reviewer, quick.fix_rounds) == (False, True, 0)


@pytest.mark.parametrize("text, message", [
    ("lanes = 0\n", "lanes must be a number from 1 to 6"),
    ("lanes = 2.5\n", "lanes must be a whole number"),
    ("pace_day_pct = 150\n", "pace_day_pct must be a number from 0 to 100"),
    ("reviewer = 1\n", "reviewer must be true or false"),
    ("colour = 'red'\n", "unknown keys colour"),
    ("[models]\nboss = 'claude'\n", "models takes the tiers"),
    ("cage = '../x'\n", "cage must name a cage policy"),
    ("lanes = [", "cannot read"),
])
def test_bad_host_preset_names_its_file_and_problem(text, message):
    _preset_file("broken", text)
    with pytest.raises(presets.PresetError, match=message) as raised:
        presets.resolve("broken")
    assert "broken.toml" in str(raised.value)


def test_unknown_preset_and_bad_names_are_refused():
    with pytest.raises(presets.PresetError, match="unknown preset ghost"):
        presets.resolve("ghost")
    with pytest.raises(presets.PresetError, match="preset name must match"):
        presets.resolve("../etc")


def test_base_cycle_is_refused():
    _preset_file("a", 'base = "b"\n')
    _preset_file("b", 'base = "a"\n')
    with pytest.raises(presets.PresetError, match="its own base"):
        presets.resolve("a")


def test_host_file_cannot_shadow_a_builtin():
    _preset_file("solo", "lanes = 4\n")
    with pytest.raises(presets.PresetError, match="name of a built-in preset"):
        presets.resolve("solo")
    assert presets.names().count("solo") == 1


def test_choose_records_the_host_choice_privately():
    assert presets.chosen() == ""
    presets.choose("solo")
    assert presets.chosen() == "solo"
    assert oct(os.stat(engine_home.root() / presets.CHOICE_FILE).st_mode & 0o777) != "0o777"
    with pytest.raises(presets.PresetError):
        presets.choose("ghost")
    assert presets.chosen() == "solo"


def test_weakens_compares_reviewer_and_evaluator():
    verified, standard, solo = (presets.resolve(name) for name in ("verified", "standard", "solo"))
    assert presets.weakens(verified, standard) and presets.weakens(standard, solo)
    assert not presets.weakens(solo, verified) and not presets.weakens(standard, verified)


# -- the preset inside the layered config --------------------------------------------------------


def _write(repo, name, text):
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / name).write_text(text)


def test_config_takes_knobs_and_roles_from_the_preset(repo):
    values = config.load(repo, overrides={"preset": "solo", "relax": ["preset"]}).values
    assert values["knobs"]["lanes"] == 1 and values["knobs"]["subagents"] == 0
    assert values["roles"]["planner"] == "claude:sonnet"
    assert config.steps(values) == config.PresetSteps(False, False, False)


def test_config_origins_name_the_preset_behind_knobs_and_moved_roles(repo):
    cfg = config.load(repo, overrides={"preset": "solo", "relax": ["preset"]})
    assert cfg.origins["knobs.lanes"] == "preset:solo"
    assert cfg.origins["roles.planner"] == "preset:solo"
    # A role the preset leaves at its default stays with the budget router.
    assert cfg.origins["roles.builder"] == "defaults"


def test_file_layer_overrides_a_single_knob_or_role(repo):
    _project(repo, '[knobs]\nlanes = 2\n[roles]\ncode_reviewer = "codex"\n')
    values = config.load(repo).values
    assert values["knobs"]["lanes"] == 2 and values["roles"]["code_reviewer"] == "codex"
    assert values["roles"]["planner"] == "claude:opus"


def test_weaker_preset_from_a_host_layer_needs_relax_and_a_repository_may_not_name_one(repo):
    _project(repo, 'preset = "solo"\n')
    with pytest.raises(config.FlowConfigError, match="relaxes preset"):
        config.load(repo)
    _project(repo, 'preset = "solo"\nrelax = ["preset"]\n')
    assert config.load(repo).values["preset"] == "solo"
    _write(repo, "flow.toml", 'preset = "verified"\n')
    with pytest.raises(config.FlowConfigError, match="preset may only be set by the host"):
        config.load(repo)


def test_turning_reviewer_or_evaluator_off_by_key_needs_relax(repo):
    for knob in ("reviewer", "evaluator"):
        _write(repo, "flow.toml", f"[knobs]\n{knob} = false\n")
        with pytest.raises(config.FlowConfigError, match=f"knobs.{knob}"):
            config.load(repo)


def test_host_choice_applies_without_relax_and_is_recorded(repo):
    presets.choose("solo")
    cfg = config.load(repo)
    assert cfg.values["preset"] == "solo"
    assert cfg.origins["preset"] == "chosen"
    assert any(entry.startswith("preset") for entry in cfg.relaxations)


def test_flag_preset_beats_the_host_choice_with_consent_to_spend_more(repo):
    presets.choose("solo")
    # verified puts the planner and the evaluator back on Opus: more spend, so the flag layer needs consent.
    with pytest.raises(config.FlowConfigError, match="relaxes preset"):
        config.load(repo, overrides={"preset": "verified"})
    assert config.load(repo, overrides={"preset": "verified", "relax": ["preset"]}).values["preset"] == "verified"


def test_unknown_preset_in_config_is_a_config_error(repo):
    _project(repo, 'preset = "ghost"\nrelax = ["preset"]\n')
    with pytest.raises(config.FlowConfigError, match="unknown preset ghost"):
        config.load(repo)


def test_Dial_SetsBuildersAndFixRounds(repo):
    values = config.load(repo, overrides={"dial": "save"}).values
    assert (values["knobs"]["builders"], values["knobs"]["fix_rounds"]) == (1, 1)
    values = config.load(repo, overrides={"dial": "power"}).values
    assert (values["knobs"]["builders"], values["knobs"]["fix_rounds"]) == (6, 4)
    assert values["knobs"]["lanes"] == 3


def test_Dial_LeavesTheLanesCeiling(repo):
    values = config.load(repo, overrides={"dial": "power"}).values
    assert values["knobs"]["lanes"] == presets.resolve("standard").lanes


def test_default_dial_leaves_the_preset_knobs(repo):
    values = config.load(repo, overrides={"preset": "solo", "relax": ["preset"]}).values
    assert (values["knobs"]["builders"], values["knobs"]["fix_rounds"]) == (1, 2)


def test_a_knob_set_by_a_layer_wins_over_the_dial(repo):
    _project(repo, "[knobs]\nbuilders = 2\n")
    values = config.load(repo, overrides={"dial": "power"}).values
    assert (values["knobs"]["builders"], values["knobs"]["fix_rounds"]) == (2, 4)


def test_knob_values_are_validated(repo):
    for text in ("[knobs]\nlanes = 0\n", "[knobs]\nsubagents = -1\n", "[knobs]\ncage = 'a b'\n",
                 "[knobs]\nfix_rounds = true\n"):
        _write(repo, "flow.toml", text)
        with pytest.raises(config.FlowConfigError, match="knobs."):
            config.load(repo)


# -- sakura preset --------------------------------------------------------------------------------


def test_preset_cli_lists_marks_the_choice_and_uses(capsys, monkeypatch):
    from sakura import cli

    assert cli.main(["preset", "use", "solo"]) == 0
    assert "preset solo is the host default" in capsys.readouterr().out
    assert cli.main(["preset", "list"]) == 0
    listing = capsys.readouterr().out
    assert "* solo" in listing and "  verified" in listing
    assert cli.main(["preset", "use", "ghost"]) != 0
    assert "unknown preset ghost" in capsys.readouterr().out
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["preset", "use", "verified"]) != 0
    assert presets.chosen() == "solo"


def test_preset_cli_show_named_and_effective(capsys, repo, monkeypatch):
    import json

    from sakura import cli

    assert cli.main(["preset", "show", "standard", "--json"]) == 0
    assert json.loads(capsys.readouterr().out)["evaluator"] is False
    monkeypatch.chdir(repo)
    presets.choose("solo")
    assert cli.main(["preset", "show"]) == 0
    shown = capsys.readouterr().out
    assert "preset: solo (chosen)" in shown and "lanes: 1 (preset:solo)" in shown
    assert "builders: 1 (preset:solo)" in shown


def test_start_ledgers_the_effective_preset():
    from sakura.flow import preset_cli

    values = config.load(None, overrides={"preset": "solo", "relax": ["preset"]}).values
    line = preset_cli.summary(values)
    assert line.startswith("preset solo: builders 1, lanes 1, subagents 0, reviewer off, evaluator off, fix rounds 2, pace normal")
    assert "builder=claude:sonnet" in line


# -- repository layers are untrusted: what they may change, and with whose consent ------------------


def test_unknown_preset_named_by_any_layer_fails_closed(repo):
    engine_home.config_file().write_text('preset = "ghost"\nrelax = ["preset"]\n')
    _project(repo, 'preset = "verified"\n')
    with pytest.raises(config.FlowConfigError, match="unknown preset ghost"):
        config.load(repo)


def test_unresolvable_preset_counts_as_weakening():
    assert config._preset_weakens(engine_home.root(), "verified", "ghost")
    assert config._preset_weakens(engine_home.root(), "ghost", "verified")


@pytest.mark.parametrize("layer, text", [("flow.toml", "[knobs]\ncage = 'open'\n"),
                                         ("local.toml", "[flow.knobs]\ncage = 'open'\n")])
def test_cage_knob_is_refused_in_repository_layers(repo, layer, text):
    _write(repo, layer, text)
    with pytest.raises(config.FlowConfigError, match="knobs.cage"):
        config.load(repo)


def test_cage_knob_is_accepted_from_host_layers(repo):
    engine_home.config_file().write_text("[knobs]\ncage = 'night'\n")
    assert config.load(repo).values["knobs"]["cage"] == "night"
    assert config.load(repo, overrides={"knobs": {"cage": "day"}}).values["knobs"]["cage"] == "day"


def test_a_preset_named_by_a_host_layer_needs_consent_for_what_it_loosens(repo):
    _preset_file("wide", 'base = "verified"\nlanes = 6\n')
    _project(repo, 'preset = "wide"\n')
    with pytest.raises(config.FlowConfigError, match="project flow config relaxes preset"):
        config.load(repo)
    _project(repo, 'preset = "wide"\nrelax = ["preset"]\n')
    assert config.load(repo).values["knobs"]["lanes"] == 6


@pytest.mark.parametrize("text", ['[models]\nreview = "none"\n', '[models]\nworker = "claude:opus"\n',
                                  '[models]\nreview = "codex:gpt-9"\n', 'cage = "open"\n', "subagents = 4\n",
                                  "pace_day_pct = 0\n"])
def test_preset_role_and_knob_changes_go_through_the_relax_checks(repo, text):
    _preset_file("base10", 'base = "verified"\npace_day_pct = 10\n')
    _preset_file("changed", f'base = "base10"\n{text}')
    engine_home.config_file().write_text('preset = "base10"\n')
    _project(repo, 'preset = "changed"\n')
    with pytest.raises(config.FlowConfigError, match="relaxes preset"):
        config.load(repo)


def test_a_cheaper_or_stricter_preset_needs_no_consent(repo):
    _preset_file("frugal", 'base = "verified"\nlanes = 1\nsubagents = 0\npace_day_pct = 5\n[models]\nlead = "claude:sonnet"\n')
    _project(repo, 'preset = "frugal"\n')
    cfg = config.load(repo)
    assert cfg.values["preset"] == "frugal" and cfg.relaxations == []


@pytest.mark.parametrize("text, key", [("[knobs]\nlanes = 4\n", "knobs.lanes"),
                                       ("[knobs]\nbuilders = 4\n", "knobs.builders"),
                                       ("[knobs]\nsubagents = 3\n", "knobs.subagents")])
def test_raising_lanes_builders_or_subagents_needs_relax(repo, text, key):
    _project(repo, text)
    with pytest.raises(config.FlowConfigError, match=key):
        config.load(repo)
    _project(repo, f'relax = ["{key}"]\n' + text)
    assert key in config.load(repo).relaxations[0]


def test_raising_or_zeroing_the_pace_needs_relax(repo):
    engine_home.config_file().write_text("[knobs]\npace_day_pct = 10\n")
    for value in (20, 0):
        _project(repo, f"[knobs]\npace_day_pct = {value}\n")
        with pytest.raises(config.FlowConfigError, match="knobs.pace_day_pct"):
            config.load(repo)
    _project(repo, "[knobs]\npace_day_pct = 5\n")
    assert config.load(repo).values["knobs"]["pace_day_pct"] == 5


def test_the_dial_is_the_hosts(repo):
    _write(repo, "flow.toml", 'dial = "save"\n')
    with pytest.raises(config.FlowConfigError, match="dial may only be set by the host"):
        config.load(repo)
    _write(repo, "flow.toml", "")
    _project(repo, 'dial = "save"\n')
    assert config.load(repo).values["knobs"]["builders"] == 1
    assert config.load(repo, overrides={"dial": "power"}).values["knobs"]["builders"] == 6


@pytest.mark.parametrize("relax", ["", 'relax = ["preset"]\n'])
@pytest.mark.parametrize("layer", ["flow.toml", "local.toml"])
def test_a_repository_layer_cannot_pick_a_preset(repo, relax, layer):
    _preset_file("loose", 'base = "verified"\ncage = "open"\n')
    text = f'{relax}preset = "loose"\n'
    _write(repo, layer, text if layer == "flow.toml" else f"[flow]\n{text}")
    with pytest.raises(config.FlowConfigError, match="preset may only be set by the host"):
        config.load(repo)


def test_the_cage_follows_host_layers_only(repo):
    _preset_file("loose", 'base = "verified"\ncage = "open"\n')
    engine_home.config_file().write_text('preset = "loose"\nrelax = ["preset"]\n')
    assert config.load(repo).values["knobs"]["cage"] == "open"
    _write(repo, "flow.toml", 'preset = "standard"\n')
    with pytest.raises(config.FlowConfigError, match="preset may only be set by the host"):
        config.load(repo)

# -- the reporter and its language are preset knobs ---------------------------------------------------------------------

def test_every_builtin_runs_the_reporter_in_italian_and_its_knobs_carry_both():
    for name in presets.BUILTINS:
        preset = presets.resolve(name)
        assert (preset.reporter, preset.report_language) == (True, "it"), name
        assert {"reporter": True, "report_language": "it"}.items() <= preset.knobs().items()


def test_a_host_preset_sets_the_reporter_and_the_report_language():
    _preset_file("quiet", 'reporter = false\nreport_language = "en"\n')
    quiet = presets.resolve("quiet")
    assert (quiet.reporter, quiet.report_language) == (False, "en")
    assert presets.weakens(presets.resolve("standard"), quiet)


@pytest.mark.parametrize("text, message", [("reporter = 1\n", "reporter"), ('report_language = "de"\n', "report_language")])
def test_a_host_preset_with_a_bad_reporter_knob_is_refused(text, message):
    _preset_file("bad", text)
    with pytest.raises(presets.PresetError, match=message):
        presets.resolve("bad")


def test_the_report_language_is_a_knob_and_report_language_is_gone(repo):
    engine_home.config_file().write_text('[knobs]\nreport_language = "en"\n')
    assert config.load(repo).values["knobs"]["report_language"] == "en"
    assert "language" not in config.load(repo).values["report"]
    engine_home.config_file().write_text('[report]\nlanguage = "en"\n')
    with pytest.raises(config.FlowConfigError, match=r"report\.language.*knobs\.report_language"):
        config.load(repo)


def test_turning_the_reporter_off_by_key_needs_relax_and_a_repository_may_not(repo):
    _project(repo, "[knobs]\nreporter = false\n")
    with pytest.raises(config.FlowConfigError, match="knobs.reporter"):
        config.load(repo)
    _project(repo, 'relax = ["knobs.reporter"]\n[knobs]\nreporter = false\n')
    assert config.load(repo).values["knobs"]["reporter"] is False
    _project(repo, "")
    for layer, text in (("flow.toml", "[knobs]\nreporter = false\n"), ("flow.toml", '[knobs]\nreport_language = "en"\n')):
        _write(repo, layer, text)
        with pytest.raises(config.FlowConfigError, match="knobs"):
            config.load(repo)


def test_preset_show_lists_the_reporter_and_its_language(capsys, repo, monkeypatch):
    import json

    from sakura import cli

    assert cli.main(["preset", "show", "standard", "--json"]) == 0
    shown = json.loads(capsys.readouterr().out)
    assert (shown["reporter"], shown["report_language"]) == (True, "it")
    monkeypatch.chdir(repo)
    assert cli.main(["preset", "show"]) == 0
    effective = capsys.readouterr().out
    assert "reporter: True" in effective and "report_language: it" in effective


def test_Presets_Lean_PlansBuildsAndReviewsTheWholeBranchOnly():
    lean = presets.resolve("lean")
    assert (lean.plan, lean.plan_review, lean.task_review, lean.final_review, lean.checks) == (
        True, False, False, True, False)
    assert presets.names()[:3] == ["solo", "standard", "verified"] and "lean" in presets.names()


def test_Presets_Solo_HasNoPlanAndNoChecks():
    solo = presets.resolve("solo")
    assert (solo.plan, solo.checks, solo.final_review) == (False, False, False)


def test_Presets_Knobs_CarryTheStepsAndTheLaneMinimum():
    knobs = presets.resolve("standard").knobs()
    assert (knobs["plan"], knobs["plan_review"], knobs["task_review"], knobs["final_review"], knobs["checks"]) == (
        True, True, True, False, True)
    assert (knobs["lanes_min"], knobs["final_review_rounds"]) == (1, 1)
    assert presets.resolve("standard").layer()["step_accounts"]["build"] == "project"


def test_Presets_HostFile_SetsTheStepsAndTheirAccounts():
    _preset_file("careful", 'base = "standard"\nfinal_review = true\nlanes_min = 2\n[accounts]\nbuild = "work"\n')
    careful = presets.resolve("careful")
    assert careful.final_review is True and careful.lanes_min == 2
    accounts = careful.layer()["step_accounts"]
    assert accounts["build"] == "work" and accounts["plan"] == "project"


def test_Presets_LanesMinAboveLanes_IsRefused():
    _preset_file("bad", 'lanes_min = 4\nlanes = 2\n')
    with pytest.raises(presets.PresetError, match="lanes_min"):
        presets.resolve("bad")


def test_Presets_Weakens_CountsAReviewTurnedOff():
    standard, lean = presets.resolve("standard"), presets.resolve("lean")
    solo = presets.resolve("solo")
    assert presets.weakens(standard, lean) and not presets.weakens(solo, standard)


def test_Config_ChoosingLean_NeedsNoRelax(repo):
    presets.choose("lean")
    values = config.load(repo).values
    assert values["knobs"]["final_review"] is True and values["knobs"]["plan_review"] is False


def test_Steps_VerifiedWithTheChecksSwitchedOff_RunNoEvaluationWhileTheEvaluatorSettingStays(repo):
    _preset_file("nochecks", 'base = "verified"\nchecks = false\n')

    values = config.load(repo, overrides={"preset": "nochecks", "relax": ["preset"]}).values

    assert values["knobs"]["evaluator"] is True and values["knobs"]["checks"] is False
    assert config.steps(values).evaluate is False


# -- the name a person reads next to a preset id ------------------------------------------------------------------------------


@pytest.mark.parametrize(("name", "label"), [("lean", "Lean"), ("solo", "Solo"), ("verified", "Verified"),
                                              ("night-shift", "Night shift"), ("my_team", "My team")])
def test_Label_ThePresetNameInWords(name, label):
    assert presets.label(name) == label


def test_PresetCli_ListJson_GivesEveryPresetItsLabelDescriptionAndWhetherItIsChosen(capsys):
    import json

    from sakura import cli

    presets.choose("lean")
    assert cli.main(["preset", "list", "--json"]) == 0
    listed = {entry["name"]: entry for entry in json.loads(capsys.readouterr().out)}
    assert list(listed) == presets.names()
    assert listed["lean"]["label"] == "Lean" and listed["lean"]["chosen"] is True and listed["solo"]["chosen"] is False
    assert listed["lean"]["description"] == presets.BUILTINS["lean"].description


def test_PresetCli_ListJsonWithNothingChosen_MarksThePresetTheEngineRulesReport(capsys):
    import json

    from sakura import cli
    from sakura.flow import rules

    assert presets.chosen() == ""
    assert cli.main(["preset", "list", "--json"]) == 0
    chosen = [entry["name"] for entry in json.loads(capsys.readouterr().out) if entry["chosen"]]
    assert chosen == [rules.describe(None)["preset"]] == [presets.DEFAULT_BASE]


def test_PresetCli_ListJsonAfterEditingTheRules_StillMarksTheirPreset(capsys):
    import json

    from sakura import cli
    from sakura.flow import rules

    presets.choose("lean")
    assert cli.main(["preset", "list", "--json"]) == 0
    chosen = [entry["name"] for entry in json.loads(capsys.readouterr().out) if entry["chosen"]]
    assert chosen == [rules.describe(None)["preset"]] == ["lean"]


def test_PresetCli_ShowJson_CarriesTheLabel(capsys):
    import json

    from sakura import cli

    assert cli.main(["preset", "show", "lean", "--json"]) == 0
    assert json.loads(capsys.readouterr().out)["label"] == "Lean"


def test_PresetCli_ListJson_MarksTheLegacyPresets(capsys):
    import json

    from sakura import cli

    assert cli.main(["preset", "list", "--json"]) == 0
    legacy = {entry["name"]: entry["legacy"] for entry in json.loads(capsys.readouterr().out)}
    assert legacy["vanilla"] is True and legacy["standard"] is False
