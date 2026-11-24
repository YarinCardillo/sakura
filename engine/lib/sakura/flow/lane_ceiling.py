"""The lane ceiling: how many lanes the master may have open at once.

A lane is a flow task. It is open until its report is ready or it was cleaned, so a stopped, blocked or failed lane (a
failed one shows as blocked) still counts: the person resumes or cleans it, or the master asks. The ceiling is the preset's `lanes` knob and binds the
master only; the person may start more from the command line, knowingly.

Counting and creating a task happen under one lock (`<engine home>/lanes.lock`), so two opens at once, one below
the ceiling, admit exactly one.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import re
from collections.abc import Iterator
from pathlib import Path

from sakura import actors, engine_home
from sakura.flow import registry, store

LOCK_NAME = "lanes.lock"
LOCK_MODE = 0o600
FINISHED_STATUSES = ("report_ready", "cleaned")
# One word for a lane that cannot continue, in `flow status`, the cockpit snapshot and the master's view: the engine's
# own statuses `blocked` and `failed` both show as it (the reason says which, and `engine_status` keeps the raw one).
BLOCKED_STATUSES = ("blocked", "failed")
BLOCKED_WORD = "blocked"
DELIVERED_WORD = "delivered"
DELIVERED_STATUSES = ("report_ready", "cleaned")
REFUSAL_PREFIX = "the master's ceiling is"
OPENER_LINE = re.compile(r"- \[operator\] start requested actor=(\S+)")


class LaneCeilingRefused(ValueError):
    """Raised when opening a lane would pass the master's ceiling."""


def lane_name(repo: Path, task_id: str) -> str:
    """How a lane is named for people and for the other lanes: `<project folder name>/<task>`."""
    return f"{repo.name}/{task_id}"


def open_lanes(repos: list[Path]) -> list[str]:
    """The lanes of `repos` that count against the ceiling, as `<project>/<task>`.

    A lane whose state cannot be read counts as open: it may be running, and the person fixes it.
    """
    lanes = []
    for repo in repos:
        for task_store in store.task_stores(repo):
            if is_open(task_store):
                lanes.append(lane_name(repo, task_store.task_id))
    return lanes


@contextlib.contextmanager
def admit(opener: actors.Actor, ceiling: int, repos: list[Path] | None = None) -> Iterator[None]:
    """Hold the lanes lock while the caller creates one lane; refuse the master when `ceiling` lanes are open.

    Any other opener passes without the lock. `repos` defaults to every registered repository.
    Raises LaneCeilingRefused, naming the ceiling and the open lanes.
    """
    if opener.source != actors.MASTER_SOURCE:
        yield
        return
    with _locked():
        lanes = open_lanes(registry.repos() if repos is None else repos)
        if len(lanes) >= ceiling:
            raise LaneCeilingRefused(
                f"{REFUSAL_PREFIX} {ceiling} open lane{'s' if ceiling != 1 else ''} and {len(lanes)} are open: "
                f"{', '.join(lanes)}. Finish, stop and clean, or ask the person to raise `lanes`.")
        yield


MASTER_OWNERSHIP_REFUSAL = "only a lane the master opened in this session can be acted on by the master"


def opener(task_store: store.TaskStore) -> actors.Actor | None:
    """Who opened the lane, from the first start line of its ledger (which no agent writes); None when it names no
    valid actor or the ledger cannot be read."""
    return opener_in(task_store.ledger)


def opener_in(ledger: Path, *, contents: bytes | None = None) -> actors.Actor | None:
    """`opener` for the ledger file at `ledger` or the supplied display copy."""
    try:
        text = ledger.read_text() if contents is None else contents.decode()
        found = next(match for line in text.splitlines() if (match := OPENER_LINE.fullmatch(line)))
        return actors.parse(found.group(1))
    except (OSError, StopIteration, actors.ActorError):
        return None


def state_word(status: str, delivered: bool = False) -> str:
    """The word a person, the snapshot and the master see for a lane whose engine status is `status`: blocked for one that
    cannot continue, delivered for a finished one that delivered something (`flow/delivery.py`)."""
    if status in BLOCKED_STATUSES:
        return BLOCKED_WORD
    return DELIVERED_WORD if delivered and status in DELIVERED_STATUSES else status


def is_open(task_store: store.TaskStore) -> bool:
    """Whether the lane counts as open; one whose state cannot be read does."""
    try:
        return task_store.read_state().get("status") not in FINISHED_STATUSES
    except (store.StateError, OSError):
        return True


@contextlib.contextmanager
def _locked() -> Iterator[None]:
    descriptor = os.open(engine_home.root() / LOCK_NAME, os.O_RDWR | os.O_CREAT, LOCK_MODE)
    with os.fdopen(descriptor, "r+") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        yield
