"""Person commands for Tracks and their decisions."""

import argparse
import json
import sqlite3
import sys
from dataclasses import asdict

from sakura import actors, engine_home, operator_only
from sakura.cockpit import track_actions as control
from sakura.flow import exits, registry
from sakura.flow.render import strip_controls
from sakura.tracks import TrackService
from sakura.tracks.database import SchemaError
from sakura.tracks.domain import Refusal, RefusalCode


def parser() -> argparse.ArgumentParser:
    """Expose the same fields used by cockpit actions."""
    root = argparse.ArgumentParser(prog="sakura track")
    sub = root.add_subparsers(dest="command", required=True)
    decision = sub.add_parser("decision").add_subparsers(dest="decision_command", required=True)
    for name, key in (("new", control.NEW), ("set", control.SET), ("link", control.LINK), ("unlink", control.UNLINK)):
        _mutation(sub.add_parser(name), key)
    _mutation(decision.add_parser("add"), control.DECISION)
    _mutation(decision.add_parser("settle"), control.SETTLE)
    sub.add_parser("list").add_argument("--json", action="store_true")
    show = sub.add_parser("show")
    show.add_argument("track_id")
    show.add_argument("--audit", action="store_true")
    show.add_argument("--json", action="store_true")
    return root


def _mutation(command, key):
    command.set_defaults(action=key)
    if key != control.NEW:
        command.add_argument("track_id")
    spec = control.SPECS[key]
    for field in spec.needs + spec.optional:
        command.add_argument(f"--{field}", required=field in spec.needs)
    command.add_argument("--json", action="store_true")


def main(argv: list[str]) -> int:
    """Read or change Tracks in the selected engine home."""
    args = parser().parse_args(argv)
    try:
        result = _change(args) if hasattr(args, "action") else _read(args)
    except Refusal as error:
        return _refused(args.json, error)
    except (sqlite3.Error, SchemaError, engine_home.EngineHomeError, OSError):
        return _refused(args.json, Refusal(RefusalCode.LANE_UNAVAILABLE, control.STORAGE_PROBLEM))
    except (actors.ActorError, registry.RegistryError):
        return _refused(args.json, Refusal(RefusalCode.INVALID_INPUT, "The local actor or project registry is unavailable"))
    print(json.dumps(result, indent=None if args.json else 2, ensure_ascii=True))
    return exits.OK


def _read(args):
    service = TrackService()
    tracks = service.read_all()
    if args.command == "list":
        return [asdict(track) for track in tracks]
    track = next((track for track in tracks if track.id == args.track_id), None)
    if track is None:
        raise Refusal(RefusalCode.NOT_FOUND, "The Track was not found")
    result = asdict(track)
    if args.audit:
        result["audit"] = [asdict(entry) for entry in service.read_audit(track.id)]
    return result


def _change(args):
    if operator_only.is_inside_a_role():
        raise Refusal(RefusalCode.FORBIDDEN, "Only the person can perform this Track action")
    actor = actors.resolve(None, {})
    spec = control.SPECS[args.action]
    fields = {name: getattr(args, name) for name in spec.needs + spec.optional if getattr(args, name) is not None}
    projects = tuple(registry.repos(include_missing=True)) if "project" in fields else ()
    target = control.Target(getattr(args, "track_id", None), projects)
    control.validate(args.action, target.kind, fields)
    return control.mutate(args.action, target, fields, actor)


def _refused(as_json, error):
    reason = strip_controls(str(error))
    if as_json:
        print(json.dumps({"result": "refused", "reason": reason, "reason_code": error.code,
                          "reason_args": {"labels": list(error.labels)} if error.labels else {}}))
    else:
        print(reason, file=sys.stderr)
    return exits.USAGE
