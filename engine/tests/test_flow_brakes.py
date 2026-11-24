import json
import os
import shlex
import stat
from pathlib import Path

import pytest

from sakura import engine_home
from sakura import agent_hooks
from sakura.flow import brakes
from sakura.flow.store import Ruling, TaskStore
from flow_support import make_context

ENV = {"SAKURA_FLOW_TASK": "demo"}


def test_decision_denies_pre_tool_use_when_stopped(repo):
    ctx = make_context(repo)
    ctx.store.stop_file.write_text("now operator\n")
    assert brakes.decision(repo, "PreToolUse", {}, ENV) == ("deny", "task demo stopped by operator")


def test_decision_allows_without_stop_file(repo):
    make_context(repo)
    assert brakes.decision(repo, "PreToolUse", {}, ENV) is None


def test_decision_ignores_sessions_outside_a_flow(repo):
    ctx = make_context(repo)
    ctx.store.stop_file.write_text("now operator\n")
    assert brakes.decision(repo, "PreToolUse", {}, {}) is None


def test_decision_ignores_missing_task_dir(repo):
    assert brakes.decision(repo, "PreToolUse", {}, {"SAKURA_FLOW_TASK": "ghost"}) is None


def test_decision_only_applies_to_pre_tool_use(repo):
    ctx = make_context(repo)
    ctx.store.stop_file.write_text("now operator\n")
    assert brakes.decision(repo, "PostToolUse", {}, ENV) is None


def test_hook_denies_when_stopped(repo, monkeypatch):
    ctx = make_context(repo)
    ctx.store.stop_file.write_text("now operator\n")
    monkeypatch.setenv("SAKURA_FLOW_TASK", "demo")
    payload = {"tool_name": "Read", "tool_input": {}, "session_id": "s"}
    output = agent_hooks._dispatch(repo, "PreToolUse", payload, "claude")
    assert output["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "stopped" in output["hookSpecificOutput"]["permissionDecisionReason"]


# -- engine-session brakes (Task 9) ---------------------------------------

BUILDER = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "builder", "SAKURA_FLOW_PHASE": "build"}


def _flow(repo, protected=("tests/test_app.py",)):
    ctx = make_context(repo, state={"protected_paths": list(protected), "overrides": {}})
    (repo / "tests").mkdir(exist_ok=True)
    (repo / "tests/test_app.py").write_text("def test_a():\n    run(timeout=5)\n")
    return ctx


def _payload(tool, **tool_input):
    return {"session_id": "s1", "tool_name": tool, "tool_input": tool_input}


def _edit(repo, path, old, new):
    return _payload("Edit", file_path=str(repo / path), old_string=old, new_string=new)


def _denied(repo, payload, env=BUILDER):
    result = brakes.decision(repo, "PreToolUse", payload, env)
    return result[1] if result and result[0] == "deny" else None


def _ruling(path, kind, decision="grant"):
    return Ruling(path, kind, decision, "because", "a hang surfaces late", False)


def test_builder_edit_to_protected_test_denied(repo):
    _flow(repo)
    assert "read-only for builders" in _denied(repo, _edit(repo, "tests/test_app.py", "a", "b"))


def test_builder_edit_to_protected_test_allowed_with_ruling(repo):
    ctx = _flow(repo)
    ctx.store.add_ruling(_ruling("tests/test_app.py", "test_change"))
    assert _denied(repo, _edit(repo, "tests/test_app.py", "a", "b")) is None


def test_timeout_ruling_does_not_cover_mixed_assertion_change(repo):
    ctx = _flow(repo)
    ctx.store.add_ruling(_ruling("tests/test_app.py", "timeout"))
    old = "def test_a():\n    run(timeout=5)\n    assert result == expected\n"
    new = "def test_a():\n    run(timeout=30)\n    assert True\n"

    reason = _denied(repo, _edit(repo, "tests/test_app.py", old, new))

    assert reason and "tests/test_app.py" in reason


def test_builder_new_test_file_allowed(repo):
    _flow(repo)
    payload = _payload("Write", file_path=str(repo / "tests/test_new.py"), content="def test_n():\n    pass\n")
    assert _denied(repo, payload) is None


def test_manual_session_edit_allowed(repo):
    _flow(repo)
    assert brakes.decision(repo, "PreToolUse", _edit(repo, "tests/test_app.py", "a", "b"), {}) is None


def test_non_builder_role_may_edit_tests(repo):
    _flow(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "planner"}
    assert _denied(repo, _edit(repo, "tests/test_app.py", "a", "b"), env) is None


def test_apply_patch_paths_checked(repo):
    _flow(repo)
    patch = "*** Begin Patch\n*** Update File: tests/test_app.py\n@@\n-a\n+b\n*** End Patch\n"
    assert "read-only" in _denied(repo, _payload("apply_patch", command=patch))
    delete = "*** Begin Patch\n*** Delete File: tests/test_app.py\n*** End Patch\n"
    assert "read-only" in _denied(repo, _payload("apply_patch", command=delete))


def test_multi_edit_and_notebook_paths_checked(repo):
    _flow(repo)
    multi = _payload("MultiEdit", file_path=str(repo / "tests/test_app.py"), edits=[{"old_string": "a", "new_string": "b"}])
    assert "read-only" in _denied(repo, multi)
    assert "read-only" in _denied(repo, _payload("NotebookEdit", notebook_path=str(repo / "tests/test_app.py"), new_source="x"))


def test_bash_redirect_into_contract_denied(repo):
    ctx = _flow(repo, protected=("tests/test_app.py", "ignored"))
    ctx.store.update_state(lambda state: state["protected_paths"].append(str(ctx.store.contract)))
    command = f"echo x > {ctx.store.contract}"
    assert _denied(repo, _payload("Bash", command=command))
    assert "read-only" in _denied(repo, _payload("Bash", command="sed -i s/a/b/ tests/test_app.py"))


def test_engine_owned_files_denied_for_every_role(repo):
    ctx = _flow(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "evaluator", "SAKURA_FLOW_PHASE": "evaluate"}
    for name in ("state.json", "rulings.json", "contract.json", "pr-body.md", "report.md"):
        payload = _payload("Write", file_path=str(ctx.store.root / name), content="x")
        assert "operator state" in _denied(repo, payload, env), name


def test_planner_writes_the_contract_in_its_exchange_folder_never_in_the_task_folder(repo):
    ctx = _flow(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "planner", "SAKURA_FLOW_PHASE": "contract"}
    exchanged = repo / ".sakura-role" / "contract-0123456789ab" / "contract.json"
    assert _denied(repo, _payload("Write", file_path=str(exchanged), content="{}"), env) is None
    assert "operator state" in _denied(repo, _payload("Write", file_path=str(ctx.store.contract), content="{}"), env)


def test_builder_edit_to_sakura_directory_denied(repo):
    _flow(repo)
    assert "read-only" in _denied(repo, _edit(repo, ".sakura/flow.toml", "a", "b"))


def test_timeout_raise_denied_without_ruling(repo):
    _flow(repo)
    assert "timeout" in _denied(repo, _edit(repo, "tests/test_app.py", "timeout=5", "timeout=30"))


def test_timeout_raise_allowed_with_ruling(repo):
    ctx = _flow(repo)
    ctx.store.add_ruling(_ruling("tests/test_app.py", "timeout"))
    assert _denied(repo, _edit(repo, "tests/test_app.py", "timeout=5", "timeout=30")) is None
    assert _denied(repo, _edit(repo, "tests/test_app.py", "run", "other")) is not None


def test_timeout_lowered_is_allowed_on_an_unprotected_test(repo):
    _flow(repo, protected=())
    assert _denied(repo, _edit(repo, "tests/test_app.py", "timeout=5", "timeout=3")) is None
    assert _denied(repo, _edit(repo, "tests/test_app.py", "timeout=5", "timeout=9")) is not None


def test_retry_wrapper_added_denied(repo):
    _flow(repo, protected=())
    payload = _payload("Write", file_path=str(repo / "tests/test_app.py"), content="@pytest.mark.flaky\ndef test_a():\n    pass\n")
    assert "retry" in _denied(repo, payload)


def test_write_compares_with_the_file_on_disk(repo):
    _flow(repo, protected=())
    grown = "def test_a():\n    run(timeout=50)\n"
    assert _denied(repo, _payload("Write", file_path=str(repo / "tests/test_app.py"), content=grown))


def test_allowlist_edit_needs_ruling(repo):
    ctx = _flow(repo)
    payload = _edit(repo, ".gitleaksignore", "", "fingerprint")
    assert "allowlist" in _denied(repo, payload)
    ctx.store.add_ruling(_ruling(".gitleaksignore", "allowlist"))
    assert _denied(repo, payload) is None


def test_claim_write_needs_evidence_read(repo):
    ctx = _flow(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "evaluator", "SAKURA_FLOW_PHASE": "evaluate"}
    evidence = ctx.store.evidence_dir / "R1.txt"
    evidence.parent.mkdir(parents=True)
    evidence.write_text("proof")
    claim = _payload("Write", file_path=str(repo / "progress.md"), content="R1 PASS\n")
    assert "read an evidence file before claiming" in _denied(repo, claim, env)
    assert _denied(repo, _payload("Read", file_path=str(evidence)), env) is None
    assert _denied(repo, claim, env) is None


def test_cat_of_evidence_counts_as_a_read(repo):
    ctx = _flow(repo)
    ctx.store.evidence_dir.mkdir(parents=True)
    (ctx.store.evidence_dir / "R1.txt").write_text("proof")
    claim = _payload("Edit", file_path=str(repo / "progress.md"), old_string="", new_string="task done")
    assert _denied(repo, claim)
    assert "operator state" in _denied(repo, _payload("Bash", command=f"cat {ctx.store.evidence_dir}/R1.txt"))
    exchanged = repo / ".sakura-role" / "evaluate-0123456789ab" / "evidence" / "R1.txt"
    exchanged.parent.mkdir(parents=True)
    exchanged.write_text("proof")
    assert _denied(repo, claim)
    assert _denied(repo, _payload("Bash", command=f"cat {exchanged}")) is None
    assert _denied(repo, claim) is None


def test_claim_text_without_a_claim_word_is_allowed(repo):
    _flow(repo)
    assert _denied(repo, _payload("Write", file_path=str(repo / "progress.md"), content="working on R1\n")) is None


def test_json_passes_true_is_a_claim(repo):
    _flow(repo)
    verdict = repo / ".sakura-role" / "evaluate-0123456789ab" / "verdict.json"
    assert "evidence" in _denied(repo, _payload("Write", file_path=str(verdict), content='{"passes": true}'))


def test_ruling_roundtrip_and_ledger_line(repo):
    ctx = _flow(repo)
    ctx.store.add_ruling(_ruling("tests/test_app.py", "test_change"))
    assert json.loads(ctx.store.rulings.read_text())["rulings"][0]["cost_if_wrong"] == "a hang surfaces late"
    assert ctx.store.granted("tests/test_app.py", "test_change")
    assert not ctx.store.granted("tests/test_app.py", "timeout")
    ctx.store.add_ruling(_ruling("tests/test_app.py", "test_change", "deny"))
    assert not ctx.store.granted("tests/test_app.py", "test_change")
    lines = [line for line in ctx.store.ledger.read_text().splitlines() if "Ruling:" in line]
    assert lines[0].lstrip("-* ") == "Ruling: grant test_change tests/test_app.py, because, cost if wrong: a hang surfaces late"


OPERATOR_VERBS = ("dial", "role", "steer", "approve", "stop", "start", "resume", "recontract", "cleanup", "open",
                  "attach", "migrate-state")
READ_ONLY_VERBS = ("status", "sessions", "events", "watch", "report")
OPERATOR_FORMS = [
    "sakura flow {verb} demo x",
    "env -u SAKURA_FLOW_ROLE sakura flow {verb} demo x",
    "env -i sakura flow {verb} demo x",
    "env FOO=x sakura flow {verb} demo x",
    "SAKURA_FLOW_ROLE= sakura flow {verb} demo x",
    "command sakura flow {verb} demo x",
    "/home/someone/.local/bin/sakura flow {verb} demo x",
    "env -u SAKURA_FLOW_ROLE /opt/sakura/bin/sakura flow {verb} demo x",
    "true && sakura flow {verb} demo x",
    "echo hi; sakura flow {verb} demo x",
    "ls | sakura flow {verb} demo x",
    "false || env -i sakura flow {verb} demo x",
    "sakura flow {verb} demo x &",
    "echo hi\nsakura flow {verb} demo x",
    "(sakura flow {verb} demo x)",
    "echo $(sakura flow {verb} demo x)",
    "bash -c 'sakura flow {verb} demo x'",
    "sh -lc \"env -i sakura flow {verb} demo x\"",
    "eval sakura flow {verb} demo x",
]


def _bash(command):
    return _payload("Bash", command=command)


@pytest.mark.parametrize("role", ["builder", "evaluator", "planner"])
@pytest.mark.parametrize("form", OPERATOR_FORMS)
def test_brake_denies_operator_verbs_in_role_sessions(repo, form, role):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": role}
    for verb in OPERATOR_VERBS:
        reason = _denied(repo, _bash(form.format(verb=verb)), env)
        assert reason and "operator" in reason and verb in reason


LANE_WRITERS = ("open", "done", "say", "model")
LANE_FORMS = [form.replace("flow {verb}", "lane {verb}") for form in OPERATOR_FORMS]
MODE_VALUES = ("day", "night", "auto")
MODE_FORMS = [form.replace("{verb} demo x", "mode {value}") for form in OPERATOR_FORMS]


@pytest.mark.parametrize("role", ["builder", "planner"])
@pytest.mark.parametrize("form", LANE_FORMS)
def test_brake_denies_lane_writers_in_role_sessions(repo, form, role):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": role}
    for verb in LANE_WRITERS:
        reason = _denied(repo, _bash(form.format(verb=verb)), env)
        assert reason and "operator" in reason and f"lane {verb}" in reason


COCKPIT_FORMS = [form.replace("flow {verb}", "cockpit {verb}") for form in OPERATOR_FORMS]


@pytest.mark.parametrize("role", ["builder", "planner"])
@pytest.mark.parametrize("form", COCKPIT_FORMS)
def test_brake_denies_cockpit_act_in_role_sessions(repo, form, role):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": role}
    reason = _denied(repo, _bash(form.format(verb="act")), env)
    assert reason and "operator" in reason and "cockpit act" in reason


@pytest.mark.parametrize("command", [
    "env -S 'sakura cockpit act s x --confirm y'",
    "a=sakura; $a cockpit act s x",
    "python3 -c \"import os; os.system('sakura cockpit act s x')\"",
])
def test_brake_fails_closed_on_unparsable_cockpit_act(repo, command):
    make_context(repo)
    reason = _denied(repo, _bash(command))
    assert reason and "operator" in reason


@pytest.mark.parametrize("command", [
    "sakura cockpit snapshot --json",
    "sakura cockpit --once",
    "sakura cockpit",
    "echo sakura cockpit act",
    "env -u SAKURA_FLOW_ROLE sakura cockpit snapshot --json | cat",
])
def test_brake_allows_cockpit_reads(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


@pytest.mark.parametrize("form", MODE_FORMS)
def test_brake_denies_the_write_forms_of_flow_mode_in_role_sessions(repo, form):
    make_context(repo)
    for value in MODE_VALUES:
        reason = _denied(repo, _bash(form.format(value=value)))
        assert reason and "operator" in reason and "mode" in reason


@pytest.mark.parametrize("argument", ["$M", "n*", "{night,}", '"$(echo night)"', "$(cat /tmp/m)", "~", "ni?ht"])
@pytest.mark.parametrize("prefix", ["sakura flow mode", "env -u SAKURA_FLOW_ROLE sakura flow mode",
                                    "bash -c 'sakura flow mode", "M=night; sakura flow mode"])
def test_brake_denies_flow_mode_with_any_argument_not_only_literal_values(repo, prefix, argument):
    make_context(repo)
    command = f"{prefix} {argument}" + ("'" if prefix.startswith("bash") else "")
    reason = _denied(repo, _bash(command))
    assert reason and "operator" in reason


@pytest.mark.parametrize("command", [
    "sakura flow mode",
    "env -u SAKURA_FLOW_ROLE sakura flow mode",
    "sakura flow mode | cat",
    "sakura flow mode; echo done",
    "sakura flow mode && sakura flow status demo",
    "(sakura flow mode)",
    "sakura lane status",
    "sakura lane status --json",
    "env -u SAKURA_FLOW_ROLE sakura lane status --group work",
    "sakura lane",
    "echo sakura lane done alpha",
])
def test_brake_allows_bare_mode_and_lane_status(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


@pytest.mark.parametrize("command", [
    "env -S 'sakura flow mode night'",
    "env -S 'sakura flow mode $M'",
    "a=sakura; $a flow mode n*",
    "a=sakura; $a flow mode {night,}",
    "a=sakura; $a flow mode \"$(echo night)\"",
    "a=sakura; $a flow mode day",
    "python3 -c \"import os; os.system('sakura flow mode auto')\"",
    "env -S 'sakura lane done alpha --remove'",
    "a=sakura; $a lane say alpha hi",
    "python3 -c \"import os; os.system('sakura lane model alpha opus')\"",
])
def test_brake_fails_closed_on_unparsable_mode_and_lane_writers(repo, command):
    make_context(repo)
    reason = _denied(repo, _bash(command))
    assert reason and "operator" in reason


@pytest.mark.parametrize("command", [
    *(f"sakura flow {verb} demo" for verb in READ_ONLY_VERBS),
    "sakura flow status demo --json",
    "env -u SAKURA_FLOW_ROLE sakura flow status demo",
    "/home/someone/.local/bin/sakura flow report demo",
    "git status && sakura flow sessions demo",
    "sakura status",
    "sakura flow",
    "echo sakura flow dial demo save",
    "grep 'sakura flow stop' README.md",
    "cat docs/sakura-flow-dial.md",
    "ls | wc -l",
])
def test_brake_allows_read_only_flow_verbs(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


def test_brake_allows_operator_verbs_outside_role_sessions(repo):
    make_context(repo)
    assert _denied(repo, _bash("sakura flow dial demo save"), {"SAKURA_FLOW_TASK": "demo"}) is None


FAIL_CLOSED_FORMS = [
    "env -S 'sakura flow {verb} demo x'",
    "env --unset=SAKURA_FLOW_ROLE sakura flow {verb} demo x",
    "a=sakura; $a flow {verb} demo x",
    "A=agent; B=kit; $A$B flow {verb} demo x",
    "python3 -c \"import os; os.system('sakura flow {verb} demo x')\"",
    "eval eval eval eval eval sakura flow {verb} demo x",
]


@pytest.mark.parametrize("form", FAIL_CLOSED_FORMS)
def test_brake_fails_closed_on_forms_the_parser_cannot_follow(repo, form):
    make_context(repo)
    for verb in OPERATOR_VERBS:
        reason = _denied(repo, _bash(form.format(verb=verb)))
        assert reason and "operator" in reason


@pytest.mark.parametrize("relative", [
    ".local/state/sakura/flow-digests/key.commands",
    ".local/state/sakura/flow/mode",
    ".local/state/sakura/lanes/events.jsonl",
    ".config/sakura/flow.toml",
    ".config/sakura/cockpit.toml",
])
def test_brake_denies_writes_to_operator_state_and_config(repo, relative):
    _flow(repo)
    target = Path.home() / relative
    assert "operator state" in _denied(repo, _payload("Write", file_path=str(target), content="x"))
    assert "operator state" in _denied(repo, _bash(f"echo abc >> {target}"))


@pytest.mark.parametrize("command", [
    "echo night > ~/.local/state/sakura/flow/mode",
    "echo x >> $HOME/.config/sakura/flow.toml",
    "printf x | tee ${HOME}/.local/state/sakura/flow-digests/k.commands",
    "cp /tmp/x ~/.config/sakura/cockpit.toml",
])
def test_brake_denies_operator_state_named_through_home_expansion(repo, command):
    _flow(repo)
    assert "operator state" in _denied(repo, _bash(command))


def test_brake_denies_writes_to_the_engine_home_outside_the_task(repo):
    _flow(repo)
    for target in (engine_home.config_file(), engine_home.project_config_file(repo),
                   engine_home.flow_dir(repo) / "other-task" / "state.json"):
        assert "operator state" in _denied(repo, _payload("Write", file_path=str(target), content="x"))
        assert "operator state" in _denied(repo, _bash(f"echo abc >> '{target}'"))


@pytest.mark.parametrize("command", [
    "echo x >> ~/Library/Application\\ Support/Sakura/engine/config.toml",
    "echo x >> \"$SAKURA_ENGINE_HOME/config.toml\"",
])
def test_brake_denies_the_engine_home_named_through_expansion_or_quoting(repo, command):
    _flow(repo)
    assert "operator state" in _denied(repo, _bash(command))


def test_brake_denies_every_mention_of_the_task_folder_even_for_its_own_role(repo):
    ctx = _flow(repo)
    plan = ctx.store.root / "plan.md"
    planner = {**BUILDER, "SAKURA_FLOW_ROLE": "planner", "SAKURA_FLOW_PHASE": "plan"}
    assert "operator state" in _denied(repo, _payload("Write", file_path=str(plan), content="# Plan\n"), planner)
    for command in (f"cat notes > '{plan}'", f"cat {ctx.store.root}/spec.md", f"ls {ctx.store.root}"):
        assert "operator state" in _denied(repo, _bash(command), planner), command


def test_brake_denies_climbing_out_of_the_task_folder(repo):
    ctx = _flow(repo)
    assert "operator state" in _denied(repo, _bash(f"echo x >> '{ctx.store.root}/../../config.toml'"))


@pytest.mark.parametrize("tail", ["10/state.json", "x/state.json", "-other/ledger.md", "/../other/state.json",
                                  "/sub/../../other/state.json"])
def test_brake_denies_a_sibling_that_shares_the_task_folder_prefix(repo, tail):
    ctx = _flow(repo)
    target = f"{ctx.store.root}{tail}"
    assert "operator state" in _denied(repo, _bash(f"echo x >> '{target}'"))
    assert "operator state" in _denied(repo, _bash(f"echo x >> {target}"))
    assert "operator state" in _denied(repo, _payload("Write", file_path=target, content="x"))


def test_brake_still_sees_the_engine_home_after_the_task_folder_in_one_command(repo):
    ctx = _flow(repo)
    command = f"cat '{ctx.store.root}/plan.md' > '{engine_home.config_file()}'"
    assert "operator state" in _denied(repo, _bash(command))


def test_brake_denies_operator_verbs_through_the_old_command_name(repo):
    _flow(repo)
    for verb in OPERATOR_VERBS:
        reason = _denied(repo, _bash(f"agentkit flow {verb} demo x"))
        assert reason and "operator" in reason


def test_brake_denies_operator_state_through_a_symlinked_home(repo, tmp_path, monkeypatch):
    real = tmp_path / "real-home"
    (real / ".config/sakura").mkdir(parents=True)
    alias = tmp_path / "alias-home"
    alias.symlink_to(real)
    monkeypatch.setenv("HOME", str(alias))
    _flow(repo)
    target = real / ".config/sakura/flow.toml"
    assert "operator state" in _denied(repo, _payload("Write", file_path=str(target), content="x"))


@pytest.mark.parametrize("form", ["sa*ra flow {verb} demo x", "/opt/*/bin/saku?a flow {verb} demo x",
                                  "sakur[a] flow {verb} demo x", "~/bin/sa{{k,}}ura flow {verb} demo x"])
def test_brake_denies_operator_verbs_behind_glob_programs(repo, form):
    make_context(repo)
    for verb in OPERATOR_VERBS:
        reason = _denied(repo, _bash(form.format(verb=verb)))
        assert reason and "operator" in reason


@pytest.mark.parametrize("form", ["nohup sak\"ura\" flow {verb} demo x", "timeout 5 'sakura' fl'ow' {verb} demo x",
                                  "echo demo | xargs sak''ura flow {verb}"])
def test_brake_matches_dequoted_words_behind_wrappers(repo, form):
    make_context(repo)
    for verb in OPERATOR_VERBS:
        quoted = verb[:1] + '""' + verb[1:]
        reason = _denied(repo, _bash(form.format(verb=quoted)))
        assert reason and "operator" in reason


TMUX_FORMS = [
    "tmux kill-server",
    "tmux send-keys -t owner 'x' Enter",
    "/usr/bin/tmux ls",
    "/opt/homebrew/bin/tmux kill-session -t x",
    "env tmux ls",
    "env -u TMUX_TMPDIR tmux ls",
    "env -i tmux ls",
    "env FOO=x tmux ls",
    "FOO=x tmux ls",
    "command tmux ls",
    "true && tmux ls",
    "echo hi; tmux ls",
    "ls | tmux load-buffer -",
    "tmux ls &",
    "(tmux ls)",
    "echo $(tmux ls)",
    "bash -c 'tmux ls'",
    "sh -lc \"env -u TMUX_TMPDIR tmux ls\"",
    "eval tmux ls",
]


@pytest.mark.parametrize("role", ["builder", "evaluator", "planner"])
@pytest.mark.parametrize("command", TMUX_FORMS)
def test_brake_denies_tmux_in_role_sessions(repo, command, role):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": role}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


@pytest.mark.parametrize("agent", ["claude", "codex"])
def test_tmux_is_denied_on_both_hook_paths(repo, monkeypatch, agent):
    make_context(repo)
    for name, value in BUILDER.items():
        monkeypatch.setenv(name, value)
    output = agent_hooks._dispatch(repo, "PreToolUse", _bash("tmux kill-server"), agent)
    assert output["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "tmux" in output["hookSpecificOutput"]["permissionDecisionReason"]


@pytest.mark.parametrize("command", [
    "echo tmux",
    "grep -rn tmux README.md",
    "cat tmux.conf",
    "ls lib/sakura/flow/tmux.py",
    "python3 -m pytest tests/test_flow_tmux.py",
])
def test_brake_allows_commands_that_only_mention_tmux(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


def test_brake_allows_tmux_outside_role_sessions(repo):
    make_context(repo)
    assert _denied(repo, _bash("tmux ls"), {"SAKURA_FLOW_TASK": "demo"}) is None


# -- the tmux brake fails closed like the operator brake: a mention in a command that is not a reader --

TMUX_MENTION_FORMS = [
    "nohup tmux ls",
    "exec tmux ls",
    "timeout 5 tmux ls",
    "nice -n 5 tmux ls",
    "stdbuf -o0 tmux ls",
    "echo ls | xargs tmux",
    "find . -maxdepth 0 -exec tmux kill-server ;",
    "t=tmux; $t kill-server",
    "TMUX ls",
    "nohup t''mux ls",
    "sh -c \"nohup tm''ux ls\"",
    "python3 -c \"import subprocess; subprocess.run(['tmux', 'ls'])\"",
    "perl -e 'system(\"tmux ls\")'",
    "node -e \"require('child_process').execSync('tmux ls')\"",
    "ruby -e 'exec(\"tmux\", \"ls\")'",
    "nc -U /private/tmp/tmux-501/default",
    "socat - UNIX-CONNECT:/tmp/tmux-501/default",
    "rm /tmp/tmux-501/default",
    "ls /tmp/tmux-501 && rm -rf /tmp/tmux-501",
]
TMUX_GLOB_FORMS = [
    "/opt/homebrew/bin/tm*x ls",
    "tm?x kill-server",
    "t[m]ux ls",
    "t{m,}ux ls",
]


@pytest.mark.parametrize("command", TMUX_MENTION_FORMS)
def test_brake_denies_a_tmux_mention_in_a_command_that_is_not_only_readers(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


@pytest.mark.parametrize("command", TMUX_GLOB_FORMS)
def test_brake_denies_a_glob_program_that_can_expand_to_tmux(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


@pytest.mark.parametrize("command", [
    "echo tmux",
    "grep tmux file",
    "grep -rn tmux-501 /tmp",
    "cat /tmp/tmux-501/default",
    "ls /tmp/tmux-501",
    "python3 -m pytest tests/test_flow_tmux.py tests/tmux_support.py",
    "ls tm*x",
])
def test_brake_still_allows_readers_and_names_that_only_contain_tmux(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


# -- no expansion emulation: a program token the shell would expand is tmux-capable ---------------

TMUX_CAPABLE_PROGRAMS = [
    "t{m,x}ux ls",
    "tm?x kill-server",
    "/usr/*/tmux ls",
    "$T kill-server",
    "${T} kill-server",
    "~/bin/tmu* ls",
    "FOO=x $T ls",
    "env FOO=x ~/bin/tmu* ls",
]
BRACE_BOMB = "{a,b}" * 200 + "tmux ls"
ODD_ASSIGNMENTS = [
    "a-b=1 ls",
    "1a=1 ls",
    "FOO+=1 ls",
    "A$B=1 ls",
    "env a-b=1 ls",
]


@pytest.mark.parametrize("command", [*TMUX_CAPABLE_PROGRAMS, BRACE_BOMB, *ODD_ASSIGNMENTS])
def test_brake_denies_a_program_token_the_shell_may_expand_or_misparse(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


def test_brake_denies_a_brace_bomb_without_expanding_it(repo, monkeypatch):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    programs = []
    expands = brakes._program_expands
    assert expands(BRACE_BOMB.split())

    def inspect(tokens):
        programs.append(tokens[0])
        return expands(tokens)

    monkeypatch.setattr(brakes, "_program_expands", inspect)
    reason = _denied(repo, _bash(BRACE_BOMB), env)
    assert reason and "tmux" in reason
    assert BRACE_BOMB.split()[0] in programs
    assert len(programs) <= 4


@pytest.mark.parametrize("command", ["FOO=x ls", "env FOO=x -u BAR ls", "A_1=2 B=3 echo hi", "ls tm*x"])
def test_brake_allows_valid_assignment_prefixes_and_globbed_arguments(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None



# -- a reader that executes or writes is not a plain reader (both brakes) -------------------------

EXEC_OR_REDIRECT_TMUX = [
    "rg --pre 'tmux kill-server' x",
    "rg --pre=tmux x file",
    "rg -n --pre 'tmux ls' x",
    "rg --hostname-bin 'tmux ls' x",
    "echo tmux > f",
    "echo tmux >> f",
    "echo tmux>f",
    "cat < tmux",
    "grep tmux file 2> out",
    "echo tmux &> out",
    "cat <<EOF\ntmux\nEOF",
    "cat <<< tmux",
]
EXEC_OR_REDIRECT_OPERATOR = [
    "rg --pre 'sakura flow stop demo' x",
    "rg --pre='sakura flow stop demo' x file",
    "echo sakura flow stop demo > f",
    "echo sakura flow stop demo >> f",
]


@pytest.mark.parametrize("command", EXEC_OR_REDIRECT_TMUX)
def test_tmux_brake_denies_a_reader_that_executes_or_redirects(repo, command):
    make_context(repo)
    reason = _denied(repo, _bash(command))
    assert reason and "tmux" in reason


@pytest.mark.parametrize("command", EXEC_OR_REDIRECT_OPERATOR)
def test_operator_brake_denies_a_reader_that_executes_or_redirects(repo, command):
    make_context(repo)
    reason = _denied(repo, _bash(command))
    assert reason and "operator" in reason


@pytest.mark.parametrize("command", ["grep tmux file", "echo tmux", "rg -n tmux lib", "echo sakura flow stop demo",
                                     "grep -rn 'sakura flow stop' docs"])
def test_plain_readers_that_only_mention_a_word_stay_allowed(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


# -- a file name with an extension is not the program ---------------------------------------------

@pytest.mark.parametrize("command", [
    "git add lib/sakura/flow/tmux.py",
    "sed -i 's/a/b/' lib/sakura/flow/tmux.py",
    "git add tmux.conf tests/test_flow_tmux.py",
    "git mv lib/sakura/flow/tmux.py lib/sakura/flow/tmux_old.py",
])
def test_tmux_brake_allows_commands_that_only_name_a_tmux_file(repo, command):
    _flow(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


@pytest.mark.parametrize("command", [
    "python3 -c \"print('tmux')\"",
    "git add tmux.py; tmux ls",
    "git add tmux.py && python3 -c 'import os; os.system(\"tmux ls\")'",
    "git add /tmp/tmux-501/default",
    "nc -U /tmp/tmux-501/default",
    "git add tmux. x",
    "git add tmux.9 x",
    "git commit -m tmux",
])
def test_tmux_brake_still_denies_the_word_when_it_is_not_a_file_name(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


# -- plain variable and substitution arguments are not programs; code text and backticks are ------

@pytest.mark.parametrize("command", [
    "echo $HOME",
    "ls $HOME",
    "git add $F",
    "git add $FILES",
    "cd $(pwd)",
    "cd $(git rev-parse --show-toplevel)",
    "make -j$(nproc)",
    "echo \"$PWD\"",
    "echo `date`",
    "echo `printf 'tm%sx' u`",
    "ls `pwd`",
])
def test_tmux_brake_allows_plain_variable_and_substitution_arguments(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


@pytest.mark.parametrize("command", [
    "bash -c '$T ls'",
    "eval \"$T ls\"",
    "eval $T",
    "sh -c \"$CMD\"",
    "echo hi; bash -c '${T} ls'",
    "`printf 'tm%sx' u` ls",
    "`printf 'tm%sx' u`",
    "env `printf 'tm%sx' u` ls",
    "FOO=x `printf 'tm%sx' u` ls",
    "true && `printf 'tm%sx' u` ls",
    "echo hi; `printf 'tm%sx' u` ls",
    "bash -c '`printf tm%sx u` ls'",
    "`printf tm%sx u ls",
    "echo \"`tmux ls`\"",
    "$(printf 'tm%sx' u) ls",
])
def test_tmux_brake_denies_expanding_programs_in_code_text_and_backticks(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


# -- a substitution body is code, quoted or not --------------------------------------------------

SUBSTITUTION_CODE_FORMS = [
    'echo "$(${T}ux ls)"',
    "cd \"$($(printf 'tm%sx' u) ls)\"",
    'echo "$(tmux ls)"',
    "x=`$T ls`",
    'echo "`$T ls`"',
    'echo "a $(echo $(${T}ux ls)) b"',
    "X=$($T kill-server) true",
    'X="$($T kill-server)" true',
    "env X=`$T ls` true",
    'env X="`$T ls`" true',
    'X="$(${T}ux ls)" make',
    '"$($T ls)"',
]


@pytest.mark.parametrize("command", SUBSTITUTION_CODE_FORMS)
def test_tmux_brake_denies_a_command_that_hides_in_a_substitution_body(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


@pytest.mark.parametrize("command", [
    "echo $HOME",
    "git add $F",
    "cd $(git rev-parse --show-toplevel)",
    "make -j$(nproc)",
    'echo "$PWD"',
    'echo "$(date) ${HOME}"',
    "X=$(pwd) make",
    'X="$(pwd)" make',
    'env X="$(pwd)" make',
])
def test_tmux_brake_allows_values_and_substitutions_that_run_other_programs(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


def test_tmux_brake_denies_a_substitution_nested_deeper_than_it_inspects(repo):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    command = 'echo "$(date)"'
    for _ in range(brakes.MAX_NESTING):
        command = f"bash -c {shlex.quote(command)}"
    reason = _denied(repo, _bash(command), env)
    assert reason and "nesting too deep" in reason


# -- `[` and `[[` are the test builtin, not a glob --------------------------------------------------

@pytest.mark.parametrize("command", ["[ -f x ] && echo hi", "[[ -f x ]]", "if [ -f x ]; then echo hi; fi"])
def test_tmux_brake_allows_the_test_builtin_as_a_program(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


# -- a program path with a leading ~/ and a literal rest is not an expanding program ---------------

@pytest.mark.parametrize("command", ["~/bin/tool", "FOO=x ~/bin/tool", "~/.venv/bin/pytest -q", "/usr/local/bin/tool --flag"])
def test_tmux_brake_allows_a_program_path_that_is_literal_after_a_home_prefix(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    assert _denied(repo, _bash(command), env) is None


@pytest.mark.parametrize("command", ["${X/y/tm}ux ls", "$(echo /usr/bin)/tmux ls", "$HOME/tmu* ls", "~/bin/$T ls",
                                     "${HOME}/$(echo tmux) ls", "$HOME/.venv/bin/pytest -q", "${HOME}/bin/tool", "$X/y ls",
                                     "~/bin/tmu* ls"])
def test_tmux_brake_denies_a_program_path_with_an_expansion_after_the_home_prefix(repo, command):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    reason = _denied(repo, _bash(command), env)
    assert reason and "tmux" in reason


# -- the hook mark directories are private to the operator ----------------------------------------

def test_mark_hook_ran_creates_the_mark_directories_owner_only(repo):
    make_context(repo)
    old_umask = os.umask(0o022)
    try:
        brakes.mark_hook_ran(repo, {**BUILDER, "SAKURA_FLOW_RUN": "build-1"})
    finally:
        os.umask(old_umask)
    mark = TaskStore(repo, "demo").hook_mark("build-1")
    assert mark.exists()
    assert stat.S_IMODE(mark.parent.stat().st_mode) == 0o700
    assert stat.S_IMODE(mark.parent.parent.stat().st_mode) == 0o700


@pytest.mark.parametrize("after", ["\\\\", "'", '"', "{", "}", "$", "*", "?"])
def test_brake_fails_closed_on_shell_syntax_right_after_the_task_folder(repo, after):
    ctx = _flow(repo)
    for command in (f"echo x >> {ctx.store.root}{after}/state.json", f"echo x >> {ctx.store.root}/a{after}b/state.json"):
        assert "operator state" in _denied(repo, _bash(command)), command


@pytest.mark.parametrize("component", [".", "..", "...", "...."])
def test_brake_fails_closed_on_a_component_made_only_of_dots(repo, component):
    ctx = _flow(repo)
    command = f"echo x >> {ctx.store.root}/runs/{component}/state.json"
    assert "operator state" in _denied(repo, _bash(command))


@pytest.mark.parametrize("template", ["cat > '{root}/plan.md", "cat > '{root}/plan.md x'y", 'cat > "{root}/a" b"',
                                      "cat > '{root}'/../config.toml"])
def test_brake_denies_a_quote_opened_before_the_task_folder_and_closed_elsewhere(repo, template):
    ctx = _flow(repo)
    assert "operator state" in _denied(repo, _bash(template.format(root=ctx.store.root)))


OUTSIDE_FORMS = [form.replace("flow {verb}", "outside {verb}") for form in OPERATOR_FORMS]


@pytest.mark.parametrize("role", ["builder", "planner"])
@pytest.mark.parametrize("form", OUTSIDE_FORMS)
def test_Brakes_EngineRoleRunsSakuraOutsideOpen_Denied(repo, form, role):
    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": role}
    for verb in ("open", "row", "ask", "close", "inbox"):
        reason = _denied(repo, _bash(form.format(verb=verb)), env)
        assert reason and "operator" in reason and f"outside {verb}" in reason


@pytest.mark.parametrize("command", [
    "sakura outside list",
    "sakura outside show demo",
    "echo sakura outside open",
])
def test_Brakes_EngineRoleReadsOutsideLanes_Allowed(repo, command):
    make_context(repo)
    assert _denied(repo, _bash(command)) is None


@pytest.mark.parametrize("form", [form.replace("flow {verb}", "master {verb}") for form in OPERATOR_FORMS])
def test_Brakes_EngineRoleRunsSakuraMasterSpec_Denied(repo, form):
    make_context(repo)
    reason = _denied(repo, _bash(form.format(verb="spec")), {**BUILDER, "SAKURA_FLOW_ROLE": "builder"})
    assert reason and "operator" in reason and "master spec" in reason
