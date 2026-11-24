"""What a lane delivered: one record for every kind of delivery, written in one place and read everywhere.

A lane delivers by opening a pull request (`pull_request`), by pushing its branch to a remote on another host with no
pull request opened (`push`), or by pushing it to a remote on this machine (`local`). Each kind records the delivered
head and the branch (and the pull request URL when there is one) under `delivery`. The overlap check compares a lane's head
with every other lane's delivered head (`flow/overlap.py`), and `flow status`, the snapshot and the master show the lane as
delivered with that branch and head or that pull request, whatever the kind.

`pr` stays as the older record of the same thing (the pull request URL, or the note a push without one leaves). A task that
delivered before `delivery` existed is read from it: a head with a URL is a pull request, a head with only a note is a push.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import time
from datetime import datetime, timezone
from collections.abc import Callable, Iterator
from pathlib import Path

from sakura.flow.store import TaskStore, flow_root

KEY = "delivery"
PULL_REQUEST, PUSH, LOCAL = "pull_request", "push", "local"
KINDS = (PULL_REQUEST, PUSH, LOCAL)
DELIVERED_WORD = "delivered"
LOCK_NAME = ".delivery.lock"
LOCK_POLL_SECONDS = 0.1


class DeliveryLockUnavailable(RuntimeError):
    """Another lane of the project kept delivering for the whole wait, or the person stopped this one while it waited."""


@contextlib.contextmanager
def project_lock(repo: Path, wait_seconds: float, should_stop: Callable[[], bool] = lambda: False) -> Iterator[None]:
    """Hold the project's delivery lock: lanes of one project deliver one at a time, from the overlap check through the push to
    the durable delivery record, so two conflicting lanes cannot both pass the check before either is recorded. The lock
    is the operating system's (`flock`), dropped when its holder dies. Raises DeliveryLockUnavailable after `wait_seconds`
    or once `should_stop` says so."""
    path = flow_root(repo) / LOCK_NAME
    path.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + wait_seconds
    with path.open("a") as held:
        while True:
            try:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if should_stop() or time.monotonic() >= deadline:
                    raise DeliveryLockUnavailable("another lane of the project is delivering; try again when it is done") from None
                time.sleep(LOCK_POLL_SECONDS)
        try:
            yield
        finally:
            fcntl.flock(held, fcntl.LOCK_UN)


def kind_of_push(push_url: str) -> str:
    """`local` for a remote that is a folder or `file://` URL on this machine, else `push`."""
    return LOCAL if os.path.isabs(push_url) or push_url.startswith("file://") else PUSH


def record(store: TaskStore, kind: str, head: str, branch: str, url: str = "", note: str = "") -> None:
    """Record that the lane delivered `head` of `branch` as `kind` (and the pull request `url` when there is one), in
    `delivery` and in the older `pr` record. The record says when the head was first delivered (`at`): a pull request found
    for a head already pushed keeps the moment of the push, the order other lanes' overlap checks go by."""
    if kind not in KINDS:
        raise ValueError(f"unknown delivery kind {kind!r}")

    def change(state: dict) -> None:
        before = state.get(KEY) if isinstance(state.get(KEY), dict) else {}
        moment = before.get("at") if before.get("head") == head and before.get("at") else _now()
        state[KEY] = {"kind": kind, "head": head, "branch": branch, "url": url, "at": moment}
        state["pr"] = {"url": url, "head": head, **({"note": note} if note else {})}

    store.update_state(change)


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def of(state: dict) -> dict | None:
    """The delivery of a task (`kind`, `head`, `branch`, `url`), or None when it delivered nothing."""
    found = state.get(KEY)
    if isinstance(found, dict) and found.get("head"):
        return {"kind": found.get("kind", PUSH), "head": found["head"], "branch": found.get("branch", ""),
                "url": found.get("url", "")}
    legacy = state.get("pr") or {}
    head = legacy.get("head")
    if not isinstance(head, str) or not head or head == state.get("base_commit"):
        return None
    if legacy.get("url"):
        return {"kind": PULL_REQUEST, "head": head, "branch": state.get("branch", ""), "url": legacy["url"]}
    if legacy.get("note"):
        return {"kind": PUSH, "head": head, "branch": state.get("branch", ""), "url": ""}
    return None


def delivered_at(state: dict) -> str:
    """When the task first delivered the head it delivered (`record`), "" when it delivered nothing or the record has no
    moment (a delivery from before it was kept)."""
    found = state.get(KEY)
    return str(found.get("at") or "") if isinstance(found, dict) and found.get("head") else ""


def head_of(state: dict) -> str:
    """The head the task delivered, or an empty string."""
    found = of(state)
    return found["head"] if found else ""


def describe(found: dict) -> str:
    """One line for a person: the pull request when there is one, else the pushed branch and head."""
    if found.get("url"):
        return f"pull request {found['url']}"
    where = "to a remote on this machine" if found.get("kind") == LOCAL else "to its remote"
    return f"branch {found.get('branch', '')} pushed {where} at {str(found.get('head', ''))[:12]}"
