"""Check actual command boundaries without evaluating shell input."""

import pytest

from sakura.quota_enforcement import hooks
from test_quota_enforcement import ENV, StubAdvisor, advice


def codex_advisor(**changes):
    response = advice()
    response.update(agent="codex", model="gpt-6.1-sol", config_dir="/home/test/.codex", **changes)
    return StubAdvisor(response)


def bash(command):
    return {"cwd": "/repo", "tool_name": "Bash", "tool_input": {"command": command}}


@pytest.mark.parametrize("command", [
    'codex exec -m gpt-6.1-sol "work"',
    'cd /repo && /usr/local/bin/codex exec --model=gpt-6.1-sol "work"',
    'env CODEX_HOME=/home/test/.codex codex exec -m gpt-6.1-sol "work"',
    'CODEX_HOME=/home/test/.codex codex -m gpt-6.1-sol exec "work"',
])
def test_codex_advised_model_can_proceed(command):
    advisor = codex_advisor()
    assert hooks.evaluate("PreToolUse", bash(command), advisor, ENV) is None
    assert advisor.requests[0]["agent"] == "codex"


@pytest.mark.parametrize("command", [
    'codex exec -m expensive "work"',
    'codex exec "work"',
    'codex exec -m gpt-6.1-sol "one"; codex exec -m expensive "two"',
    'CODEX_HOME=/other codex exec -m gpt-6.1-sol "work"',
])
def test_codex_wrong_model_or_account_is_denied(command):
    result = hooks.evaluate("PreToolUse", bash(command), codex_advisor(), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "gpt-6.1-sol" in result["hookSpecificOutput"]["permissionDecisionReason"]


@pytest.mark.parametrize("verdict", ["deny", "wait"])
def test_codex_denied_by_service_even_with_matching_model(verdict):
    result = hooks.evaluate("PreToolUse", bash('codex exec -m gpt-6.1-sol "work"'), codex_advisor(verdict=verdict), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


@pytest.mark.parametrize("command", ['echo "codex exec -m expensive"', 'git status', 'echo codex exec -m expensive', 'codex --version'])
def test_non_launch_commands_are_untouched(command):
    assert hooks.evaluate("PreToolUse", bash(command), object(), ENV) is None


@pytest.mark.parametrize("command", [
    'codex exec -C /other -m gpt-6.1-sol task',
    'codex --cd=/other exec -m gpt-6.1-sol task',
    'cd /other && codex exec -m gpt-6.1-sol task',
    'cd nested; codex exec -m gpt-6.1-sol task',
])
def test_codex_advice_uses_effective_repository(command):
    advisor = codex_advisor()
    hooks.evaluate("PreToolUse", bash(command), advisor, ENV)
    expected = "/repo/nested" if "nested" in command else "/other"
    assert advisor.requests[0]["repo"] == expected


@pytest.mark.parametrize("command", [
    'export CODEX_HOME=/other; codex exec -m gpt-6.1-sol task',
    'CODEX_HOME=/other; codex exec -m gpt-6.1-sol task',
    'env -u CODEX_HOME CODEX_HOME=/other codex exec -m gpt-6.1-sol task',
])
def test_codex_account_changes_are_enforced(command):
    result = hooks.evaluate("PreToolUse", bash(command), codex_advisor(), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


@pytest.mark.parametrize("separator", [";\n", "&&\n", "\n", ";\n\n"])
def test_codex_launch_after_multiline_separator_is_checked(separator):
    result = hooks.evaluate("PreToolUse", bash('echo prep' + separator + 'codex exec -m expensive task'), codex_advisor(), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_unset_codex_home_restores_default_account():
    env = {**ENV, "CODEX_HOME": "/other"}
    result = hooks.evaluate("PreToolUse", bash('unset CODEX_HOME; codex exec -m gpt-6.1-sol task'), codex_advisor(), env)
    assert result is None


def test_command_local_account_does_not_leak_to_next_launch():
    command = 'CODEX_HOME=/home/test/.codex codex exec -m gpt-6.1-sol one; codex exec -m gpt-6.1-sol two'
    result = hooks.evaluate("PreToolUse", bash(command), codex_advisor(), {**ENV, "CODEX_HOME": "/other"})
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_quoted_literal_environment_variable_cannot_authorize_another_account():
    command = "CODEX_HOME='$HOME/.codex' codex exec -m gpt-6.1-sol task"
    result = hooks.evaluate("PreToolUse", bash(command), codex_advisor(), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_dynamic_directory_requires_an_explicit_relaunch():
    command = 'cd "$DESTINATION" && codex exec -m gpt-6.1-sol task'
    result = hooks.evaluate("PreToolUse", bash(command), codex_advisor(), {**ENV, "DESTINATION": "/other"})
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_relative_account_path_requires_an_absolute_relaunch(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    response = advice()
    response.update(agent="codex", model="gpt-6.1-sol", config_dir=str(tmp_path / 'profile'))
    result = hooks.evaluate("PreToolUse", bash('cd /other; CODEX_HOME=profile codex exec -m gpt-6.1-sol task'), StubAdvisor(response), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"
