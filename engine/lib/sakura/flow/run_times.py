"""When each part of a run's log was written.

A Claude or Codex run writes its log straight to a file and stamps nothing, so the runner keeps marks next to it
(`<run>.times`): every mark says that the log was `size` bytes long at a moment. Every mark has the same width, so
`time_at` finds the time of a byte offset by binary search without reading the file. The marks are advisory: a reader
ignores a line that is not a mark, and a failed write never stops a run.
"""

from __future__ import annotations

import os
import sys
from collections.abc import Callable
from datetime import datetime, timezone
from pathlib import Path

from sakura import text_limits

SIZE_DIGITS = 13
EPOCH_DIGITS = 11
MARK_BYTES = SIZE_DIGITS + 1 + EPOCH_DIGITS + 1
MIN_GAP_SECONDS = 0.5


def times_path(log: Path) -> Path:
    """The marks file that belongs to the log at `log`."""
    return log.with_suffix(".times")


def record(log: Path, size: int, now: datetime) -> None:
    """Note that the log was `size` bytes long at `now`; a failure is reported on stderr only."""
    line = f"{size:0{SIZE_DIGITS}d} {int(now.timestamp()):0{EPOCH_DIGITS}d}\n"
    try:
        with times_path(log).open("ab") as stream:
            stream.write(line.encode())
    except OSError as error:
        print(f"run times: cannot record a mark for {log.name} ({error})", file=sys.stderr)


def time_at(log: Path, offset: int) -> str:
    """The UTC time (`text_limits.utc_text`) of the first mark whose size reaches `offset`, or "" when none does.

    A byte at `offset` was written by the time the log first measured more than `offset` bytes, so this is when it
    was on disk, to within the gap between two marks.
    """
    try:
        with times_path(log).open("rb") as stream:
            count = os.fstat(stream.fileno()).st_size // MARK_BYTES
            return _search(stream, count, offset)
    except OSError:
        return ""


class Marker:
    """Marks a growing log: `tick` is called by the run's supervision loop and marks the log's size when it changed."""

    def __init__(self, log: Path, now: Callable[[], datetime] = lambda: datetime.now(timezone.utc)) -> None:
        self._log = log
        self._now = now
        self._marked_size = -1
        self._marked_at: datetime | None = None

    def tick(self) -> None:
        """Mark the log if it grew since the last mark and the last mark is at least `MIN_GAP_SECONDS` old.

        A growth inside that gap stays unmarked until a later tick, whether the log grew again or not, so a tool that
        starts and then stays silent still gets its time.
        """
        self._mark(is_forced=False)

    def flush(self) -> None:
        """Mark the log if it grew since the last mark, whatever the gap: the run is over."""
        self._mark(is_forced=True)

    def _mark(self, is_forced: bool) -> None:
        try:
            size = self._log.stat().st_size
        except OSError:
            return
        moment = self._now()
        gap_over = self._marked_at is None or (moment - self._marked_at).total_seconds() >= MIN_GAP_SECONDS
        if size != self._marked_size and (gap_over or is_forced):
            record(self._log, size, moment)
            self._marked_size, self._marked_at = size, moment


def _search(stream, count: int, offset: int) -> str:
    low, high, found = 0, count, None
    while low < high:
        middle = (low + high) // 2
        mark = _read_mark(stream, middle)
        if mark is None:
            return ""
        if mark[0] >= offset:
            found, high = mark, middle
        else:
            low = middle + 1
    return text_limits.utc_text(datetime.fromtimestamp(found[1], timezone.utc)) if found else ""


def _read_mark(stream, index: int) -> tuple[int, int] | None:
    stream.seek(index * MARK_BYTES)
    line = stream.read(MARK_BYTES)
    size, _, rest = line.decode(errors="replace").partition(" ")
    epoch = rest.strip()
    if len(line) != MARK_BYTES or not size.isdigit() or not epoch.isdigit():
        return None
    return int(size), int(epoch)
