"""`sakura connectors`: see what the digest can read through, connect an account, or disconnect it.

Operator commands only. A secret never travels on a command line: a `notion` or `linear` token is stored by the person in
the Keychain under `sakura-connector-notion` or `sakura-connector-linear`, and `connect` only checks that it is there.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Callable, Mapping
from datetime import datetime, timezone
from typing import TextIO

from sakura import engine_home, operator_only
from sakura.connectors import contract, kinds, registry
from sakura.flow import brakes, exits

ROLE_REFUSAL = "sakura connectors cannot run inside an engine role: " + brakes.ENGINE_ROLE_REFUSAL


def main(argv: list[str], *, now: Callable[[], datetime] | None = None, env: Mapping[str, str] | None = None,
         stdout: TextIO | None = None, ports: kinds.Ports | None = None) -> int:
    """Run `sakura connectors <list | connect | disconnect>`; the clock, environment, output and the ports (programs,
    Keychain, network) are injectable."""
    out, clock, ports = stdout or sys.stdout, now or (lambda: datetime.now(timezone.utc)), ports or kinds.Ports()
    if operator_only.is_inside_a_role(os.environ if env is None else env):
        print(f"sakura: error: {ROLE_REFUSAL}", file=sys.stderr)
        return exits.USAGE
    versioned = argv[:1] and argv[0] in _MUTATIONS
    if versioned and argv[-1:] == ["--"]:
        argv = argv[:-1]
    args = _parser().parse_args(argv)
    if versioned:
        return _mutate(args, clock(), out, ports)
    try:
        return _COMMANDS[args.command](args, clock(), out, ports)
    except (registry.ConnectorError, engine_home.EngineHomeError) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.USAGE


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura connectors")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list", help="check every connector again and print them").add_argument("--json", action="store_true",
                                                                                         dest="as_json")
    connect = sub.add_parser("connect", help="connect an account or source; secrets are never given here")
    connect.add_argument("kind", choices=kinds.KINDS)
    connect.add_argument("--label", default="", help="a name for it; a feed and a program need one")
    connect.add_argument("--target", default="", help="the address of a feed, or the absolute path of a program")
    disconnect = sub.add_parser("disconnect")
    disconnect.add_argument("connector")
    for each in (connect, disconnect):
        each.add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its action log records it
    for verb in _MUTATIONS:
        _mutation_parser(sub.add_parser(verb), verb)
    return parser


def _mutation_parser(parser: argparse.ArgumentParser, verb: str) -> None:
    parser.add_argument("--op", required=True)
    parser.add_argument("--actor", help=argparse.SUPPRESS)
    if verb != "add":
        parser.add_argument("--id", required=True)
        parser.add_argument("--revision", required=True)
    if verb == "replace":
        parser.add_argument("--candidate", required=True)
        parser.add_argument("--candidate-revision", required=True)
    if verb in ("add", "edit"):
        for field in sorted(set().union(*contract.FIELDS.values()) | {"label", "kind"}):
            action = {"args": "append", "whole_repo": "store_true"}.get(field, "store")
            parser.add_argument("--" + contract.OPTIONS[field], dest=field, action=action, default=None)


def _references(args: argparse.Namespace) -> dict:
    revision, candidate_revision = getattr(args, "revision", ""), getattr(args, "candidate_revision", "")
    connector_id, candidate = getattr(args, "id", ""), getattr(args, "candidate", "")
    if any(not contract.ID.fullmatch(value) for value in (connector_id, candidate) if value) or any(
        not contract.REVISION.fullmatch(value) for value in (revision, candidate_revision) if value
    ):
        raise contract.Refusal("invalid_request")
    return {"connector_id": connector_id, "revision": int(revision or 0), "candidate": candidate,
            "candidate_revision": int(candidate_revision or 0)}


def _mutate(args: argparse.Namespace, now: datetime, out: TextIO, ports: kinds.Ports) -> int:
    fields = {key: val for key, val in vars(args).items() if key in contract.OPTIONS and key != "id" and val is not None}
    try:
        result = registry.mutate(args.command, args.op, ports, now, fields=fields, **_references(args))
    except (contract.Refusal, registry.ConnectorError, engine_home.EngineHomeError, OSError) as error:
        print(contract.client_reason(str(error)), file=sys.stderr)
        return exits.USAGE
    print(json.dumps(result), file=out)
    return exits.OK


def _list(args: argparse.Namespace, now: datetime, out: TextIO, ports: kinds.Ports) -> int:
    registry.refresh(ports, now)
    found = registry.rows()
    if args.as_json:
        print(json.dumps(found), file=out)
    else:
        for row in found:
            print(f"{row['id']}: {'connected' if row['connected'] else 'not connected'} ({row['detail']})", file=out)
    return exits.OK


def _connect(args: argparse.Namespace, now: datetime, out: TextIO, ports: kinds.Ports) -> int:
    found = registry.connect(args.kind, args.label, args.target, ports, now)
    print(f"connected {found.id}: {found.detail}", file=out)
    return exits.OK


def _disconnect(args: argparse.Namespace, now: datetime, out: TextIO, ports: kinds.Ports) -> int:
    registry.disconnect(args.connector)
    print(f"disconnected {args.connector}", file=out)
    return exits.OK


_MUTATIONS = ("add", "edit", "revoke", "replace")
_COMMANDS = {"list": _list, "connect": _connect, "disconnect": _disconnect}
