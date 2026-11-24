"""Give priority to the person's own sessions: a lane waits before starting a new run while the person works with an agent.

When the host switch is on (`yield-to-person.json` in the engine home, set by `sakura yield on|off`), a lane about to start a
phase looks, every minute, whether a Claude transcript (`<config dir>/projects/*/*.jsonl`) or a Codex session file
(`<codex home>/sessions/**`) changed in the last five minutes in a folder that is not one of the engine's lane workspaces.
If one did, the lane waits (`yield_wait` in its state, which the snapshot shows as `yielding`) and starts within one check
after the person stops. A run that has already started is never stopped, the operator's stop file ends the wait at once, and
a session inside any lane's workspace, the lane's own agents or the person working in a lane's worktree, is never counted:
a lane must not yield to itself. A transcript whose folder cannot be read is not counted either, for the same reason.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TextIO

from sakura import engine_home, operator_only
from sakura.flow import brakes, exits, gates, store, task_repository

SETTINGS_FILE = "yield-to-person.json"
FILE_MODE = 0o600
YIELD_KEY = "yield_wait"
CHECK_SECONDS = 60
ACTIVE_WINDOW = timedelta(minutes=5)
HEAD_BYTES = 1 << 16
HEAD_LINES = 20
TRANSCRIPT_PATTERNS = (("projects", "*/*.jsonl"), ("sessions", "**/*.jsonl"))


@dataclass(frozen=True)
class Ports:
    """What `hold` reads and waits with, injectable so no test sleeps or reads a transcript."""

    is_enabled: Callable[[], bool]
    person_active: Callable[[datetime], bool]
    sleep: Callable[[float], None] = time.sleep
    now: Callable[[], datetime] = lambda: datetime.now(timezone.utc)


def real_ports() -> Ports:
    """The ports on this machine: the switch in the engine home, the accounts' folders and the registered projects."""
    from sakura.accounts import registry as accounts

    def active(now: datetime) -> bool:
        homes = [account.config_dir for account in accounts.load().accounts if account.enabled]
        return person_active(now, homes=homes, lane_workspaces=lane_workspaces(_registered()))

    return Ports(is_enabled=is_enabled, person_active=active)


# The switch

def settings_path() -> Path:
    """The file that keeps the switch, in the engine home; not created."""
    return engine_home.root() / SETTINGS_FILE


def is_enabled() -> bool:
    """Whether the lanes yield to the person; off when never set or when the file cannot be read."""
    try:
        return json.loads(settings_path().read_text()).get("on") is True
    except (OSError, ValueError, AttributeError):
        return False


def set_enabled(is_on: bool) -> None:
    """Keep the switch (mode 0600). Raises OSError when the file cannot be written."""
    gates.write_atomically(settings_path(), json.dumps({"on": is_on}).encode())
    settings_path().chmod(FILE_MODE)


# Looking for the person

def person_active(now: datetime, *, homes: list[Path], lane_workspaces: set[Path],
                  window: timedelta = ACTIVE_WINDOW) -> bool:
    """Whether a transcript under `homes` changed within `window` of `now` in a folder outside every lane workspace."""
    cutoff = (now - window).timestamp()
    for path in _transcripts(homes):
        try:
            if path.stat().st_mtime < cutoff:
                continue
        except OSError:
            continue
        folder = _working_folder(path)
        if folder is not None and not _inside_any(folder, lane_workspaces):
            return True
    return False


def lane_workspaces(repos: list[Path]) -> set[Path]:
    """Every workspace the engine recorded for any lane of `repos`, resolved: where its agents work."""
    found: set[Path] = set()
    for repo in repos:
        for lane in store.task_stores(repo):
            try:
                state = lane.read_state()
            except (store.StateError, OSError):
                continue
            entries = [*state.get("workspaces", {}).values(),
                       *(state.get(task_repository.ROLE_REPOSITORIES_KEY) or {}).values()]
            found.update(Path(os.path.realpath(path)) for entry in entries
                         if (path := entry.get("path") or entry.get("workspace")))
    return found


def _registered() -> list[Path]:
    from sakura.flow import registry

    return registry.repos()


def _transcripts(homes: list[Path]):
    for home in homes:
        for subdirectory, pattern in TRANSCRIPT_PATTERNS:
            folder = home / subdirectory
            if folder.is_dir():
                yield from folder.glob(pattern)


def _working_folder(path: Path) -> Path | None:
    """The folder a session works in, from the first lines of its file; None when they name none."""
    try:
        with path.open("rb") as stream:
            head = stream.read(HEAD_BYTES).decode("utf-8", "replace")
    except OSError:
        return None
    for line in head.splitlines()[:HEAD_LINES]:
        try:
            document = json.loads(line)
        except ValueError:
            continue
        found = _cwd_of(document)
        if isinstance(found, str) and os.path.isabs(found):
            return Path(os.path.realpath(found))
    return None


def _cwd_of(document: object) -> object:
    """The `cwd` of a Claude line, or of the `payload` of a Codex session line."""
    if not isinstance(document, dict):
        return None
    payload = document.get("payload")
    return document.get("cwd") or (payload.get("cwd") if isinstance(payload, dict) else None)


def _inside_any(folder: Path, workspaces: set[Path]) -> bool:
    return any(folder == workspace or workspace in folder.parents for workspace in workspaces)


# Waiting

def hold(task_store: store.TaskStore, ports: Ports) -> bool:
    """Wait while the switch is on and the person is active, checking every `CHECK_SECONDS`; True when it waited.

    The wait is recorded in the lane's state (`yield_wait`) and ends at once when the operator's stop file appears."""
    if not ports.is_enabled():
        return False
    waited = False
    try:
        while ports.person_active(ports.now()) and not task_store.stop_file.exists():
            if not waited:
                _mark(task_store, ports.now())
            waited = True
            ports.sleep(CHECK_SECONDS)
    finally:
        if waited:
            clear(task_store)
    return waited


def clear(task_store: store.TaskStore) -> None:
    """Remove a wait mark a dead driver left behind."""
    if task_store.read_state().get(YIELD_KEY):
        task_store.update_state(lambda state: state.pop(YIELD_KEY, None))


def _mark(task_store: store.TaskStore, now: datetime) -> None:
    since = now.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    task_store.update_state(lambda state: state.update({YIELD_KEY: {"waiting": "person", "since": since}}))


# The command

def main(argv: list[str], *, stdout: TextIO | None = None) -> int:
    """Run `sakura yield <on | off | status>`; operator only."""
    out = stdout or sys.stdout
    parser = argparse.ArgumentParser(prog="sakura yield")
    parser.add_argument("setting", choices=("on", "off", "status"))
    parser.add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its action log records it
    parser.add_argument("none", nargs="*", help=argparse.SUPPRESS)  # takes the `--` the cockpit ends its options with
    args = parser.parse_args(argv)
    if operator_only.is_inside_a_role():
        print(f"sakura: error: sakura yield cannot run inside an engine role: {brakes.ENGINE_ROLE_REFUSAL}", file=sys.stderr)
        return exits.USAGE
    if args.setting != "status":
        set_enabled(args.setting == "on")
    print(f"yield to the person: {'on' if is_enabled() else 'off'}", file=out)
    return exits.OK
