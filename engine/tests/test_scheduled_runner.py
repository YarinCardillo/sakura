import sys
from datetime import timedelta
from pathlib import Path

from sakura.master.spec import MasterSpec
from sakura.scheduled import dispatch, runner, store
from test_scheduled_store import DEFINITION, NOW

ADAPTER = """
import json
import sys
import time

def send(message):
    print(json.dumps({"jsonrpc": "2.0", **message}), flush=True)

for raw in sys.stdin:
    request = json.loads(raw)
    method = request["method"]
    params = request["params"]
    result = {}
    if method == "initialize":
        result = {"protocolVersion": 1, "agentInfo": {"name": "claude-agent-acp"}}
    if method == "session/new":
        assert params["_meta"]["claudeCode"]["options"]["settingSources"] == []
        result = {"sessionId": "fresh-run"}
    if method == "session/prompt":
        send({"method": "session/update", "params": {"sessionId": "fresh-run", "update": {
            "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Completed."}
        }}})
        send({"id": 120, "method": "session/request_permission", "params": {
            "sessionId": "fresh-run", "toolCall": {"name": "Bash", "rawInput": {"command": "false"}},
            "options": [{"optionId": "allow", "kind": "allow_once"}]
        }})
        assert json.loads(sys.stdin.readline())["result"]["outcome"]["outcome"] == "cancelled"
        if len(sys.argv) > 1:
            time.sleep(30)
        result = {"stopReason": "end_turn"}
    send({"id": request["id"], "result": result})
"""


def _spec(tmp: Path, *options: str) -> MasterSpec:
    script = tmp / "adapter.py"
    script.write_text(ADAPTER)
    return MasterSpec([sys.executable, str(script), *options], {}, tmp, {"settingSources": []}, [], script, script, script)


def test_headless_turn_persists_prompt_and_reply_and_declines_permission(tmp_path):
    tasks = store.Store()
    task = tasks.create(DEFINITION, "runner", NOW)
    run_id = dispatch.run_now(tasks, task.id, task.revision, "run-1", NOW)
    assert dispatch.run_now(tasks, task.id, task.revision, "run-1", NOW) == run_id
    runner.drain(tasks, lambda: NOW, prepare=lambda _run: _spec(tmp_path))
    (run,) = dispatch.runs(store.Store(), task.id)
    assert run["state"] == "finished"
    assert [(entry["kind"], entry["text"]) for entry in runner.conversation(store.Store(), run_id)] == [
        ("prompt", DEFINITION["prompt"]), ("response", "Completed."),
    ]


def test_timeout_keeps_output_and_prunes_transcript(tmp_path):
    tasks = store.Store()
    task = tasks.create(DEFINITION, "runner", NOW)
    run_id = dispatch.run_now(tasks, task.id, task.revision, "run-1", NOW)
    runner.drain(tasks, lambda: NOW, prepare=lambda _run: _spec(tmp_path, "stall"), timeout=1)
    (run,) = dispatch.runs(tasks, task.id)
    assert (run["state"], run["reason"]) == ("failed", "adapter_timeout")
    assert runner.conversation(tasks, run_id)[-1]["text"] == "Completed."
    runner.prune(tasks, NOW + timedelta(days=31))
    assert dispatch.runs(tasks, task.id) == []
    assert runner.conversation(tasks, run_id) is None


def test_run_grant_rejects_forgery_and_revocation():
    import pytest
    from sakura.connectors import registry
    from sakura.scheduled import run_tools

    registry._save([registry.Connector("feed-news", "feed", "News", "https://news.example/feed")], {})
    tasks = store.Store()
    task = tasks.create({**DEFINITION, "connector_ids": ["feed-news"]}, "grant", NOW)
    dispatch.run_now(tasks, task.id, task.revision, "run-1", NOW)
    claim = runner._begin(tasks, NOW)
    assert run_tools.LANE_OPEN not in run_tools.names(claim.definition)
    for token, name in (("forged", run_tools.READ), (claim.token, run_tools.LANE_OPEN)):
        with pytest.raises(store.Refused), run_tools.authorized(tasks, claim.id, token, name):
            pytest.fail("the tool acquired authority")
    tasks.change(task.id, task.revision, "scheduledpause", {}, NOW)
    with pytest.raises(store.Refused), run_tools.authorized(tasks, claim.id, claim.token, run_tools.READ):
        pytest.fail("the revoked tool acquired authority")


def test_restart_fails_interrupted_run_once():
    tasks = store.Store()
    task = tasks.create(DEFINITION, "restart", NOW)
    dispatch.run_now(tasks, task.id, task.revision, "run-1", NOW)
    claim = runner._begin(tasks, NOW)
    for _ in range(2):
        runner.drain(tasks, lambda: NOW, prepare=lambda _claim: None)
    (run,) = dispatch.runs(tasks, task.id)
    assert (run["id"], run["state"], run["reason"]) == (claim.id, "failed", "interrupted")
    assert runner.conversation(tasks, claim.id)[0]["text"] == DEFINITION["prompt"]


def test_scoped_read_requires_prepared_and_current_connector_grants(monkeypatch):
    import pytest
    from sakura import grants
    from sakura.connectors import kinds, registry
    from sakura.scheduled import run_tools

    permissions = {}
    monkeypatch.setattr(grants, "current_connectors", lambda *_: permissions.copy())
    fields = {"kind": "feed", "label": "News", "address": "https://example.test/news"}
    connector_id = registry.mutate("add", "add-scope", kinds.Ports(), NOW, fields=fields)["id"]
    tasks = store.Store()
    task = tasks.create({**DEFINITION, "connector_ids": [connector_id]}, "scoped", NOW)
    dispatch.run_now(tasks, task.id, task.revision, "without-grant", NOW)
    first = runner._begin(tasks, NOW)
    permissions[connector_id] = ("read",)
    with pytest.raises(store.Refused, match="connector_scope_denied"):
        with run_tools.authorized(tasks, first.id, first.token, run_tools.READ, connector_id):
            pytest.fail("adding a grant widened a running tool")

    dispatch.run_now(tasks, task.id, task.revision, "with-grant", NOW)
    second = runner._begin(tasks, NOW)
    with run_tools.authorized(tasks, second.id, second.token, run_tools.READ, connector_id):
        pass
    permissions.clear()
    with pytest.raises(store.Refused, match="connector_scope_denied"):
        with run_tools.authorized(tasks, second.id, second.token, run_tools.READ, connector_id):
            pytest.fail("a removed grant remained usable")
    permissions[connector_id] = ("read",)
    registry.mutate("edit", "new-scope", kinds.Ports(), NOW, connector_id=connector_id, revision=1,
                    fields={**fields, "address": "https://example.test/other"})
    with pytest.raises(store.Refused, match="connector_scope_denied"):
        with run_tools.authorized(tasks, second.id, second.token, run_tools.READ, connector_id):
            pytest.fail("a newer connector revision widened a running tool")
