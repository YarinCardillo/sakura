"""`sakura digest`: run the digest now or on schedule, and see its status.

All of it is operator commands: a person runs them (or the launchd agent does), never an agent inside an engine role
or its cage, which is refused here by the role's variables and by the cage itself, as `sakura stack` and
`sakura preset use` refuse it. Exit codes: 0 done or skipped, 2 refused (configuration, usage, inside a role), 4 failed
(the model or a writer), 6 another run holds the lock.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from collections.abc import Mapping
from dataclasses import replace
from datetime import datetime, timezone
from typing import Callable, TextIO

from sakura import engine_home
from sakura.automations import registry as automations
from sakura.digest import config, launchd, run, state, summarize
from sakura.flow import brakes, exits, process
from sakura.flow.surface import KIT_BIN
from sakura.scheduled import migrate
from sakura.scheduled import store as scheduled_store

ROLE_REFUSAL = "sakura digest cannot run inside an engine role: " + brakes.ENGINE_ROLE_REFUSAL
BUSY_REASON = "another digest run is in progress"
LOG_FILE = "run.log"
Spawn = Callable[[list[str]], None]


class _Parser(argparse.ArgumentParser):
    def error(self, message: str):
        raise config.ConfigError(f"usage: {message}")


def main(argv: list[str], *, now: Callable[[], datetime] | None = None, env: Mapping[str, str] | None = None,
         stdout: TextIO | None = None, steps: run.Steps | None = None,
         agent: launchd.Agent | None = None, spawn: Spawn | None = None) -> int:
    """Run `sakura digest <command>`; the clock, environment, output, the run's steps, the launchd agent's
    `launchctl` and folder, and the start of a detached run are injectable."""
    env = os.environ if env is None else env
    out = stdout or sys.stdout
    clock = now or (lambda: datetime.now(timezone.utc))
    try:
        if _is_inside_a_role(env):
            raise config.ConfigError(ROLE_REFUSAL)
        args = _parse(argv)
        arguments = (args, clock(), out, steps or run.Steps(), agent or launchd.Agent(), spawn or _spawn_detached)
        if args.command not in ("run", "status"):
            with migrate.lock():
                if migrate.owns_digest():
                    raise config.ConfigError(migrate.MIGRATED)
                return _COMMANDS[args.command](*arguments)
        return _COMMANDS[args.command](*arguments)
    except (config.ConfigError, scheduled_store.Refused) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.USAGE
    except (run.RunRefused, state.StateError, engine_home.EngineHomeError, summarize.SummarizeError,
            launchd.LaunchdError) as error:
        print(f"sakura: error: {error}", file=sys.stderr)
        return exits.FAILED


def _is_inside_a_role(env: Mapping[str, str]) -> bool:
    """Whether an engine role or its cage runs this: its variables, or the cage itself, which an agent cannot unset
    (it cannot write the engine's trust root: `engine_home.sandboxed`, as `stack` and `preset use` check)."""
    return bool(env.get(brakes.ROLE_ENV) or env.get(brakes.TASK_ENV)) or engine_home.sandboxed()


def _parse(argv: list[str]) -> argparse.Namespace:
    parser = _Parser(prog="sakura digest")
    sub = parser.add_subparsers(dest="command", required=True, parser_class=_Parser)
    run_parser = sub.add_parser("run")
    run_parser.add_argument("--scheduled", action="store_true",
                            help="do nothing before the configured time or when today is done")
    run_parser.add_argument("--detach", action="store_true", help="start the run in the background and return")
    run_parser.add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its action log records it
    run_parser.add_argument("none", nargs="*", help=argparse.SUPPRESS)  # takes the `--` the cockpit ends its options with
    sub.add_parser("status").add_argument("--json", action="store_true", dest="as_json")
    settings = sub.add_parser("settings", help="change the digest's time, lookback, language, model or account")
    settings.add_argument("--at")
    settings.add_argument("--lookback_hours")
    settings.add_argument("--language")
    settings.add_argument("--model")
    settings.add_argument("--account")
    settings.add_argument("--actor", help=argparse.SUPPRESS)
    settings.add_argument("none", nargs="*", help=argparse.SUPPRESS)
    source = sub.add_parser("source", help="switch a source on or off; it stays in the file")
    source.add_argument("--actor", help=argparse.SUPPRESS)
    source.add_argument("source_id")
    source.add_argument("setting", choices=("on", "off"))
    sub.add_parser("install", help="install the launchd agent that runs the digest every day at the configured time")
    sub.add_parser("uninstall", help="remove that agent")
    return parser.parse_args(argv)


def _run(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
         spawn: Spawn) -> int:
    if args.none:
        raise config.ConfigError(f"usage: run takes no arguments; got {args.none[0][:40]!r}")
    if args.detach:
        with migrate.lock():
            if migrate.owns_digest():
                raise config.ConfigError(migrate.MIGRATED)
            return _detached(out, spawn)
    outcome = run.run(config.load(), now, scheduled=args.scheduled, steps=steps)
    if not outcome.ran:
        print(f"digest skipped: {outcome.reason}", file=out)
        return exits.BUSY if outcome.reason == BUSY_REASON and not args.scheduled else exits.OK
    published = outcome.published
    failed = [report.id for report in outcome.reports if not report.ok]
    print(f"digest written: lane {published.lane}, {published.rows} checks, {published.questions} decisions"
          + (f"; could not read: {', '.join(failed)}" if failed else ""), file=out)
    return exits.OK


def _detached(out: TextIO, spawn: Spawn) -> int:
    """Start `sakura digest run` in the background, unless a run holds the lock or the file cannot be used."""
    config.load()  # a bad configuration is refused here, where the person is looking, not in a log
    if run.is_running():
        print(BUSY_REASON, file=out)
        return exits.BUSY
    spawn([str(KIT_BIN), "digest", "run"])
    print("digest started in the background; the lane appears on the Waiting page when it is done", file=out)
    return exits.OK


def _spawn_detached(argv: list[str]) -> None:
    """Start `argv` in a session of its own with its output in the digest folder's log, and do not wait for it."""
    descriptor = os.open(state.ensure_folder() / LOG_FILE, os.O_WRONLY | os.O_APPEND | os.O_CREAT, config.FILE_MODE)
    try:
        subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=descriptor, stderr=subprocess.STDOUT,
                         env=process.child_environment(os.environ), start_new_session=True)
    finally:
        os.close(descriptor)


def _status(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
         spawn: Spawn) -> int:
    found = run.status(config.load(), now)
    document = {"at": found.at, "last_run_at": found.last_run_at or None, "last_date": found.last_date or None,
                "next_at": found.next_at.isoformat(timespec="minutes"), "is_pending_today": found.is_pending_today,
                "sources": [{"id": name, "last_good_run": at or None} for name, at in found.sources]}
    if args.as_json:
        print(json.dumps(document), file=out)
        return exits.OK
    print(f"digest at {found.at}; last run {found.last_run_at or 'never'}; next {document['next_at']}"
          + (" (due now, runs at the next launch)" if found.is_pending_today else ""), file=out)
    for name, at in found.sources:
        print(f"  {name}: last good run {at or 'never'}", file=out)
    return exits.OK


def _install(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
         spawn: Spawn) -> int:
    settings = config.load()
    path = launchd.install(settings, launchctl=agent.launchctl, agents_dir=agent.folder, home=agent.home)
    automations.note_morning_digest(settings, is_installed=True)
    print(f"installed {path}: runs every day at {settings.at}, and at login or wake when it was missed", file=out)
    return exits.OK


def _uninstall(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
         spawn: Spawn) -> int:
    removed = launchd.uninstall(launchctl=agent.launchctl, agents_dir=agent.folder, home=agent.home)
    if removed:
        automations.note_morning_digest(config.load_or_default(), is_installed=False)
    print("agent removed" if removed else "no agent was installed", file=out)
    return exits.OK


SETTING_FIELDS = (("at", "at"), ("lookback_hours", "lookback_hours"), ("language", "language"), ("model", "model"),
                  ("account", "account"))


def _settings(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
              spawn: Spawn) -> int:
    """Change the named settings of the digest file, and replace the installed agent when the time moved."""
    if args.none:
        raise config.ConfigError(f"usage: settings takes options only; got {args.none[0][:40]!r}")
    given = {name: getattr(args, name) for name, _ in SETTING_FIELDS if getattr(args, name) is not None}
    if not given:
        raise config.ConfigError("usage: settings needs at least one of --at, --lookback_hours, --language, --model, --account")
    before = config.load_or_default()
    merged = {"at": before.at, "model": before.model, "language": before.language,
              "lookback_hours": before.lookback_hours, "account": before.account}
    merged.update(given)
    if "lookback_hours" in given:
        if not given["lookback_hours"].isdigit():
            raise config.ConfigError("lookback_hours must be a whole number of hours")
        merged["lookback_hours"] = int(given["lookback_hours"])
    after = replace(config.parse(merged), sources=before.sources)
    config.save(after)
    if after.at != before.at and launchd.is_installed(agents_dir=agent.folder, home=agent.home):
        launchd.install(after, launchctl=agent.launchctl, agents_dir=agent.folder, home=agent.home)
        automations.note_morning_digest(after, is_installed=True)
    print("digest settings changed: " + ", ".join(sorted(given)), file=out)
    return exits.OK


def _source(args: argparse.Namespace, now: datetime, out: TextIO, steps: run.Steps, agent: launchd.Agent,
            spawn: Spawn) -> int:
    """Switch one source on or off in the digest file."""
    settings = config.load()
    if args.source_id not in [source.id for source in settings.sources]:
        raise config.ConfigError(f"no source {args.source_id!r} in the digest file")
    is_on = args.setting == "on"
    config.save(replace(settings, sources=tuple(replace(source, enabled=is_on) if source.id == args.source_id else source
                                                for source in settings.sources)))
    print(f"source {args.source_id} is {args.setting}", file=out)
    return exits.OK


_COMMANDS = {"run": _run, "status": _status, "install": _install, "uninstall": _uninstall, "settings": _settings,
             "source": _source}
