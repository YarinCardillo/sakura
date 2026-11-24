"""`sakura lane` command line: open, close, show and talk to manual lanes; every write is logged with its actor."""

from __future__ import annotations

import argparse
import os
import shlex
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from sakura import actors
from sakura.flow import brakes, exits
from sakura.flow.render import strip_controls
from sakura.flow.tmux import TmuxClient, TmuxError
from sakura.lanes import config, events, identity, lane, status
from sakura.lanes import input as lane_input
from sakura.lanes.errors import (
    InputInvalid,
    InputRefused,
    LaneBusy,
    LaneConflict,
    LaneError,
)
from sakura.lanes.transcripts import Sources

NEEDS_TMUX = "needs tmux on PATH"
PREVIEW_CHARS = 80
_REFUSALS = (config.LaneConfigError, identity.LaneNameError, actors.ActorError, InputInvalid)
_BLOCKS = (LaneConflict, LaneBusy, InputRefused)


@dataclass(frozen=True)
class Services:
    """What `say`, `model` and `status` reach outside the process through; tests point these at temp files."""

    sources: Sources | None = None
    messages_log: Path | None = None
    gh: status.Gh = status.run_gh
    sleep: Callable[[float], None] = time.sleep


class _Attempt:
    """One lane command: who asked, which lane, and the single event line it leaves behind."""

    def __init__(self, kind: str, name: str, group: str, actor: str, log_path: Path | None):
        self._kind, self._name, self._group, self._actor, self._log_path = kind, name, group, actor, log_path
        self._subject = ""

    def group(self, name: str) -> None:
        """Record the group once it has been resolved."""
        self._group = name

    def subject(self, text: str) -> None:
        """Remember the text sent: its length and first characters go ahead of the detail, never more."""
        self._subject = f"{len(text)} characters, starts with: {text[:PREVIEW_CHARS]}"

    def record(self, outcome: str, detail: str = "") -> None:
        """Append the event line for this attempt."""
        detail = "; ".join(part for part in (self._subject, detail) if part)
        events.append(self._kind, self._group, self._name, self._actor, outcome, detail, path=self._log_path)


def main(argv: list[str], *, client: TmuxClient | None = None, config_path: Path | None = None,
         events_path: Path | None = None, sources: Sources | None = None, messages_log: Path | None = None,
         gh: status.Gh = status.run_gh, sleep: Callable[[float], None] = time.sleep) -> int:
    """Run `sakura lane <open|done|status|say|model> ...`; paths, the tmux client and gh are injectable.

    The writers are operator commands: they are refused inside an engine role session.
    """
    args = _parser().parse_args(argv)
    services = Services(sources, messages_log, gh, sleep)
    if args.command == "status":
        return _status(args, client or TmuxClient(), config_path, services)
    if os.environ.get(brakes.ROLE_ENV):
        return _fail(brakes.ENGINE_ROLE_REFUSAL, exits.USAGE)
    try:
        actor = actors.format(actors.resolve(args.actor))
    except actors.ActorError as error:
        return _fail(str(error), exits.USAGE)
    attempt = _Attempt(args.command, args.name, args.group or "", actor, events_path)
    try:
        return _execute(args, attempt, client or TmuxClient(), config_path, services)
    except _REFUSALS + _BLOCKS as error:
        return _finish(attempt, "refused", str(error), exits.USAGE if isinstance(error, _REFUSALS) else exits.BLOCKED)
    except (LaneError, TmuxError) as error:
        return _finish(attempt, "failed", str(error), exits.FAILED)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura lane")
    sub = parser.add_subparsers(dest="command", required=True)
    writers = {command: sub.add_parser(command) for command in ("open", "done", "say", "model")}
    for command in writers.values():
        command.add_argument("name")
    writers["open"].add_argument("--from", dest="from_ref")
    writers["done"].add_argument("--remove", action="store_true")
    writers["say"].add_argument("text")
    writers["model"].add_argument("model")
    for command in writers.values():
        command.add_argument("--group")
        command.add_argument("--actor")
    viewer = sub.add_parser("status")
    viewer.add_argument("--group")
    viewer.add_argument("--json", action="store_true")
    return parser


def _execute(args: argparse.Namespace, attempt: _Attempt, client: TmuxClient, config_path: Path | None,
             services: Services) -> int:
    if args.command in ("say", "model"):
        attempt.subject(args.text if args.command == "say" else args.model)
    group = config.select(config.load(config_path), args.group)
    attempt.group(group.name)
    lane_id = identity.resolve(group, args.name)
    if not client.available():
        raise LaneError(NEEDS_TMUX)
    if args.command == "open":
        return _open(args, attempt, client, group, lane_id)
    if args.command == "done":
        return _done(args, attempt, client, group, lane_id)
    return _talk(args, attempt, client, group, lane_id, services)


def _open(args, attempt: _Attempt, client: TmuxClient, group, lane_id) -> int:
    lane.open(lane_id, group, args.from_ref, client)
    attempt.record("ok", f"window {lane_id.window_marker}")
    _say(f"lane {lane_id.window_marker} ready at {lane_id.worktree}")
    _say("switch with: " + shlex.join(client.switch_or_attach(group.session)))
    return exits.OK


def _done(args, attempt: _Attempt, client: TmuxClient, group, lane_id) -> int:
    closed = "closed and removed" if args.remove else "closed"

    def announce() -> None:
        attempt.record("ok", closed)
        _say(f"lane {lane_id.window_marker} {closed}")

    lane.done(lane_id, group, args.remove, client, before_kill=announce)
    return exits.OK


def _talk(args, attempt: _Attempt, client: TmuxClient, group, lane_id, services: Services) -> int:
    seams = {"sleep": services.sleep, "messages_log": services.messages_log}
    if args.command == "say":
        lane_input.say(lane_id, group, args.text, client, **seams)
        attempt.record("ok", "sent")
        return exits.OK
    outcome = lane_input.model(lane_id, group, args.model, client, sources=services.sources, **seams)
    attempt.record("ok", outcome)
    _say(f"lane {lane_id.window_marker}: model {outcome}")
    return exits.OK


def _status(args: argparse.Namespace, client: TmuxClient, config_path: Path | None, services: Services) -> int:
    try:
        group = config.select(config.load(config_path), args.group)
        if not client.available():
            raise LaneError(NEEDS_TMUX)
        rows = status.collect(group, client, services.gh, services.sources)
    except _REFUSALS as error:
        return _fail(str(error), exits.USAGE)
    except (LaneError, TmuxError) as error:
        return _fail(str(error), exits.FAILED)
    _say(status.render_json(rows) if args.json else status.render(rows))
    return exits.OK


def _finish(attempt: _Attempt, outcome: str, detail: str, code: int) -> int:
    attempt.record(outcome, detail)
    return _fail(detail, code)


def _fail(message: str, code: int) -> int:
    print(strip_controls(message), file=sys.stderr)
    return code


def _say(text: str) -> None:
    print(strip_controls(text))
