"""In-process HTTP tests for the advisor wire contract."""

import io
import json
import sys

import pytest

from sakura.cli import main
from sakura.quota_enforcement import client


@pytest.fixture
def service(monkeypatch):
    calls = []

    class Connection:
        def __init__(self, host, port, *, timeout):
            assert (host, port, timeout) == ("127.0.0.1", 42002, client.TIMEOUT_SECONDS)

        def request(self, method, path, *, body, headers):
            assert headers == {"Content-Type": "application/json"}
            assert method == ("POST" if path == "/advise" else "GET")
            calls.append((path, json.loads(body) if body else None))

        def getresponse(self):
            path, _ = calls[-1]
            documents = {
                "/advise": {"verdict": "go", "agent": "claude", "model": "sonnet", "effort": "medium",
                            "config_dir": "/profile", "reason": "available", "retry_at": None},
                "/status": {"generated_at": "now", "accounts": [], "history": {}},
            }
            response = io.BytesIO(json.dumps(documents[path]).encode())
            response.status = 200
            return response

        def close(self):
            pass

    monkeypatch.setattr(client, "HTTPConnection", Connection)
    return "http://127.0.0.1:42002", calls


def test_client_posts_advise_and_reads_status(service):
    url, calls = service
    advisor = client.Advisor(url)
    data = {"repo": "/repo", "role": "builder", "task_class": "mechanical", "agent": "claude"}
    assert advisor.advise(data)["model"] == "sonnet"
    assert advisor.status()["accounts"] == []
    assert calls == [("/advise", data), ("/status", None)]


@pytest.mark.parametrize("url", ["https://127.0.0.1:8799", "http://example.com", "http://127.0.0.1:8799/path", "http://user:pass@127.0.0.1:8799"])
def test_client_rejects_non_loopback_or_ambiguous_urls(url):
    with pytest.raises(ValueError):
        client.Advisor(url)


def test_deny_response_needs_reason_but_not_a_usable_route():
    client.validate_advice({"verdict": "deny", "agent": "claude", "model": None,
                            "effort": None, "config_dir": None, "reason": "no quota", "retry_at": None})


@pytest.mark.parametrize("changes", [{"verdict": "unknown"}, {"agent": "other"}, {"model": ""}, {"retry_at": 42}])
def test_invalid_go_advice_is_rejected(changes):
    response = {"verdict": "go", "agent": "claude", "model": "sonnet", "effort": "medium",
                "config_dir": "/profile", "reason": "available", "retry_at": None}
    response.update(changes)
    with pytest.raises(ValueError):
        client.validate_advice(response)


def test_hook_cli_rewrites_agent_using_loopback_service(service, monkeypatch, capsys):
    url, calls = service
    payload = {"cwd": "/repo", "tool_name": "Agent", "tool_input": {
        "prompt": "private task text", "subagent_type": "Explore", "model": "opus"}}
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(payload)))
    assert main(["quota-hooks", "run", "PreToolUse", "--config-dir", "/profile",
                 "--advisor-url", url, "--task-class", "design"]) == 0
    output = json.loads(capsys.readouterr().out)["hookSpecificOutput"]
    assert output["updatedInput"] == {"prompt": "private task text", "subagent_type": "Explore", "model": "sonnet"}
    assert "permissionDecision" not in output
    assert calls == [("/advise", {"repo": "/repo", "role": "explorer", "task_class": "design", "agent": "claude"})]
