import json
import re

import pytest

from sakura import engine_home
from sakura.flow import cli, config, exits


@pytest.fixture
def home(tmp_path):
    return tmp_path / "home"


def _write(repo, name, text):
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / name).write_text(text)


def test_flow_defaults_load(repo, home):
    result = config.load(repo, home=home)
    assert result.values["preset"] == "standard"
    assert result.origins["preset"] == "defaults"
    assert result.values["timeouts"]["idle_seconds"] == 600
    assert result.values["app"]["origin"] == ""
    assert result.values["contract"]["required_ids"] == []


def test_contract_required_ids_are_unique_nonempty_strings(repo, home):
    _write(repo, "flow.toml", '[contract]\nrequired_ids = ["R1", "R15"]\n')
    assert config.load(repo, home=home).values["contract"]["required_ids"] == ["R1", "R15"]
    for value in ('["R1", "R1"]', '["R1", ""]', '[1]'):
        _write(repo, "flow.toml", f'[contract]\nrequired_ids = {value}\n')
        with pytest.raises(config.FlowConfigError, match="contract.required_ids"):
            config.load(repo, home=home)


def test_app_origin_accepts_only_explicit_http_loopback_origins(repo, home):
    for origin in ("http://localhost:4202", "https://localhost:4202", "http://example.com:4202", "http://localhost",
                   "http://localhost:4202/path"):
        engine_home.config_file().write_text(f'[builders]\nmax = 1\n[app]\norigin = "{origin}"\n')
        if origin == "http://localhost:4202":
            assert config.load(repo, home=home).values["app"]["origin"] == origin
            continue
        with pytest.raises(config.FlowConfigError, match="app.origin"):
            config.load(repo, home=home)


def test_app_origin_is_a_host_setting(repo, home):
    _write(repo, "flow.toml", '[app]\norigin = "http://localhost:4202"\n')
    with pytest.raises(config.FlowConfigError, match="app.origin may only be set by the host"):
        config.load(repo, home=home)


def test_app_origin_with_parallel_builders_rejected(repo, home):
    engine_home.config_file().write_text('[builders]\nmax = 2\n[app]\norigin = "http://localhost:4200"\n')
    with pytest.raises(config.FlowConfigError) as raised:
        config.load(repo, home=home)
    assert str(raised.value) == "app.origin needs builders.max = 1: every workspace would need the same port block"

def test_load_config_defaults_and_validates_values(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    assert config.load(repo, home=tmp_path / "home").values["load"] == {
        "slots": 0, "idle_stack_seconds": 900, "critical": [], "nice": 10}
    engine_home.config_file().write_text('[load]\nslots = 2\nidle_stack_seconds = 30\ncritical = ["T1"]\nnice = 5\n')
    assert config.load(repo, home=tmp_path / "home").values["load"] == {
        "slots": 2, "idle_stack_seconds": 30, "critical": ["T1"], "nice": 5}
    engine_home.config_file().write_text("[load]\nnice = 20\n")
    with pytest.raises(config.FlowConfigError, match="load.nice"):
        config.load(repo, home=tmp_path / "home")


def test_flow_preset_downgrade_requires_relax(repo, home):
    _write(repo, "local.toml", '[flow]\npreset = "standard"\n')
    with pytest.raises(config.FlowConfigError, match="preset"):
        config.load(repo, home=home)


def test_flow_relaxation_listed_with_layers(repo, home):
    engine_home.config_file().write_text('preset = "solo"\nrelax = ["preset"]\n')
    result = config.load(repo, home=home)
    assert result.values["preset"] == "solo"
    assert result.relaxations == ["preset (defaults to personal)"]


@pytest.mark.parametrize("name, text", [("flow.toml", 'relax = ["preset"]\n'), ("local.toml", '[flow]\nrelax = []\n')])
def test_a_repository_layer_cannot_carry_relax_at_all(repo, home, name, text):
    _write(repo, name, text)
    with pytest.raises(config.FlowConfigError, match="may only tighten settings, so it cannot carry relax"):
        config.load(repo, home=home)


def test_flow_personal_and_shared_layers_apply(repo, home):
    engine_home.config_file().write_text('dial = "lean"\n')
    _write(repo, "flow.toml", '[report]\nproject = "demo"\n')
    result = config.load(repo, home=home)
    assert (result.origins["dial"], result.origins["report.project"]) == ("personal", "shared")


def test_the_project_layer_in_the_engine_home_sits_between_shared_and_local(repo, home):
    _write(repo, "flow.toml", '[report]\nproject = "demo"\n[app]\nready_seconds = 10\n')
    _write(repo, "local.toml", '[flow.app]\nstart = "make run"\n')
    project = engine_home.project_config_file(repo)
    project.parent.mkdir(parents=True)
    project.write_text('[app]\nready_seconds = 20\nstart = "make serve"\n')
    result = config.load(repo, home=home)
    assert (result.values["app"]["ready_seconds"], result.origins["app.ready_seconds"]) == (20, "project")
    assert (result.values["app"]["start"], result.origins["app.start"]) == ("make run", "local")
    assert result.origins["report.project"] == "shared"


def test_a_folder_outside_git_has_no_project_layer(tmp_path, home):
    plain = tmp_path / "plain"
    plain.mkdir()
    assert "project" not in config.load(plain, home=home).origins.values()


def test_flow_overrides_are_the_last_layer(repo, home):
    result = config.load(repo, home=home, overrides={"preset": "vanilla", "relax": ["preset"]})
    assert result.origins["preset"] == "flags"
    assert result.relaxations == ["preset (defaults to flags)"]


def test_flow_unknown_key_rejected(repo, home):
    _write(repo, "flow.toml", "bogus = 1\n")
    with pytest.raises(config.FlowConfigError, match="bogus"):
        config.load(repo, home=home)


def test_flow_invalid_enum_values_rejected(repo, home):
    for text in ('dial = "turbo"\n', 'preset = "ultra"\n', 'familiarity = "x"\n', '[roles]\nplanner = "gpt"\n'):
        _write(repo, "flow.toml", text)
        with pytest.raises(config.FlowConfigError):
            config.load(repo, home=home)


def test_silent_check_in_command_rejected(repo, home):
    _write(repo, "flow.toml", '[tests]\nfull = ["pytest >/dev/null && echo ok"]\n')
    with pytest.raises(config.FlowConfigError, match="silent check"):
        config.load(repo, home=home)


def test_evaluator_codex_rejected(repo, home):
    _write(repo, "flow.toml", '[roles]\nevaluator = "codex"\n')
    with pytest.raises(config.FlowConfigError, match="evaluator"):
        config.load(repo, home=home)


def test_role_none_requires_relax(repo, home):
    _write(repo, "flow.toml", '[roles]\nevaluator = "none"\n')
    with pytest.raises(config.FlowConfigError, match="roles.evaluator"):
        config.load(repo, home=home)


# -- Opus on a role other than the planner and the evaluator is the person's choice only ----------------------------

def _personal(home, text):
    engine_home.config_file().write_text(text)




# -- reviewers and the evaluator decide whether work passes: only the host picks them ----------------------------

@pytest.mark.parametrize("name, text", [("flow.toml", '[roles]\ncode_reviewer = "exec:python3 approve.py"\n'),
                                        ("flow.toml", '[roles]\nplan_reviewer = "exec:python3 approve.py"\n'),
                                        ("flow.toml", '[roles]\nevaluator = "exec:python3 approve.py"\n'),
                                        ("flow.toml", '[roles]\ncode_reviewer = "claude:sonnet"\n'),
                                        ("local.toml", '[flow.roles]\nplan_reviewer = "codex"\n'),
                                        ("local.toml", '[flow.roles]\ncode_reviewer = "claude:opus-4-5"\n'),
                                        ("local.toml", '[flow.roles]\nevaluator = "claude:opus"\n')])
def test_GateRole_FromARepositoryLayer_IsRefusedWhateverItsProvider(repo, home, name, text):
    _write(repo, name, text)
    with pytest.raises(config.FlowConfigError,
                       match=r"roles\.(plan_reviewer|code_reviewer|evaluator) may only be set by the host"):
        config.load(repo, home=home)


def test_GateRole_FromTheHost_MayBeAnExecProvider(repo, home):
    _personal(home, '[roles]\ncode_reviewer = "exec:python3 approve.py"\n')
    assert config.load(repo, home=home).values["roles"]["code_reviewer"] == "exec:python3 approve.py"
    flagged = config.load(repo, home=home, overrides={"roles": {"evaluator": "exec:python3 judge.py"}})
    assert flagged.values["roles"]["evaluator"] == "exec:python3 judge.py"


# -- a repository layer declares project facts only; every other key is the host's -----------------------------

REFUSED_FAMILIES = {
    "roles": '[roles]\nbuilder = "claude:haiku"\n', "preset": 'preset = "verified"\n', "dial": 'dial = "save"\n',
    "knobs": "[knobs]\nfix_rounds = 1\n", "lanes": "[knobs]\nlanes = 1\n", "subagents": "[knobs]\nsubagents = 0\n",
    "pace": "[knobs]\npace_day_pct = 5\n", "reviewer": "[knobs]\nreviewer = true\n", "evaluator": "[knobs]\nevaluator = true\n",
    "cage": '[knobs]\ncage = "default"\n', "routes": '[budget.routes]\nreviewer = "haiku"\n',
    "budget": "[budget]\nreserve_week_pct = 50\n", "checks": '[checks]\napproved = ["npm test"]\n',
    "builders": "[builders]\nmax = 1\n", "timeouts": "[timeouts]\nbuild = 60\n", "load": "[load]\nslots = 1\n",
    "familiarity": 'familiarity = "new"\n', "approve_spec": "approve_spec = true\n",
    "workspace root": '[workspace]\nroot = "/tmp/elsewhere"\n', "surface": "[surface]\ntmux = false\n",
    "limits": "[limits]\nclaude_max_budget_usd = 1.0\n", "stack": '[stack]\nup = "docker compose up -d"\n',
}
REFUSED_KEYS = {"roles": "roles.builder", "preset": "preset", "dial": "dial", "knobs": "knobs.fix_rounds",
                "lanes": "knobs.lanes", "subagents": "knobs.subagents", "pace": "knobs.pace_day_pct",
                "reviewer": "knobs.reviewer", "evaluator": "knobs.evaluator", "cage": "knobs.cage",
                "routes": "budget.routes.reviewer", "budget": "budget.reserve_week_pct", "checks": "checks.approved",
                "builders": "builders.max", "timeouts": "timeouts.build", "load": "load.slots",
                "familiarity": "familiarity", "approve_spec": "approve_spec", "workspace root": "workspace.root",
                "surface": "surface.tmux", "limits": "limits.claude_max_budget_usd", "stack": "stack.up"}


def test_Config_RepositoryRaisesBuilders_Refused(repo, home):
    _write(repo, "flow.toml", "[knobs]\nbuilders = 6\n")
    with pytest.raises(config.FlowConfigError, match=r"^knobs.builders may only be set by the host"):
        config.load(repo, home=home)


@pytest.mark.parametrize("family", list(REFUSED_FAMILIES))
def test_RepositoryLayer_AHostKey_IsRefusedNamingIt(repo, home, family):
    _write(repo, "flow.toml", REFUSED_FAMILIES[family])
    with pytest.raises(config.FlowConfigError, match=rf"^{re.escape(REFUSED_KEYS[family])} may only be set by the host"):
        config.load(repo, home=home)


def test_RepositoryLayer_TheLocalOverlay_IsHeldToTheSameKeys(repo, home):
    _write(repo, "local.toml", '[flow.budget.routes]\nreviewer = "haiku"\n')
    with pytest.raises(config.FlowConfigError, match="budget.routes.reviewer may only be set by the host"):
        config.load(repo, home=home)


def test_RepositoryLayer_ProjectFacts_Load(repo, home):
    _write(repo, "flow.toml", '[workspace]\nbranch = "team/{task}"\nsetup = ["make deps"]\n'
                              '[app]\nstart = "make run"\nstop = "make stop"\nready_url = "http://127.0.0.1:1/"\n'
                              'ready_seconds = 30\n[tests]\nfocused = ["make test"]\nfull = ["make check"]\n'
                              'viewports = ["1440x900", "390x844", "1280x800"]\n[contract]\n'
                              'required_ids = ["R1"]\n[brakes]\ntest_patterns = ["tests/**", "spec/**", "test/**", '
                              '"e2e/**", "**/test_*.py", "**/*_test.*", "**/*.test.*", "**/*.spec.*"]\n'
                              '[report]\nproject = "demo"\n')
    values = config.load(repo, home=home).values
    assert values["workspace"]["branch"] == "team/{task}" and values["app"]["stop"] == "make stop"
    assert values["tests"]["full"] == ["make check"] and values["report"]["project"] == "demo"


def test_RepositoryLayer_TheHostStillSetsEveryKey(repo, home):
    _personal(home, '[budget.routes]\nreviewer = "haiku"\n[knobs]\nlanes = 2\n')
    flagged = config.load(repo, home=home, overrides={"dial": "save", "roles": {"code_reviewer": "exec:python3 r.py"}})
    assert flagged.values["budget"]["routes"]["reviewer"] == "haiku" and flagged.values["dial"] == "save"


def test_a_repository_layer_that_declares_the_opus_role_relaxation_is_refused(repo, home):
    _write(repo, "flow.toml", 'relax = ["tests.full"]\n\n[tests]\nfull = []\n')
    with pytest.raises(config.FlowConfigError, match="cannot carry relax"):
        config.load(repo, home=home)


def test_the_personal_layer_and_flags_may_choose_opus_for_any_role(repo, home):
    _personal(home, '[roles]\nbuilder = "claude:opus"\n')
    assert config.load(repo, home=home).values["roles"]["builder"] == "claude:opus"
    result = config.load(repo, home=home, overrides={"roles": {"reporter": "claude:opus"}})
    assert result.values["roles"]["reporter"] == "claude:opus" and result.relaxations == []






def test_approve_spec_and_test_list_loss_require_relax(repo, home):
    _write(repo, "flow.toml", "approve_spec = false\n")
    with pytest.raises(config.FlowConfigError, match="approve_spec"):
        config.load(repo, home=home)
    _write(repo, "flow.toml", '[tests]\nfull = ["a", "b"]\n')
    _write(repo, "local.toml", '[flow.tests]\nfull = ["a"]\n')
    with pytest.raises(config.FlowConfigError, match="tests.full"):
        config.load(repo, home=home)


def test_string_test_value_normalizes_to_list(repo, home):
    _write(repo, "flow.toml", '[tests]\nfocused = "pytest -q"\n')
    assert config.load(repo, home=home).values["tests"]["focused"] == ["pytest -q"]


def test_steps_follow_preset_and_turn_off_with_none_role():
    values = {"preset": "verified", "roles": {"plan_reviewer": "codex", "code_reviewer": "none", "evaluator": "claude:sonnet"}}
    assert config.steps(values) == config.PresetSteps(True, False, True)
    assert config.PRESETS["vanilla"] == config.PresetSteps(False, False, False)
    assert config.PRESETS["standard"] == config.PresetSteps(True, True, False)


def test_dial_table_values():
    expected = {
        "save": ("haiku", "low", 1, 1),
        "lean": ("sonnet", "low", 2, 2),
        "balanced": ("sonnet", "medium", 3, 2),
        "strong": ("opus", "high", 4, 3),
        "power": ("opus", "xhigh", 6, 4),
    }
    assert {k: tuple(vars(v).values()) for k, v in config.DIAL.items()} == expected


def test_status_prints_json_and_usage_error(repo, home, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(home))
    assert cli.main(["status", "--json"]) == exits.OK
    payload = json.loads(capsys.readouterr().out)
    assert set(payload) == {"effective", "layers", "relaxations", "load", "tasks"}
    assert set(payload["load"]) == {"slots_total", "slots_in_use", "queue", "loadavg"}
    _write(repo, "flow.toml", 'dial = "turbo"\n')
    assert cli.main(["status"]) == exits.USAGE
    assert "dial" in capsys.readouterr().out
    _write(repo, "flow.toml", "")
    assert cli.main(["status", "some-task"]) == exits.USAGE


def test_scan_allowlist_loss_requires_relax_and_growth_does_not(repo, home):
    _write(repo, "flow.toml", '[brakes]\nscan_allowlists = [".gitleaksignore"]\n')
    with pytest.raises(config.FlowConfigError, match="brakes.scan_allowlists"):
        config.load(repo, home=home)
    _write(repo, "flow.toml", '[brakes]\nscan_allowlists = [".gitleaksignore", ".gitleaks.toml", ".secrets.baseline", "extra"]\n')
    assert config.load(repo, home=home).relaxations == []


@pytest.mark.parametrize("key", ["start", "stop"])
def test_silent_check_in_app_command_rejected(repo, home, key):
    _write(repo, "flow.toml", f'[app]\n{key} = "x 2>/dev/null && y"\n')
    with pytest.raises(config.FlowConfigError, match="silent check"):
        config.load(repo, home=home)


def test_brake_test_patterns_losing_entry_requires_relax(repo, home):
    _write(repo, "flow.toml", '[brakes]\ntest_patterns = ["tests/**"]\n')
    with pytest.raises(config.FlowConfigError, match="brakes.test_patterns"):
        config.load(repo, home=home)


def test_silent_check_reported_before_relax_error(repo, home):
    _write(repo, "flow.toml", '[tests]\nfull = ["pytest"]\n')
    _write(repo, "local.toml", '[flow.tests]\nfull = ["pytest >/dev/null && echo ok"]\n')
    with pytest.raises(config.FlowConfigError, match="silent check"):
        config.load(repo, home=home)


def test_default_roles_follow_the_spec_and_never_use_codex(tmp_path):
    from sakura.flow import config

    roles = config.DEFAULTS["roles"]
    assert roles == {"planner": "claude:opus", "plan_reviewer": "claude:sonnet", "builder": "claude:sonnet",
                     "code_reviewer": "claude:sonnet", "evaluator": "claude:opus", "reporter": "claude:sonnet"}


def test_surface_defaults_and_overrides(repo, home):
    assert config.load(repo, home=home).values["surface"] == {
        "tmux": True, "session_prefix": "flow-", "cockpit_session": "flow"}
    engine_home.config_file().write_text('[surface]\ntmux = false\nsession_prefix = "w."\n')
    values = config.load(repo, home=home).values["surface"]
    assert values["tmux"] is False and values["session_prefix"] == "w."


@pytest.mark.parametrize("text", ['tmux = "yes"', 'session_prefix = "a b"', 'session_prefix = "x;rm"',
                                  'session_prefix = ""', 'cockpit_session = "a:b"', 'cockpit_session = ""',
                                  'session_prefix = 3'])
def test_surface_refuses_unusable_values(repo, home, text):
    _write(repo, "flow.toml", f"[surface]\n{text}\n")
    with pytest.raises(config.FlowConfigError, match="surface"):
        config.load(repo, home=home)


def test_budget_routes_default_table(repo, home):
    routes = config.load(repo, home=home).values["budget"]["routes"]
    assert routes == {"mechanical": "haiku", "integration": "sonnet", "design": "sonnet:high", "reviewer": "sonnet",
                      "reporter": "sonnet", "intake": "haiku"}


# -- the task branch a repository layer names ----------------------------------------------------------

@pytest.mark.parametrize("template", ["feat/flow-{task}", "{task}-work", "team/{task}"])
def test_TaskBranch_FromARepositoryLayer_WithThePlaceholderThatCannotReachAProtectedName_Loads(repo, home, template):
    _write(repo, "flow.toml", f'[workspace]\nbranch = "{template}"\n')
    assert config.load(repo, home=home).values["workspace"]["branch"] == template


@pytest.mark.parametrize("template", ["main", "feat/fixed-branch", "{task}", "ma{task}", "{task}in"])
def test_TaskBranch_FromARepositoryLayer_WithoutThePlaceholderOrThatCanRenderToAProtectedName_IsRefused(repo, home, template):
    _write(repo, "flow.toml", f'[workspace]\nbranch = "{template}"\n')
    with pytest.raises(config.FlowConfigError, match="workspace.branch"):
        config.load(repo, home=home)


def test_TaskBranch_ThatCanRenderToTheIntegrationBranch_IsRefused(repo, home):
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / "profile.toml").write_text('integration_branch = "release/next"\n')
    _write(repo, "flow.toml", '[workspace]\nbranch = "release/{task}"\n')
    with pytest.raises(config.FlowConfigError, match="release/next"):
        config.load(repo, home=home)


def test_TaskBranch_FromAHostLayer_IsNotChecked(repo, home):
    assert config.load(repo, home=home, overrides={"workspace": {"branch": "main"}}).values["workspace"]["branch"] == "main"


def test_TaskBranch_ThatCanRenderToAProtectedBranchOfTheProfile_IsRefused(repo, home):
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / "profile.toml").write_text('protected_branches = ["main", "master", "release"]\n')
    _write(repo, "flow.toml", '[workspace]\nbranch = "rel{task}"\n')
    with pytest.raises(config.FlowConfigError, match="'release'"):
        config.load(repo, home=home)


def test_TaskBranch_ARepositoryLayerChoosingAnotherIntegrationBranch_CannotUnprotectTheHostsOne(repo, home):
    personal = home / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True, exist_ok=True)
    personal.write_text('integration_branch = "develop"\n')
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / "profile.toml").write_text('integration_branch = "dev"\n')
    _write(repo, "flow.toml", '[workspace]\nbranch = "deve{task}"\n')
    with pytest.raises(config.FlowConfigError, match="'develop'"):
        config.load(repo, home=home)


def test_TaskBranch_ThatCanRenderToAProtectedBranchOnlyTheHostProfileNames_IsRefused(repo, home):
    personal = home / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True, exist_ok=True)
    personal.write_text('protected_branches = ["main", "master", "release/stable"]\n')
    _write(repo, "flow.toml", '[workspace]\nbranch = "release/{task}"\n')
    with pytest.raises(config.FlowConfigError, match="'release/stable'"):
        config.load(repo, home=home)


# -- command values hold no control characters ----------------------------------------------------------------

@pytest.mark.parametrize("text", ['[tests]\nfull = ["make check\\u001b[2J"]\n', '[workspace]\nsetup = ["make\\ndeps"]\n',
                                  '[app]\nstart = "serve\\u0007"\n', '[tests]\nfocused = ["make\\ttest"]\n',
                                  '[app]\nstop = "stop\\u007f"\n'])
def test_CommandValue_WithAControlCharacter_IsRefused(repo, home, text):
    _write(repo, "flow.toml", text)
    with pytest.raises(config.FlowConfigError, match="control character"):
        config.load(repo, home=home)


@pytest.mark.parametrize("text", ['[stack]\nup = "docker compose up\\u001b]0;x\\u0007"\n',
                                  '[checks]\napproved = ["npm\\rtest"]\n'])
def test_HostCommandValue_WithAControlCharacter_IsRefusedToo(repo, home, text):
    engine_home.config_file().write_text(text)
    with pytest.raises(config.FlowConfigError, match="control character"):
        config.load(repo, home=home)


# -- review 12, finding 1: where a contract flow may live is the host's ------------------------------------------------

def test_ContractFlowPaths_DefaultToTheEnginesRootedFolders(repo, home):
    assert config.load(repo, home=home).values["contract"]["flow_paths"] == ["tests/**", "test/**", "e2e/**"]


def test_ContractFlowPaths_ARepositoryLayerCannotSetThem(repo, home):
    _write(repo, "flow.toml", '[contract]\nflow_paths = ["**"]\n')
    with pytest.raises(config.FlowConfigError, match=r"^contract.flow_paths may only be set by the host"):
        config.load(repo, home=home)


def test_ContractFlowPaths_ARepositoryWideningTestPatterns_DoesNotWidenThem(repo, home):
    widened = [*config.load(repo, home=home).values["brakes"]["test_patterns"], "**"]  # a superset: a tightening
    _write(repo, "flow.toml", f"[brakes]\ntest_patterns = {widened!r}\n".replace("'", '"'))
    values = config.load(repo, home=home).values
    assert values["brakes"]["test_patterns"] == widened
    assert values["contract"]["flow_paths"] == ["tests/**", "test/**", "e2e/**"]


@pytest.mark.parametrize("pattern", ["**", "**/*.py", "*/x", "/tests/**", "tests", "../x/**"])
def test_ContractFlowPaths_TheHostsPatternsMustBeRooted(repo, home, pattern):
    _personal(home, f'[contract]\nflow_paths = ["{pattern}"]\n')
    with pytest.raises(config.FlowConfigError, match="contract.flow_paths"):
        config.load(repo, home=home)


def test_ContractFlowPaths_TheHostMayExtendThem(repo, home):
    _personal(home, '[contract]\nflow_paths = ["tests/**", "spec/**"]\n')
    assert config.load(repo, home=home).values["contract"]["flow_paths"] == ["tests/**", "spec/**"]


def test_config_cache_keeps_parsed_layers_private_and_refreshes_same_size_replacements(repo, monkeypatch):
    import os

    path = engine_home.config_file()
    initial = "[load]\nslots = 2\n"
    path.write_text(initial)
    original = config.tomllib.loads
    parses = []

    def record(text, *args, **kwargs):
        if text == initial:
            parses.append(text)
        return original(text, *args, **kwargs)

    monkeypatch.setattr(config.tomllib, "loads", record)
    first = config.load(repo)
    first.values["load"]["slots"] = 99
    assert config.load(repo).values["load"]["slots"] == 2
    assert parses == [initial]

    stamp = path.stat()
    replacement = path.with_suffix(".new")
    replacement.write_text(initial.replace("2", "3"))
    os.utime(replacement, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    replacement.replace(path)
    assert config.load(repo).values["load"]["slots"] == 3

    path.write_text(initial.replace("2", "4"))
    os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    assert config.load(repo).values["load"]["slots"] == 4
    path.unlink()
    assert config.load(repo).values["load"]["slots"] == config.defaults().values["load"]["slots"]
