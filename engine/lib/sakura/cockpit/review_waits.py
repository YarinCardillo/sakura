"""Reconcile the review of a lane's current pull request delivery."""

import sys
import threading
from bisect import bisect_right
from collections.abc import Callable, Iterable, Iterator
from pathlib import Path

from sakura.cockpit import decided
from sakura.flow import exits, store, waits
from sakura.flow.pr_status import read as _pr_state

CLOSED_STATES = {"MERGED", "CLOSED"}
PROBES_PER_REFRESH = 4


def collect(stores: dict[Path, list[store.TaskStore]], *, read_state: store.ReadState | None = None) -> list[waits.Wait]:
    """Return locally unresolved reviews without contacting GitHub or writing state."""
    return [wait for _, wait in _candidates(
        (task for tasks in stores.values() for task in tasks), read_state)]


def _candidates(tasks: Iterable[store.TaskStore], read_state: store.ReadState | None = None) -> Iterator[tuple[store.TaskStore, waits.Wait]]:
    for task in tasks:
        try:
            state = read_state(task) if read_state else task.read_state()
            wait = waits.review_wait(task, state)
        except store.StateError:
            continue
        if wait is not None:
            yield task, wait


class Refresher:
    """Run one bounded round of probes at a time, advancing past unknown reviews."""

    def __init__(self, repos: Callable[[], list[Path]]) -> None:
        self._repos = repos
        self._cursor = ""
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        """Schedule a refresh unless the previous round is still running."""
        if self._thread and self._thread.is_alive():
            return
        self._thread = threading.Thread(target=self.refresh, name="pull-request-reviews", daemon=True)
        self._thread.start()

    def refresh(self) -> None:
        """Probe a bounded batch. Only GitHub's explicit terminal states resolve a wait."""
        try:
            self._refresh()
        except (OSError, ValueError, RuntimeError) as error:
            print(f"keeper: review refresh failed: {error}", file=sys.stderr)

    def _refresh(self) -> None:
        candidates = {wait.id: (task, wait) for task, wait in _candidates(self._tasks())}
        keys = sorted(candidates)
        offset = bisect_right(keys, self._cursor)
        for key in (keys[offset:] + keys[:offset])[:PROBES_PER_REFRESH]:
            self._cursor = key
            task, wait = candidates[key]
            state = _pr_state(wait.links[0][1], task.repo)
            if state in CLOSED_STATES:
                resolve(task, wait, "github", state.lower())

    def _tasks(self) -> Iterator[store.TaskStore]:
        for repo in self._repos():
            try:
                yield from store.task_stores(repo)
            except (OSError, ValueError, RuntimeError) as error:
                print(f"keeper: cannot list reviews in {repo}: {error}", file=sys.stderr)


def resolve(task: store.TaskStore, wait: waits.Wait, actor: str, answer: str = "reviewed") -> tuple[int, str]:
    """Mark the exact delivery reviewed and keep its first resolution."""
    def mark(state: dict) -> None:
        current = waits.review_wait(task, state)
        if current is None or current.id != wait.id:
            raise _DeliveryChanged
        state[waits.REVIEWED_DELIVERY] = wait.id
        decided.record_now(decided.wait_fields(wait), answer, actor)

    try:
        with task.as_side_writer():
            task.update_state(mark)
    except (_DeliveryChanged, store.StateError, FileNotFoundError):
        return exits.USAGE, waits.NO_LONGER_WAITING
    return exits.OK, ""


class _DeliveryChanged(Exception):
    pass
