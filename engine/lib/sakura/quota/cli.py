"""`sakura quota`: the quota service from the command line."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

from sakura.accounts import registry
from sakura.quota import advise, config, models, render
from sakura.quota.service import Service
from sakura.usage import leeway
from sakura.usage.archive import quota, settings

DELEGATED = ("status", "advise")
DELEGATED_NOTE = "usage comes from Leeway ({helper}); `sakura quota` only passes the command on"
STALE_NOTE = "* stale: the source did not answer, showing the last snapshot"


def main(argv: list[str], run: quota.Runner = quota.subprocess_runner,
         clock: Callable[[], datetime] = quota.utc_now, home: Path | None = None) -> int:
    """Run an `sakura quota` command; `run` and `clock` are injected so tests never start a real process."""
    args = _parser().parse_args(argv)
    if args.command in DELEGATED and leeway.is_available():
        return _delegate(args)
    base = home or Path.home()
    try:
        rules = config.load(args.config or config.default_path(base), base)
    except config.QuotaConfigError as error:
        print(error, file=sys.stderr)
        return 1
    service = Service(settings.load(), registry.load(), rules, run, clock, base)
    if args.command == "advise":
        return _advise(service, args)
    if args.command == "serve":
        return _serve(service, args)
    return _status(service, args)


def _delegate(args: argparse.Namespace) -> int:
    """Pass `status` and `advise` to the Leeway helper and say so: Leeway is the one reader of usage."""
    words = [args.command]
    if args.command == "status":
        words = ["refresh" if args.refresh else "status", *(["--json"] if args.json else [])]
    else:
        words += ["--repo", str(args.repo.expanduser()), "--role", args.role, "--task-class", args.task_class]
        words += ["--agent", args.agent] if args.agent else []
    print(DELEGATED_NOTE.format(helper=leeway.helper()), file=sys.stderr)
    try:
        code, text = leeway.run_text(words)
    except leeway.LeewayError as error:
        print(error, file=sys.stderr)
        return 1
    print(text, end="")
    return code


def _status(service: Service, args: argparse.Namespace) -> int:
    document = service.status(refresh=args.refresh)
    print(json.dumps(document, indent=2) if args.json else _text(document))
    return 0


def _advise(service: Service, args: argparse.Namespace) -> int:
    request = advise.Request(args.repo.expanduser(), args.role, args.task_class, args.agent)
    print(json.dumps(service.advise(request), indent=2))
    return 0


def _serve(service: Service, args: argparse.Namespace) -> int:
    from sakura.quota import server

    return server.run(service, args.port or service.config.port, args.refresh_minutes)


def _text(document: dict) -> str:
    text = render.status_text(document)
    return text + f"\n{STALE_NOTE}" if any(a["stale"] for a in document["accounts"]) else text


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura quota")
    parser.add_argument("--config", type=Path, help="owner rules file (default ~/.config/sakura/quota.toml)")
    sub = parser.add_subparsers(dest="command", required=True)
    show = sub.add_parser("status", help="live quota and weekly forecast of every account")
    show.add_argument("--json", action="store_true")
    show.add_argument("--refresh", action="store_true", help="ask every source now, ignoring recent snapshots")
    ask = sub.add_parser("advise", help="where may this work run now")
    ask.add_argument("--repo", type=Path, required=True)
    ask.add_argument("--role", choices=advise.ROLES, required=True)
    ask.add_argument("--task-class", choices=advise.TASK_CLASSES, required=True)
    ask.add_argument("--agent", choices=(models.CLAUDE, models.CODEX))
    serve = sub.add_parser("serve", help="HTTP on 127.0.0.1 with /status and /advise")
    serve.add_argument("--port", type=int)
    serve.add_argument("--refresh-minutes", type=int, default=10)
    return parser


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
