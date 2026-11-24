"""Contract-level enforcement tests with a local advisor double."""

from pathlib import Path

import pytest

from sakura.quota_enforcement import hooks


def advice(**changes):
    return dict(verdict="go", agent="claude", model="sonnet", effort="medium",
                config_dir="/profiles/current", reason="within quota", retry_at=None, **changes)


class StubAdvisor:
    def __init__(self, response=None):
        self.response = response or advice()
        self.requests = []

    def advise(self, request):
        self.requests.append(request)
        return self.response


ENV = {"HOME": "/home/test", "CLAUDE_CONFIG_DIR": "/profiles/current"}


def payload(tool="Agent", **fields):
    return {"cwd": "/repo", "tool_name": tool,
            "tool_input": {"prompt": "Build this", "subagent_type": "general-purpose", **fields}}


@pytest.mark.parametrize("tool", ["Agent", "Task"])
def test_agent_rewrites_model_preserving_all_other_input(tool):
    advisor = StubAdvisor()
    data = payload(tool, model="opus", run_in_background=True)
    result = hooks.evaluate("PreToolUse", data, advisor, ENV)["hookSpecificOutput"]
    assert result["updatedInput"] == {**data["tool_input"], "model": "sonnet"}
    assert "permissionDecision" not in result
    assert advisor.requests == [{"repo": "/repo", "role": "builder", "task_class": "integration", "agent": "claude"}]


@pytest.mark.parametrize("kind", ["deny", "wait"])
def test_agent_blocks_when_advisor_refuses(kind):
    response = advice()
    response.update(verdict=kind, reason="weekly ceiling", retry_at="2026-11-04T00:00:00Z")
    result = hooks.evaluate("PreToolUse", payload(), StubAdvisor(response), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "weekly ceiling" in result["hookSpecificOutput"]["permissionDecisionReason"]
    assert "2026-11-04" in result["hookSpecificOutput"]["permissionDecisionReason"]


@pytest.mark.parametrize("changes", [{"verdict": "switch", "config_dir": "/profiles/other"}, {"agent": "codex"}])
def test_agent_denies_routing_that_needs_another_process(changes):
    response = advice()
    response.update(changes)
    result = hooks.evaluate("PreToolUse", payload(), StubAdvisor(response), ENV)
    assert result["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert response["config_dir"] in result["hookSpecificOutput"]["permissionDecisionReason"]


@pytest.mark.parametrize("agent_type, role", [("Explore", "explorer"), ("Plan", "planner"), ("code-reviewer", "reviewer"), ("independent-evaluator", "evaluator")])
def test_agent_maps_specialized_roles(agent_type, role):
    advisor = StubAdvisor()
    hooks.evaluate("PreToolUse", payload(subagent_type=agent_type), advisor, ENV)
    assert advisor.requests[0]["role"] == role


def test_service_failure_allows_and_logs_warning(caplog):
    class Offline:
        def advise(self, request):
            raise OSError("connection refused")

    assert hooks.evaluate("PreToolUse", payload(), Offline(), ENV) is None
    assert "advisor unavailable" in caplog.text


def test_unrelated_tool_does_not_contact_advisor():
    assert hooks.evaluate("PreToolUse", payload("Read"), object(), ENV) is None


@pytest.mark.parametrize("verdict,blocked", [("deny", True), ("wait", False), ("switch", False), ("go", False)])
def test_prompt_blocks_only_deny(verdict, blocked):
    response = advice()
    response["verdict"] = verdict
    output = hooks.evaluate("UserPromptSubmit", {"cwd": "/repo", "prompt": "private text"}, StubAdvisor(response), ENV)
    assert (output.get("decision") == "block") is blocked
    if blocked:
        assert "within quota" in output["reason"]
    else:
        assert output["hookSpecificOutput"]["hookEventName"] == "UserPromptSubmit"


class SessionAdvisor(StubAdvisor):
    def status(self):
        return {"generated_at": "now", "accounts": [
            {"config_dir": "/profiles/current", "provider": "claude", "label": "current",
             "seven_day": {"pct": 94}, "forecast": {"next_round_pct": 96}, "stale": False},
            {"config_dir": "/profiles/other", "provider": "claude", "label": "other",
             "seven_day": {"pct": 99}, "forecast": {"next_round_pct": 100}, "stale": False}], "history": {}}


def test_session_injects_advice_and_warns_on_own_forecast():
    output = hooks.evaluate("SessionStart", {"cwd": "/repo"}, SessionAdvisor(), ENV)
    assert "model=sonnet" in output["hookSpecificOutput"]["additionalContext"]
    assert "96" in output["systemMessage"]
    assert "current" in output["systemMessage"]
    assert "other" not in output["systemMessage"]


def test_session_does_not_warn_for_another_accounts_quota():
    env = {**ENV, "CLAUDE_CONFIG_DIR": "/profiles/unlisted"}
    output = hooks.evaluate("SessionStart", {"cwd": "/repo"}, SessionAdvisor(), env)
    assert "systemMessage" not in output


def test_status_failure_keeps_available_session_advice(caplog):
    class BrokenStatus(SessionAdvisor):
        def status(self):
            raise OSError("offline")

    output = hooks.evaluate("SessionStart", {"cwd": "/repo"}, BrokenStatus(), ENV)
    assert "model=sonnet" in output["hookSpecificOutput"]["additionalContext"]
    assert "advisor unavailable" in caplog.text


@pytest.mark.parametrize("event", ["SessionStart", "UserPromptSubmit"])
def test_session_service_failure_allows_with_warning(event, caplog):
    class Offline:
        def advise(self, request):
            raise OSError("offline")

    assert hooks.evaluate(event, {"cwd": "/repo"}, Offline(), ENV) is None
    assert "advisor unavailable" in caplog.text


def test_explicit_role_and_task_class_reach_advisor_without_prompt_text():
    advisor = StubAdvisor()
    hooks.evaluate("PreToolUse", payload(), advisor, ENV, role="evaluator", task_class="design")
    assert advisor.requests == [{"repo": "/repo", "role": "evaluator", "task_class": "design", "agent": "claude"}]


def test_session_warns_on_deduplicated_profile_alias():
    class Aliased(SessionAdvisor):
        def status(self):
            document = super().status()
            document["accounts"][0]["aliases"] = ["/profiles/orca"]
            return document

    output = hooks.evaluate("SessionStart", {"cwd": "/repo"}, Aliased(), {**ENV, "CLAUDE_CONFIG_DIR": "/profiles/orca"})
    assert "96" in output["systemMessage"]


def test_explicit_deny_without_route_fields_still_blocks():
    response = {"verdict": "deny", "agent": "claude", "reason": "quota exhausted", "retry_at": None}
    output = hooks.evaluate("UserPromptSubmit", {"cwd": "/repo"}, StubAdvisor(response), ENV)
    assert output["decision"] == "block"
    assert "quota exhausted" in output["reason"]
