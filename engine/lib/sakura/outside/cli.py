"""`sakura outside`: the verbs an outside agent calls.

Verbs print their answer document as JSON on stdout and exit 0 (done), 2 (refused) or 4 (a store file could not be
read or written). Select the issued grant with `--agent NAME` (else `SAKURA_OUTSIDE_AGENT`) and present its token
in `SAKURA_OUTSIDE_GRANT`. This command carries no verb
of the person: their marks, replies, messages and close come only through the cockpit's action table.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Mapping, TextIO

from sakura import engine_home
from sakura.flow import brakes, exits
from sakura.outside import access, records, server, verbs
from sakura.outside.records import Refused, StoreError

AGENT_ENV = "SAKURA_OUTSIDE_AGENT"
ROLE_REFUSAL = "sakura outside cannot write inside an engine role: " + brakes.ENGINE_ROLE_REFUSAL
EXIT_OF = {"refused": exits.USAGE, "error": exits.FAILED}


class _Parser(argparse.ArgumentParser):
    def error(self, message: str):
        raise Refused(f"usage: {message}")


def main(argv: list[str], *, now: Callable[[], datetime] | None = None, pusher: verbs.Pusher | None = None,
         env: Mapping[str, str] | None = None, cwd: Path | None = None, stdout: TextIO | None = None) -> int:
    """Run `sakura outside <verb>`; the clock, pusher, environment, folder and output are injectable."""
    env = os.environ if env is None else env
    out = stdout or sys.stdout
    if argv[:1] == ["serve"]:
        return server.main(argv[1:], env=env, cwd=cwd or Path.cwd(), now=now, pusher=pusher)
    if argv[:1] == ["grant"]:
        return _grant(argv[1:], env, cwd or Path.cwd(), out)
    try:
        verb, args = _parse(argv)
        answer = _run(verb, args, now or (lambda: datetime.now(timezone.utc)), pusher, env, cwd or Path.cwd())
    except Refused as error:
        answer = verbs.refused(str(error))
    except (StoreError, engine_home.EngineHomeError) as error:
        answer = {"result": "error", "reason": str(error)}
    print(json.dumps(answer), file=out)
    return exits.OK if answer["result"] not in EXIT_OF else EXIT_OF[answer["result"]]


def _run(verb: str, args: argparse.Namespace, now: Callable[[], datetime], pusher, env: Mapping[str, str],
         cwd: Path) -> dict:
    name = getattr(args, "agent", None) or env.get(AGENT_ENV, "")
    if env.get(brakes.ROLE_ENV):
        raise Refused(ROLE_REFUSAL)
    given = _arguments(verb, args)
    if verb in verbs.READ_VERBS and not name and not env.get(access.GRANT_ENV):
        return verbs.dispatch(verb, records.Agent("reader", cwd), given, now(), pusher)
    if not name:
        raise Refused(f"name the agent with --agent NAME or {AGENT_ENV}")
    grant = access.resolve(name, env.get(access.GRANT_ENV, ""))
    if verb not in grant.verbs:
        raise Refused("this outside grant does not permit that verb")
    return verbs.dispatch(verb, grant.agent, given, now(), pusher, scoped=True)


def _grant(argv: list[str], env: Mapping[str, str], cwd: Path, out: TextIO) -> int:
    try:
        if env.get(brakes.ROLE_ENV) or env.get(access.GRANT_ENV):
            raise Refused("only the host operator can issue an outside grant")
        if len(argv) < 2:
            raise Refused("usage: sakura outside grant NAME VERB [VERB ...]")
        print(access.issue(argv[0], argv[1:], cwd), file=out)
    except (Refused, engine_home.EngineHomeError) as error:
        print(json.dumps(verbs.refused(str(error))), file=out)
        return exits.USAGE
    return exits.OK


def _arguments(verb: str, args: argparse.Namespace) -> dict:
    """The verb's arguments as the answer-document functions take them (the MCP tools take the same names)."""
    if verb == "open":
        return {"lane": args.lane, "title": args.title, "about": args.about, "phase": args.phase,
                "links": [_link(text) for text in args.link]}
    if verb == "row":
        return {"lane": args.lane, "row": args.row, "title": args.title, "steps": args.step,
                "expected": args.expected, "note": args.note}
    if verb == "ask":
        return {"lane": args.lane, "question": args.question, "text": args.text, "choices": args.choice}
    if verb == "close":
        return {"lane": args.lane, "outcome": args.outcome}
    if verb == "inbox":
        return {"lane": args.lane, "after": args.after, "wait": args.wait}
    if verb == "list":
        return {"agent": args.list_agent, "closed": args.closed}
    return {"lane": args.lane}


def _link(text: str) -> dict:
    label, separator, url = text.partition("=")
    if not separator:
        raise Refused(f"link {text[:40]!r} must be LABEL=URL")
    return {"label": label, "url": url}


def _parse(argv: list[str]) -> tuple[str, argparse.Namespace]:
    parser = _Parser(prog="sakura outside")
    sub = parser.add_subparsers(dest="verb", required=True, parser_class=_Parser)
    _agent_verbs(sub)
    args = parser.parse_args(argv)
    return args.verb, args


def _agent_verbs(sub) -> None:
    for name in ("open", "row", "ask", "close", "inbox", "list", "show"):
        verb = sub.add_parser(name)
        if name != "list":
            verb.add_argument("--agent")
            verb.add_argument("lane")
    sub.choices["open"].add_argument("--title", required=True)
    sub.choices["open"].add_argument("--about", default="")
    sub.choices["open"].add_argument("--phase", default="draft")
    sub.choices["open"].add_argument("--link", action="append", default=[], metavar="LABEL=URL")
    sub.choices["row"].add_argument("row")
    sub.choices["row"].add_argument("--title", required=True)
    sub.choices["row"].add_argument("--step", action="append", default=[])
    sub.choices["row"].add_argument("--expected", default="")
    sub.choices["row"].add_argument("--note", default="")
    sub.choices["ask"].add_argument("question")
    sub.choices["ask"].add_argument("--text", required=True)
    sub.choices["ask"].add_argument("--choice", action="append", default=[])
    sub.choices["close"].add_argument("--outcome", default="done")
    sub.choices["inbox"].add_argument("--after", type=int)
    sub.choices["inbox"].add_argument("--wait", type=int, default=0)
    sub.choices["list"].add_argument("--agent", dest="list_agent")
    sub.choices["list"].add_argument("--closed", action="store_true")
