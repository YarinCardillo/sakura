"""`sakura cockpit`: serve the web cockpit, print it as text or JSON, act on it, or open it in its tmux session."""

from __future__ import annotations

import argparse
import os
import shutil
import sys
from collections.abc import Callable
from dataclasses import replace
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING

from sakura import actors
from sakura.cockpit import INPUT_PREFIX, background_actions
from sakura.cockpit.settings import CockpitRefused
from sakura.flow import config as flow_config
from sakura.flow import exits, registry
from sakura.flow.render import strip_controls

if TYPE_CHECKING:
    from sakura.cockpit import gate, snapshot
    from sakura.cockpit.settings import CockpitSettings
    from sakura.flow.tmux import TmuxClient

SNAPSHOT_CACHE_SECONDS = 5
DEFAULT_WIDTH = 100
DEFAULT_HEIGHT = 24
WINDOW_NAME = "cockpit"
WINDOW_MARKER_KEY = "@sakura-cockpit"
SESSION_MARKER = (WINDOW_MARKER_KEY, "1")

ACT_INPUTS = (("text", "text"), ("model", "model"), ("role", "role"), ("step", "step"), ("name", "name"),
              ("from", "from_ref"), ("label", "label"), ("percent", "percent"), ("plan", "plan"), ("since", "since"),
              ("verdict", "verdict"), ("note", "note"), ("remember", "remember"), ("choice", "choice"), ("folder", "folder"),
              ("level", "level"), ("preset", "preset"), ("rule", "rule"), ("value", "value"), ("accounts", "accounts"),
              ("device", "device"), ("port", "port"), ("provider", "provider"), ("state", "state"), ("op", "op"), ("choices", "choices"), ("until", "until"),
              ("project", "project"), ("title", "title"), ("definition", "definition"),
              ("track", "track"), ("goal", "goal"), ("summary", "summary"), ("status", "status"), ("clear", "clear"),
              ("decision", "decision"), ("answer", "answer"), ("task", "task"),
              ("conversation", "conversation"), ("startop", "startop"))
ALL_INPUTS = ACT_INPUTS + background_actions.INPUTS
LOGIN_SOURCE = "cli"
ACT_SOURCES = (LOGIN_SOURCE, actors.APP_SOURCE)
ADD_PROJECT, ADD_PROJECT_KEY = "add-project", "P"

Serve = Callable[..., None]
_REFUSALS = (CockpitRefused, flow_config.FlowConfigError, registry.RegistryError)


def main(argv: list[str], *, client: TmuxClient | None = None, serve: Serve | None = None,
         window_argv: list[str] | None = None, run: Callable | None = None) -> int:
    """Run `sakura cockpit <serve | --once | --open | snapshot | act>`.

    The tmux client, `serve`, the window command and the command runner of `act` are injectable.
    """
    if argv[:1] == ["snapshot"]:
        return _snapshot(argv[1:])
    if argv[:1] == ["act"]:
        return _act(argv[1:], run)
    if argv[:1] == ["phone"]:
        return _phone(argv[1:])
    args = serve_parser().parse_args(argv)
    _require_one_mode(args)
    from sakura.flow import brakes
    from sakura.flow.surface import SurfaceUnavailable
    from sakura.flow.tmux import TmuxError

    if not args.once and os.environ.get(brakes.ROLE_ENV):
        return _fail(brakes.ENGINE_ROLE_REFUSAL, exits.USAGE)
    try:
        if args.once:
            return _once()
        if args.open_window:
            return _open(client, window_argv)
        return _serve(serve)
    except _REFUSALS as error:
        return _fail(str(error), exits.USAGE)
    except (SurfaceUnavailable, TmuxError) as error:
        return _fail(str(error), exits.USAGE if isinstance(error, SurfaceUnavailable) else exits.FAILED)


def serve_parser() -> argparse.ArgumentParser:
    """The parser of `serve`, `--once` and `--open`."""
    parser = argparse.ArgumentParser(prog="sakura cockpit")
    parser.add_argument("action", nargs="?", choices=("serve",), help="serve the web cockpit in the foreground")
    parser.add_argument("--once", action="store_true", help="print the cockpit once as text")
    parser.add_argument("--open", dest="open_window", action="store_true",
                        help="run serve in window cockpit of the cockpit tmux session and print the URL")
    return parser


def snapshot_parser() -> argparse.ArgumentParser:
    """The parser of `cockpit snapshot`."""
    parser = argparse.ArgumentParser(prog="sakura cockpit snapshot")
    parser.add_argument("--json", action="store_true", required=True, help="print the snapshot as JSON (api 1)")
    return parser


def act_parser() -> argparse.ArgumentParser:
    """The parser of `cockpit act`; one option per input of the action table."""
    parser = argparse.ArgumentParser(prog="sakura cockpit act")
    parser.add_argument("key", help="action key from the snapshot's actions list, or add-project")
    parser.add_argument("target", help="the id of an item, group, account, wait or project of the snapshot; "
                                       "for add-project, the folder to add")
    for name, destination in ALL_INPUTS:
        parser.add_argument(f"--{name}", dest=destination, metavar=name.upper(), help=f"input {name}")
    parser.add_argument("--confirm", metavar="TOKEN", help="the token a confirm result returned")
    parser.add_argument("--actor", metavar="LOGIN", help="login written to the action log and the ledger")
    parser.add_argument("--source", choices=ACT_SOURCES, default=LOGIN_SOURCE,
                        help="where the person acts: a terminal (cli) or the Sakura window (app)")
    parser.add_argument("--json", action="store_true", required=True, help="print the result as JSON")
    return parser


def phone_parser() -> argparse.ArgumentParser:
    """The parser of `cockpit phone`: the phone page setup, and the changes the app can make to it."""
    parser = argparse.ArgumentParser(prog="sakura cockpit phone")
    parser.add_argument("--json", action="store_true", required=True, help="print the setup as JSON")
    parser.add_argument("--actor", help=argparse.SUPPRESS)
    parser.add_argument("change", nargs="?", choices=("allow", "remove", "port"),
                        help="allow a overlay device, remove one, or set the port")
    parser.add_argument("value", nargs="?", help="the device id, or the port number")
    return parser


def _require_one_mode(args: argparse.Namespace) -> None:
    if [bool(args.action), args.once, args.open_window].count(True) != 1:
        serve_parser().error("choose exactly one of: serve, --once, --open")


# Modes

def _once() -> int:
    from sakura.cockpit import text

    width = shutil.get_terminal_size((DEFAULT_WIDTH, DEFAULT_HEIGHT)).columns
    print("\n".join(text.render(_collect(), width, None)))
    return exits.OK


def _serve(serve: Serve | None) -> int:
    from sakura.cockpit import api, proofs, settings, snapshot
    from sakura.cockpit.controller import Controller

    if serve is None:
        from sakura.cockpit import server

        serve = server.serve
    trusted = settings.load(registered_repos=registry.repos())
    fresh = _collect
    try:
        controller = Controller(proofs.load_secret(), snapshot.CachedSnapshot(fresh, SNAPSHOT_CACHE_SECONDS), fresh)
        serve(trusted, controller.index, controller.phone_act, api=api.Api(controller),
              reload=lambda: settings.load(registered_repos=registry.repos()))
    except KeyboardInterrupt:
        return exits.OK
    except (CockpitRefused, OSError) as error:
        return _fail(str(error), exits.FAILED)
    return exits.OK


def _open(client: TmuxClient | None, argv: list[str] | None) -> int:
    from sakura.cockpit import gate, settings
    from sakura.flow import surface
    from sakura.flow.tmux import TmuxClient

    client = client or TmuxClient()
    argv = argv or [str(surface.KIT_BIN), "cockpit", "serve"]
    trusted = settings.load(registered_repos=registry.repos())
    personal = flow_config.load(Path.home())
    surface.require_tmux_surface(personal, client)  # raises SurfaceUnavailable: needs tmux, before any tailscale call
    url = _url(gate.read_self(trusted), trusted)
    session = personal.values["surface"]["cockpit_session"]
    if not _ensure_window(client, session, argv):
        return _fail(f"tmux session {session} exists but was not created by the cockpit; leaving it alone", exits.BLOCKED)
    print(url)
    return exits.OK


# JSON for the dashboard

def _snapshot(argv: list[str]) -> int:
    snapshot_parser().parse_args(argv)
    from sakura.cockpit import api

    try:
        print(api.dumps(api.snapshot_document(_collect())))
    except _REFUSALS as error:
        return _fail(str(error), exits.USAGE)
    return exits.OK


def _phone(argv: list[str]) -> int:
    """Print the phone page setup, after the change asked for. Reading is open; a change is refused inside an engine role."""
    args = phone_parser().parse_args(argv)
    from sakura.cockpit import api, phone
    from sakura.flow import brakes

    if args.change and os.environ.get(brakes.ROLE_ENV):
        return _refuse(brakes.ENGINE_ROLE_REFUSAL)
    try:
        change = {"allow": phone.allow_device, "remove": phone.remove_device, "port": phone.set_port}.get(args.change)
        if change and args.value is None:
            return _refuse(f"{args.change} needs a value")
        print(api.dumps(change(args.value) if change else phone.read()))
    except (phone.PhoneSetupRefused, CockpitRefused, OSError) as error:
        return _refuse(strip_controls(str(error)))
    return exits.OK


def _act(argv: list[str], run: Callable | None) -> int:
    args = act_parser().parse_args(argv)
    from sakura import operator_only
    from sakura.cockpit import actions, api, proofs, track_actions
    from sakura.cockpit.controller import (
        FIELD_CONFIRM,
        PROOF_SOURCE_CLI,
        Controller,
        Refused,
    )
    from sakura.connectors import contract as connector_contract
    from sakura.flow import brakes

    if run is None:
        run = actions.run_in_own_group
    if args.key in connector_contract.ACTIONS and operator_only.is_inside_a_role():
        print(api.dumps(api.action_document(Refused(brakes.ENGINE_ROLE_REFUSAL, reason_code="operator_only"))))
        return exits.USAGE
    is_track_action = args.key in track_actions.SPECS or (
        args.key == "L" and (args.track is not None or args.op is not None))
    if is_track_action and operator_only.is_inside_a_role():
        print(api.dumps(api.action_document(Refused(brakes.ENGINE_ROLE_REFUSAL, reason_code="forbidden"))))
        return exits.USAGE
    if os.environ.get(brakes.ROLE_ENV):
        return _refuse(brakes.ENGINE_ROLE_REFUSAL)
    try:
        actor = replace(actors.resolve(None if args.actor is None else f"{LOGIN_SOURCE}:{args.actor}",
                                       {**os.environ, actors.TRUSTED_ENV: "1"}),
                        source=args.source)  # SAKURA_ACTOR may name another source; this transport is the person's
        fresh = _collect
        outcome = Controller(proofs.load_secret(), fresh, fresh, run=run).perform(
            _act_fields(args), actor, proof_field=FIELD_CONFIRM, proof_source=PROOF_SOURCE_CLI)
    except actors.ActorError as error:
        return _refuse(str(error))
    except _REFUSALS as error:
        return _refuse(str(error))
    except OSError as error:
        print(api.dumps({"result": "refused", "reason": strip_controls(str(error))}))
        return exits.FAILED
    print(api.dumps(api.action_document(outcome)))
    return exits.USAGE if isinstance(outcome, Refused) else exits.OK


def _act_fields(args: argparse.Namespace) -> dict:
    from sakura.cockpit import api, host_actions
    from sakura.cockpit.controller import FIELD_CONFIRM

    if args.key == ADD_PROJECT:
        return {"key": ADD_PROJECT_KEY, "target": api.target_id(host_actions.PROJECTS),
                INPUT_PREFIX + "folder": args.target}
    fields = {"key": args.key, "target": args.target}
    fields.update({INPUT_PREFIX + name: getattr(args, destination) for name, destination in ALL_INPUTS
                   if getattr(args, destination) is not None})
    if args.confirm is not None:
        fields[FIELD_CONFIRM] = args.confirm
    return fields


def _refuse(reason: str) -> int:
    from sakura.cockpit import api
    from sakura.cockpit.controller import Refused

    print(api.dumps(api.action_document(Refused(reason))))
    return exits.USAGE


# Snapshot

def _collect() -> snapshot.Snapshot:
    from sakura.cockpit import snapshot

    return snapshot.collect(registry.repos(include_missing=True), datetime.now().astimezone())


# tmux window

def _url(own: gate.Self, trusted: CockpitSettings) -> str:
    return f"http://{own.dns_name or own.address}:{trusted.port}/"


def _ensure_window(client: TmuxClient, session: str, argv: list[str]) -> bool:
    """Make sure a live window `cockpit` runs `argv` in `session`; False when the session is not the cockpit's."""
    from sakura import interpreter
    from sakura.flow import surface

    env = {key: os.environ[key] for key in surface.WINDOW_ENV_KEYS if key in os.environ}
    env.update(interpreter.environment())
    if not client.has_session(session):
        client.ensure_session(session, Path.home(), WINDOW_NAME, argv, env, SESSION_MARKER)
        for window in client.windows(session):
            client.set_option(window.id, WINDOW_MARKER_KEY, SESSION_MARKER[1])
        return True
    if client.option(session, WINDOW_MARKER_KEY) != SESSION_MARKER[1]:
        return False
    ours = [window for window in client.windows(session) if window.options.get(WINDOW_MARKER_KEY)]
    if all(window.dead for window in ours):
        for window in ours:
            client.kill_window(window.id)
        client.set_option(client.new_window(session, WINDOW_NAME, argv, Path.home(), env), WINDOW_MARKER_KEY,
                          SESSION_MARKER[1])
    return True


def _fail(message: str, code: int) -> int:
    print(strip_controls(message), file=sys.stderr)
    return code
