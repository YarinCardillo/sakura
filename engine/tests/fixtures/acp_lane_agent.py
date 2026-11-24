"""A deterministic ACP subprocess for lane tests."""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

name, scenario = sys.argv[1:3]
SESSION = "session:fixture/one"


def send(**message):
    line = (json.dumps({"jsonrpc": "2.0", **message}) + "\n").encode()
    os.write(sys.stdout.fileno(), line[:17])
    os.write(sys.stdout.fileno(), line[17:])


def update(kind, **fields):
    send(method="session/update", params={"sessionId": SESSION, "update": {"sessionUpdate": kind, **fields}})


def message(text):
    update("agent_message_chunk", content={"type": "text", "text": text})


for line in sys.stdin:
    request = json.loads(line)
    method, identity = request.get("method"), request.get("id")
    if not method:
        Path("permission.json").write_text(json.dumps(request))
        continue
    if method == "initialize":
        result = {"protocolVersion": 1, "agentInfo": {"name": name, "version": "fixture-v1"},
                  "agentCapabilities": {"loadSession": scenario != "no-load",
                                        "sessionCapabilities": {"close": {}}, "mcpCapabilities": {"http": True}}}
    elif method == "session/new":
        if scenario == "auth":
            send(id=identity, error={"code": -32000, "message": "private diagnostic"})
            continue
        result = {"sessionId": SESSION}
    elif method == "session/load":
        assert request["params"]["sessionId"] == SESSION
        message('SAKURA_RESULT {"verdict":"stale replay"}')
        result = {}
    elif method == "session/prompt":
        assert request["params"]["sessionId"] == SESSION
        Path("prompt.json").write_text(json.dumps(request["params"]["prompt"]))
        print('SAKURA_RESULT {"verdict":"stderr"}', file=sys.stderr, flush=True)
        if scenario == "earlier-result":
            message('SAKURA_RESULT {"verdict":"earlier"}\n')
        update("tool_call", toolCallId="tool-1", title="workspace_command", status="in_progress")
        if scenario != "unclosed-tool":
            update("tool_call_update", toolCallId="tool-1", status="completed",
                   content=[{"type": "text", "text": 'SAKURA_RESULT {"verdict":"tool"}'}])
        if scenario in {"permission", "stop"}:
            send(id="allowed", method="session/request_permission",
                 params={"sessionId": SESSION, "toolCall": {"title": "workspace_command"},
                         "options": [{"kind": "allow_once", "optionId": "yes"},
                                     {"kind": "reject_once", "optionId": "no"}]})
        if scenario == "stop":
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            message("partial before stop")
            for pending in sys.stdin:
                if json.loads(pending).get("method") == "session/cancel":
                    Path("cancelled").touch()
            time.sleep(30)
        if scenario == "detached":
            leaf = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
            Path("descendant").write_text(str(leaf.pid))
        if scenario != "tool-only":
            message("First line.\n")
            if scenario != "earlier-result":
                message('SAKURA_RESULT {"verdict":"APPROVE"}')
        if scenario == "lost":
            sys.exit(0)
        result = {"stopReason": "refusal" if scenario == "refused" else "end_turn"}
    elif method == "session/close":
        Path("closed").touch()
        result = {}
    else:
        raise AssertionError(method)
    send(id=identity, result=result)
    if method == "session/close" and scenario == "bad-exit":
        sys.exit(7)
