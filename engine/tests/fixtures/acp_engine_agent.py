"""ACP executable fixture shared by catalog and Master tests."""

import json
import os
import sys


def send(**message):
    print(json.dumps({"jsonrpc": "2.0", **message}), flush=True)


def choices(models, selected):
    model = next(item for item in models if item["value"] == selected)
    options = [{"id": "model-id", "type": "select", "category": "model",
                "currentValue": selected, "options": models}]
    if model["efforts"]:
        options.append({"id": "reasoning", "type": "select", "category": "thought_level",
                        "currentValue": model.get("default_effort") or model["efforts"][0],
                        "options": [{"value": value, "name": value} for value in model["efforts"]]})
    return options


def update(session, **update):
    send(method="session/update", params={"sessionId": session, "update": update})


def main(config):
    models = config.get("models", [])
    if config.get("master") and config["harness"] == "codex":
        policy = json.loads(os.environ["CODEX_CONFIG"])
        assert policy["default_permissions"] == "sakura-work"
        assert policy["features"]["shell_tool"] is False and policy["mcp_servers"] == {}
        assert os.environ["INITIAL_AGENT_MODE"] == "permission-profile:sakura-work"
    selected = config.get("default") or (models[0]["value"] if models else "")
    session = config.get("session", "fixture-session")
    mode = {"availableModes": [{"id": "permission-profile:sakura-work", "name": "Master"}],
            "currentModeId": "permission-profile:sakura-work"}
    for line in sys.stdin:
        request = json.loads(line)
        method = request["method"]
        params = request.get("params", {})
        with open(config["log"], "a") as log:
            log.write(json.dumps([method, params, os.getenv(config.get("account_var", "GOOSE_PROVIDER"))]) + "\n")
        result = {}
        if method == "initialize":
            optional = {"systemPrompt": {}} if config.get("system_prompt") else {}
            result = {"protocolVersion": 1, "agentInfo": {"name": "fixture", "version": config["version"]},
                      "agentCapabilities": {"loadSession": config.get("load", True),
                                            "mcpCapabilities": {"http": True}, "sessionCapabilities": {"close": {}, **optional}}}
        elif method == "session/new":
            result = {"sessionId": session}
            if models:
                result["configOptions"] = choices(models, selected)
        elif method == "session/load":
            session = params["sessionId"]
            update(session, sessionUpdate="user_message_chunk", content={"type": "text", "text": "history"})
            if models:
                result["configOptions"] = choices(models, selected)
        elif method == "session/set_config_option":
            if params["configId"] == "model-id":
                selected = params["value"]
            result["configOptions"] = choices(models, selected)
            if params["configId"] == "reasoning":
                result["configOptions"][1]["currentValue"] = params["value"]
        elif method == "session/prompt":
            if change := config.get("switch_on_prompt"):
                update(session, sessionUpdate="config_option_update", configOptions=choices(models, change))
            update(session, sessionUpdate="agent_message_chunk", content={"type": "text", "text": "fixture reply"})
            result = {"stopReason": "end_turn"}
        if config["harness"] == "codex" and method in {"session/new", "session/load"}:
            result["modes"] = mode
        if "id" in request:
            send(id=request["id"], result=result)
