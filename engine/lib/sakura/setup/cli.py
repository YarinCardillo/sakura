"""The first-launch setup commands."""

import argparse
import json
import os
import sys
from pathlib import Path

from sakura import engine_home, programs
from sakura.accounts import goose
from sakura.accounts.providers import GOOSE
from sakura.cockpit import account_login
from sakura.flow import brakes, exits
from sakura.setup import apply, detect


def main(argv: list[str]) -> int:
    """Print the host's program and login inventory on request."""
    parser = argparse.ArgumentParser(prog="sakura setup")
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("detect", help="look for supported programs and their login folders")
    command.add_argument("--json", action="store_true", required=True)
    apply_command = commands.add_parser("apply", help="apply or resume confirmed first-launch choices")
    apply_command.add_argument("--op", required=True)
    apply_command.add_argument("--choices", required=True)
    configure = commands.add_parser("configure-goose", help="open the installed Goose configuration flow")
    configure.add_argument("--folder", required=True)
    args = parser.parse_args(argv)
    if args.command == "apply":
        return _apply(args)
    return _configure(args.folder) if args.command == "configure-goose" else _detect()


def _detect() -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print("sakura setup detect is only available outside engine roles and agent cages", file=sys.stderr)
        return exits.USAGE
    print(json.dumps(detect.detect(consented=True)))
    return exits.OK


def _configure(folder: str) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print("local_setup_required", file=sys.stderr)
        return exits.USAGE
    if problem := account_login.folder_problem(folder, GOOSE):
        print(problem, file=sys.stderr)
        return exits.USAGE
    try:
        goose.configure(Path(folder).resolve())
    except programs.ProgramError as error:
        print(error, file=sys.stderr)
        return exits.USAGE
    except OSError:
        print("adapter_could_not_start", file=sys.stderr)
        return exits.FAILED
    return exits.OK


def _apply(args: argparse.Namespace) -> int:
    try:
        reply = apply.run(args.op, apply.parse_choices(args.op, args.choices))
    except apply.SetupRefused as error:
        reply = exits.USAGE, str(error)
    code, output = reply
    print(output, file=sys.stdout if code == exits.OK else sys.stderr)
    return code
