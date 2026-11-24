"""A minimal stdio MCP server: newline-delimited JSON-RPC with `initialize`, `tools/list`, `tools/call` and `ping`.

Notifications get no answer. A line over 64 KiB is answered with the JSON-RPC error -32600 and skipped, never
parsed. A tool reports a refusal as a result with `isError: true`; a tool that raises is reported the same way and
the server keeps serving.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Callable, IO, Sequence

PROTOCOL_VERSION = "2025-06-18"
MAX_LINE = 64 * 1024
INVALID_REQUEST, PARSE_ERROR, UNKNOWN_METHOD, INVALID_PARAMS = -32600, -32700, -32601, -32602
OVERSIZE = object()


@dataclass(frozen=True)
class Tool:
    """A tool: its MCP spec (name, description, inputSchema) and `call(arguments) -> (text, is_error)`."""

    spec: dict
    call: Callable[[dict], tuple[str, bool]]


def serve(name: str, tools: Sequence[Tool], stdin: IO[str], stdout: IO[str]) -> None:
    """Answer requests from `stdin` until it closes; every answer is one line on `stdout`."""
    by_name = {tool.spec["name"]: tool for tool in tools}
    while True:
        line = _read_line(stdin)
        if line is None:
            return
        answer = _error(None, INVALID_REQUEST, f"line over {MAX_LINE} characters") if line is OVERSIZE \
            else _answer(name, by_name, line)
        if answer is not None:
            stdout.write(json.dumps(answer) + "\n")
            stdout.flush()



def _read_line(stdin: IO[str]):
    """The next line, OVERSIZE (its rest skipped) when it is over the limit, or None at the end of input."""
    line = stdin.readline(MAX_LINE + 1)
    if not line:
        return None
    if len(line) > MAX_LINE and not line.endswith("\n"):
        while line and not line.endswith("\n"):
            line = stdin.readline(MAX_LINE)
        return OVERSIZE
    return line


def _answer(name: str, tools: dict[str, Tool], line: str) -> dict | None:
    if not line.strip():
        return None
    try:
        message = json.loads(line)
    except (ValueError, RecursionError):
        return _error(None, PARSE_ERROR, "invalid JSON")
    if not isinstance(message, dict):
        return _error(None, INVALID_REQUEST, "a request is a JSON object")
    if "id" not in message:
        return None
    return _respond(name, tools, message)


def _respond(name: str, tools: dict[str, Tool], message: dict) -> dict:
    identifier, method = message["id"], message.get("method")
    params = message.get("params", {})
    if not isinstance(params, dict):
        return _error(identifier, INVALID_PARAMS, "params must be an object")
    if method == "initialize":
        result = {"protocolVersion": params.get("protocolVersion", PROTOCOL_VERSION), "capabilities": {"tools": {}},
                  "serverInfo": {"name": name, "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [tool.spec for tool in tools.values()]}
    elif method == "tools/call":
        return _call(tools, identifier, params)
    elif method == "ping":
        result = {}
    else:
        return _error(identifier, UNKNOWN_METHOD, "unknown method")
    return {"jsonrpc": "2.0", "id": identifier, "result": result}


def _call(tools: dict[str, Tool], identifier, params: dict) -> dict:
    tool = tools.get(params.get("name")) if isinstance(params.get("name"), str) else None
    arguments = params.get("arguments", {})
    if tool is None:
        return _error(identifier, INVALID_PARAMS, "unknown tool")
    if not isinstance(arguments, dict):
        return _error(identifier, INVALID_PARAMS, "arguments must be an object")
    try:
        text, is_error = tool.call(arguments)
    except Exception as error:  # noqa: BLE001 (a tool that raises must not stop the server; the client is told)
        text, is_error = f"the tool failed: {type(error).__name__}", True
    return {"jsonrpc": "2.0", "id": identifier,
            "result": {"content": [{"type": "text", "text": text}], "isError": is_error}}


def _error(identifier, code: int, message: str) -> dict:
    return {"jsonrpc": "2.0", "id": identifier, "error": {"code": code, "message": message}}
