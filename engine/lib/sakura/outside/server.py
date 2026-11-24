"""`sakura outside serve --agent NAME`: the granted agent verbs as tools of a stdio MCP server.

An agent's client starts it outside the shell sandbox, so it can write the engine home where a sandboxed
`sakura outside` cannot. Each tool calls the same function as the command line and answers the same JSON document
as text. The server keeps, per lane, the cursor of its last `inbox` answer and sends it as `after` on the next call,
so a session never sees an entry twice even when its model forgets the cursor, and a session that ends before the
next call leaves its entries unacknowledged for the next session.
"""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, IO, Mapping

from sakura import mcp_stdio
from sakura.flow import brakes, exits
from sakura.outside import access, verbs
from sakura.outside.records import Agent, Refused

SERVER_NAME = "sakura-outside"
MAX_MCP_WAIT = 50
AGENT_ENV = "SAKURA_OUTSIDE_AGENT"
_LANE = {"type": "string", "description": "The lane id: lowercase letters, digits and dashes, at most 40."}
_TEXT, _TEXTS = {"type": "string"}, {"type": "array", "items": {"type": "string"}}
SPECS = {
    "open": ("Open a lane the person follows, or update your own. Repeating the same call changes nothing.",
             {"lane": _LANE, "title": _TEXT, "about": _TEXT, "phase": {"enum": ["draft", "ready"]},
              "links": {"type": "array", "maxItems": 4, "items": {"type": "object", "required": ["label", "url"],
                                                                  "properties": {"label": _TEXT, "url": _TEXT}}}},
             ["lane", "title"]),
    "row": ("Add a check only the person makes, or change one (a changed row waits for the person again).",
            {"lane": _LANE, "row": _TEXT, "title": _TEXT, "steps": _TEXTS, "expected": _TEXT, "note": _TEXT},
            ["lane", "row", "title", "steps"]),
    "ask": ("Ask the person a question, optionally with up to six choices. A new question pages the person.",
            {"lane": _LANE, "question": _TEXT, "text": _TEXT, "choices": _TEXTS}, ["lane", "question", "text"]),
    "close": ("Close your lane as done or dropped.", {"lane": _LANE, "outcome": {"enum": ["done", "dropped"]}},
              ["lane", "outcome"]),
    "inbox": ("Read the person's answers to this lane (marks, replies, messages). Waits up to `wait` seconds "
              "(0 to 50) for one. Entries you have read are not returned again.",
              {"lane": _LANE, "wait": {"type": "integer", "minimum": 0, "maximum": MAX_MCP_WAIT}}, ["lane"]),
    "list": ("List the lanes with what each waits on.", {"agent": _TEXT, "closed": {"type": "boolean"}}, []),
    "show": ("Show one lane's record and its log.", {"lane": _LANE}, ["lane"]),
}


def main(argv: list[str], *, env: Mapping[str, str], cwd: Path, now: Callable[[], datetime] | None = None,
         pusher: verbs.Pusher | None = None) -> int:
    """Start the server for `--agent NAME` (else `SAKURA_OUTSIDE_AGENT`) on stdin and stdout."""
    name = argv[1] if argv[:1] == ["--agent"] and len(argv) == 2 else env.get(AGENT_ENV, "")
    try:
        if env.get(brakes.ROLE_ENV):
            raise Refused(f"serve cannot start inside an engine role: {brakes.ENGINE_ROLE_REFUSAL}")
        if not name:
            raise Refused(f"name the agent with --agent NAME or {AGENT_ENV}")
        grant = access.resolve(name, env.get(access.GRANT_ENV, ""))
    except Refused as error:
        print(f"sakura outside serve: {error}", file=sys.stderr)
        return exits.USAGE
    serve(grant.agent, sys.stdin, sys.stdout, allowed=grant.verbs, now=now, pusher=pusher)
    return exits.OK


def serve(agent: Agent, stdin: IO[str], stdout: IO[str], *, now: Callable[[], datetime] | None = None,
          pusher: verbs.Pusher | None = None, allowed: frozenset[str] = frozenset(SPECS)) -> None:
    """Serve the tools granted to `agent` until `stdin` closes."""
    clock = now or (lambda: datetime.now(timezone.utc))
    cursors: dict[str, int] = {}
    tools = [mcp_stdio.Tool(_spec(name), _caller(name, agent, clock, pusher, cursors)) for name in SPECS if name in allowed]
    mcp_stdio.serve(SERVER_NAME, tools, stdin, stdout)


def _spec(name: str) -> dict:
    description, properties, required = SPECS[name]
    return {"name": name, "description": description,
            "inputSchema": {"type": "object", "properties": properties, "required": required}}


def _caller(name: str, agent: Agent, clock: Callable[[], datetime], pusher, cursors: dict[str, int]):
    def call(arguments: dict) -> tuple[str, bool]:
        if name == "inbox":
            document = _inbox(agent, arguments, clock, cursors)
        else:
            document = verbs.dispatch(name, agent, arguments, clock(), pusher, scoped=True)
        return json.dumps(document), document["result"] in ("refused", "error")

    return call


def _inbox(agent: Agent, arguments: dict, clock: Callable[[], datetime], cursors: dict[str, int]) -> dict:
    lane, wait = arguments.get("lane"), arguments.get("wait", 0)
    if isinstance(wait, int) and not isinstance(wait, bool) and wait > MAX_MCP_WAIT:
        return verbs.refused(f"wait is at most {MAX_MCP_WAIT} seconds over MCP")
    given = {"lane": lane, "wait": wait, "after": cursors.get(lane) if isinstance(lane, str) else None}
    document = verbs.dispatch("inbox", agent, given, clock())
    if document["result"] == "inbox":
        cursors[lane] = document["cursor"]
    return document
