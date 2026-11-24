"""Explicit quota hook runner and profile installer, independent of the service CLI."""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from pathlib import Path

from sakura.quota_enforcement import hooks, install
from sakura.quota_enforcement.client import Advisor, DEFAULT_URL


def main(argv: list[str]) -> int:
    """Run enforcement or install hooks only after an explicit install command."""
    args = _parser().parse_args(argv)
    env = dict(os.environ)
    if args.command == "run":
        return _run(args, env)
    try:
        paths = args.profile or install.discover(Path.home(), env)
        kit = Path(__file__).resolve().parents[3] / "bin/sakura"
        for path in install.profiles(paths, kit, args.advisor_url):
            print(path)
        return 0
    except (OSError, ValueError, TypeError) as error:
        print(f"quota hook installation failed: {error}", file=sys.stderr)
        return 1


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura quota-hooks")
    commands = parser.add_subparsers(dest="command", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("event", choices=install.EVENTS)
    runner.add_argument("--config-dir", type=Path)
    runner.add_argument("--role", choices=("builder", "reviewer", "explorer", "planner", "evaluator"))
    runner.add_argument("--task-class", choices=("mechanical", "integration", "design"), default="integration")
    installer = commands.add_parser("install")
    installer.add_argument("--profile", action="append", type=Path)
    for command in (runner, installer):
        command.add_argument("--advisor-url", default=os.environ.get("SAKURA_QUOTA_URL", DEFAULT_URL))
    return parser


def _run(args: argparse.Namespace, env: dict) -> int:
    try:
        if args.config_dir:
            env["CLAUDE_CONFIG_DIR"] = str(args.config_dir)
        payload = json.load(sys.stdin)
        output = hooks.evaluate(args.event, payload, Advisor(args.advisor_url), env,
                                role=args.role, task_class=args.task_class)
        if output:
            print(json.dumps(output))
    except Exception as error:
        logging.getLogger(__name__).warning("quota advisor unavailable; allowing work (%s)", type(error).__name__)
    return 0
