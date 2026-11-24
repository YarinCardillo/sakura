"""`sakura automations`: see the automations, accept or reject the master's proposals, switch them on and off.

Operator commands only: the person runs them (the cockpit runs them for the person, and a LaunchAgent runs `run`), never
an agent inside an engine role or its cage. Exit codes: 0 done, 2 refused.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Callable, Mapping
from datetime import datetime, timezone
from typing import TextIO

from pathlib import Path

from sakura import actors, engine_home, operator_only
from sakura.cockpit import decided
from sakura.automations import registry
from sakura.digest import launchd
from sakura.flow import brakes, exits, waits
from sakura.scheduled import migrate

ROLE_REFUSAL = "sakura automations cannot run inside an engine role: " + brakes.ENGINE_ROLE_REFUSAL


def main(argv: list[str], *, now: Callable[[], datetime] | None = None, env: Mapping[str, str] | None = None,
         stdout: TextIO | None = None, agent: launchd.Agent | None = None,
         run_verb: registry.RunVerb | None = None) -> int:
    """Run `sakura automations <list | enable | disable | remove | accept | reject | run>`; the clock, environment,
    output, the LaunchAgent's `launchctl` and folder, and the start of a verb are injectable."""
    out, clock, agent = stdout or sys.stdout, now or (lambda: datetime.now(timezone.utc)), agent or launchd.Agent()
    if operator_only.is_inside_a_role(os.environ if env is None else env):
        print(f"sakura: error: {ROLE_REFUSAL}", file=sys.stderr)
        return exits.USAGE
    args = _parser().parse_args(argv)
    try:
        if args.command in ("list", "run"):
            return _COMMANDS[args.command](args, clock(), out, agent, run_verb)
        with migrate.lock():
            return _COMMANDS[args.command](args, clock(), out, agent, run_verb)
    except (registry.AutomationRefused, launchd.LaunchdError, engine_home.EngineHomeError, actors.ActorError) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.USAGE


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura automations")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list").add_argument("--json", action="store_true", dest="as_json")
    for name in ("enable", "disable", "remove", "run"):
        sub.add_parser(name).add_argument("name")
    for name in ("accept", "reject"):
        each = sub.add_parser(name)
        each.add_argument("name")
        each.add_argument("digest")
    for name in ("enable", "disable", "remove", "accept", "reject"):
        sub.choices[name].add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its log records it
    return parser


def _list(args: argparse.Namespace, now: datetime, out: TextIO, agent: launchd.Agent, run_verb) -> int:
    found = registry.rows()
    if args.as_json:
        print(json.dumps(found), file=out)
    else:
        for row in found:
            print(f"{row['name']}: {row['schedule_text']}, {'on' if row['enabled'] else 'off'}", file=out)
    return exits.OK


def _switch(change: Callable[[str, launchd.Agent], None], word: str):
    def command(args: argparse.Namespace, now: datetime, out: TextIO, agent: launchd.Agent, run_verb) -> int:
        change(args.name, agent)
        print(f"{args.name} {word}", file=out)
        return exits.OK
    return command


def _accept(args: argparse.Namespace, now: datetime, out: TextIO, agent: launchd.Agent, run_verb) -> int:
    asked = _ask_of(args.name)
    found = registry.accept(args.name, args.digest, agent, now)
    decided.record_now(asked, "accept", _actor(args))
    print(f"accepted {found.name}: {found.schedule}", file=out)
    return exits.OK


def _reject(args: argparse.Namespace, now: datetime, out: TextIO, agent: launchd.Agent, run_verb) -> int:
    asked = _ask_of(args.name)
    registry.reject(args.name, args.digest)
    decided.record_now(asked, "reject", _actor(args))
    print(f"rejected {args.name}", file=out)
    return exits.OK


def _ask_of(name: str) -> dict:
    """The ask the person is deciding, as the decided-asks log keeps it: the engine's own record while it is readable."""
    return (decided.find_wait(Path(waits.AUTOMATIONS_ROOT), [], waits.AUTOMATION, name)
            or decided.fallback_wait(Path(waits.AUTOMATIONS_ROOT), name, waits.AUTOMATION, name))


def _actor(args: argparse.Namespace) -> str:
    return actors.format(actors.resolve(args.actor))


def _run(args: argparse.Namespace, now: datetime, out: TextIO, agent: launchd.Agent, run_verb) -> int:
    return registry.run(args.name, now, run_verb=run_verb)


_COMMANDS = {"list": _list, "enable": _switch(registry.enable, "enabled"), "disable": _switch(registry.disable, "disabled"),
             "remove": _switch(registry.remove, "removed"), "accept": _accept, "reject": _reject, "run": _run}
