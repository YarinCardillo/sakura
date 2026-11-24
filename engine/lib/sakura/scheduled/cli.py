"""JSON reads of the Scheduled engine contract."""

import argparse
from datetime import UTC, datetime

from sakura import operator_only
from sakura.flow import exits
from sakura.scheduled import dispatch, launchd, migrate, run_tools, runner, store


def parser() -> argparse.ArgumentParser:
    """Build the scheduled CLI parser."""
    result = argparse.ArgumentParser(prog="sakura scheduled")
    commands = result.add_subparsers(dest="command", required=True)
    commands.add_parser("list").add_argument("--json", action="store_true", required=True)
    for command in ("history", "conversation"):
        read = commands.add_parser(command)
        read.add_argument("id")
        read.add_argument("--json", action="store_true", required=True)
        read.add_argument("--limit", type=int, choices=range(1, 101), default=50, metavar="1..100")
        read.add_argument("--before" if command == "history" else "--after", type=_cursor)
    serve = commands.add_parser("serve", help=argparse.SUPPRESS)
    serve.add_argument("id")
    serve.add_argument("policy", help=argparse.SUPPRESS)
    commands.add_parser("tick")
    commands.add_parser("reconcile")
    commands.add_parser("migrate")
    return result


def main(argv: list[str]) -> int:
    """Print Scheduled without running connector or agent work."""
    from sakura.cockpit import api

    args = parser().parse_args(argv)
    if args.command == "serve":
        return run_tools.serve(args.id)
    if args.command in ("history", "conversation"):
        return _read(args)
    if args.command != "list":
        return _mutate(args.command)
    document = api.scheduled_document()
    print(api.dumps(document))
    return exits.FAILED if document.get("problem") else exits.OK


def _mutate(command: str) -> int:
    if operator_only.is_inside_a_role():
        print("scheduled_action_unavailable")
        return exits.USAGE
    tasks = store.Store()
    now = datetime.now(UTC)
    try:
        if command == "migrate":
            migrate.upgrade(tasks, now)
            if problem := migrate.status(tasks):
                raise store.Refused(problem)
        elif command == "reconcile":
            if migrate.owns_digest(tasks):
                migrate.upgrade(tasks, now)
            launchd.reconcile(tasks, verify_loaded=True)
        else:
            dispatch.claim_clock(tasks, now)
            dispatch.poll_events(tasks, now)
            runner.drain(tasks)
            launchd.retire_idle(tasks)
    except store.Refused as error:
        print(str(error))
        return exits.USAGE
    return exits.OK


def _cursor(value: str) -> int:
    if not value.isascii() or not value.isdecimal() or len(value) > 18:
        raise argparse.ArgumentTypeError("expected a nonnegative sequence")
    return int(value)


def _read(args: argparse.Namespace) -> int:
    from sakura.cockpit import api

    try:
        tasks = store.Store()
        if args.command == "history":
            data = dispatch.runs(tasks, args.id, limit=args.limit, before=args.before)
            result = {"runs": data, "before": data[-1]["sequence"] if len(data) == args.limit else None}
        else:
            data = runner.conversation(tasks, args.id, limit=args.limit, after=args.after or 0)
            result = ({"problem": "run_unavailable"} if data is None else
                      {"entries": data, "after": data[-1]["sequence"] if len(data) == args.limit else None})
    except store.Refused as error:
        result = {"problem": str(error)}
    print(api.dumps(result))
    return exits.USAGE if result.get("problem") else exits.OK
