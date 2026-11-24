"""`sakura keeper`: keep lanes running without the app and resume them after a restart.

Operator commands only: a person runs them (or the launchd agent runs `serve`), never an agent inside an engine role
or its cage. Exit codes: 0 done, 2 refused (usage, inside a role), 4 failed (the agent could not be installed).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Mapping
from datetime import datetime, timezone
from typing import Callable, TextIO

from sakura import engine_home, keeper, keeper_launchd, operator_only
from sakura.digest import launchd
from sakura.flow import brakes, exits

ROLE_REFUSAL = "sakura keeper cannot run inside an engine role: " + brakes.ENGINE_ROLE_REFUSAL


class _Parser(argparse.ArgumentParser):
    def error(self, message: str):
        raise ValueError(f"usage: {message}")


def main(argv: list[str], *, now: Callable[[], datetime] | None = None, env: Mapping[str, str] | None = None,
         stdout: TextIO | None = None, agent: launchd.Agent | None = None) -> int:
    """Run `sakura keeper <serve | install | uninstall | status | phone-page>`; the clock, environment, output and the
    launchd agent's `launchctl` and folder are injectable."""
    env = os.environ if env is None else env
    out, clock, agent = stdout or sys.stdout, now or (lambda: datetime.now(timezone.utc)), agent or launchd.Agent()
    try:
        if operator_only.is_inside_a_role(env):
            raise ValueError(ROLE_REFUSAL)
        args = _parse(argv)
        return _COMMANDS[args.command](args, clock, out, agent)
    except (ValueError, engine_home.EngineHomeError) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.USAGE
    except (OSError, launchd.LaunchdError) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.FAILED


def _parse(argv: list[str]) -> argparse.Namespace:
    parser = _Parser(prog="sakura keeper")
    sub = parser.add_subparsers(dest="command", required=True, parser_class=_Parser)
    sub.add_parser("serve", help="look every minute and resume lanes that died with a restart (the agent runs this)")
    for name, help_text in (("install", "install the launchd agent that keeps the keeper running"),
                            ("uninstall", "remove that agent; lanes are left alone")):
        _with_cockpit_options(sub.add_parser(name, help=help_text)).add_argument("none", nargs="*", help=argparse.SUPPRESS)
    sub.add_parser("status").add_argument("--json", action="store_true", dest="as_json")
    phone = _with_cockpit_options(sub.add_parser("phone-page", help="run the phone page under the keeper, or not"))
    phone.add_argument("setting", choices=("on", "off"))
    return parser.parse_args(argv)


def _with_cockpit_options(parser: argparse.ArgumentParser) -> argparse.ArgumentParser:
    """Accept the `--actor` the cockpit says who asked with (its action log records it)."""
    parser.add_argument("--actor", help=argparse.SUPPRESS)
    return parser


def _serve(args: argparse.Namespace, clock: Callable[[], datetime], out: TextIO, agent: launchd.Agent) -> int:
    from sakura.cockpit import cli as cockpit_cli
    from sakura.cockpit import server

    try:
        keeper.serve(now=clock, phone_page=lambda stop: cockpit_cli.main(
            ["serve"], serve=lambda *given, **options: server.serve(*given, stop=stop, **options)))
    except KeyboardInterrupt:
        return exits.OK
    return exits.OK


def _install(args: argparse.Namespace, clock: Callable[[], datetime], out: TextIO, agent: launchd.Agent) -> int:
    path = keeper_launchd.install(launchctl=agent.launchctl, agents_dir=agent.folder, home=agent.home)
    print(f"installed {path}: the keeper starts at login and resumes lanes that a restart stopped", file=out)
    return exits.OK


def _uninstall(args: argparse.Namespace, clock: Callable[[], datetime], out: TextIO, agent: launchd.Agent) -> int:
    removed = keeper_launchd.uninstall(launchctl=agent.launchctl, agents_dir=agent.folder, home=agent.home)
    print("agent removed; lanes were left as they are" if removed else "no agent was installed", file=out)
    return exits.OK


def _status(args: argparse.Namespace, clock: Callable[[], datetime], out: TextIO, agent: launchd.Agent) -> int:
    found = keeper.status(clock())
    if args.as_json:
        print(json.dumps(found), file=out)
        return exits.OK
    last = found["last_restart"]
    print(f"keeper {'on' if found['on'] else 'off'}; phone page {'on' if found['phone_page'] else 'off'}; "
          + (f"last restart {last['at']}: resumed {len(last['resumed'])}, waiting {len(last['waiting'])}" if last
             else "no restart seen"), file=out)
    for lane in found["needs_person"]:
        print(f"  needs you: {lane}", file=out)
    return exits.OK


def _phone_page(args: argparse.Namespace, clock: Callable[[], datetime], out: TextIO, agent: launchd.Agent) -> int:
    keeper.set_phone_page(args.setting == "on")
    print(f"phone page {args.setting}: the keeper picks it up on its next look", file=out)
    return exits.OK


_COMMANDS = {"serve": _serve, "install": _install, "uninstall": _uninstall, "status": _status,
             "phone-page": _phone_page}
