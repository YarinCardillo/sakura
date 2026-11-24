"""When each line of a task's ledger was written.

The ledger (`ledger.md`) is one line per event and carries no time, so every line the engine appends is also recorded
in `ledger.times` next to it, as `<byte offset of the line>\t<UTC time>`. A line written before this file existed, or
by a writer that does not record, simply has no time. The file is advisory: a reader ignores any line that is not an
offset and a time, and a failed write never stops the engine (the ledger line itself is already written).
"""

from __future__ import annotations

import re
import sys
from datetime import datetime, timezone
from pathlib import Path

from sakura import text_limits

TIMES_FILE = "ledger.times"
MAX_TIMES_BYTES = 4 * 1024 * 1024
TIMES_LINE = re.compile(r"([0-9]{1,15})\t(\S{1,40})")


def times_path(ledger: Path) -> Path:
    """The times file that belongs to the ledger at `ledger`."""
    return ledger.with_name(TIMES_FILE)


def record(ledger: Path, offset: int, now: datetime | None = None) -> None:
    """Note that the ledger line starting at byte `offset` was written now; a failure is reported on stderr only."""
    stamp = text_limits.utc_text(now or datetime.now(timezone.utc))
    try:
        with times_path(ledger).open("a") as stream:
            stream.write(f"{offset}\t{stamp}\n")
    except OSError as error:
        print(f"ledger times: cannot record the time of a ledger line ({error})", file=sys.stderr)


def read(ledger: Path) -> dict[int, str]:
    """The time (UTC ISO text) of each ledger line that has one, by the byte offset of the line."""
    try:
        with times_path(ledger).open(errors="replace") as stream:
            text = stream.read(MAX_TIMES_BYTES)
    except OSError:
        return {}
    found = {}
    for line in text.splitlines():
        match = TIMES_LINE.fullmatch(line)
        moment = _moment(match[2]) if match else None
        if moment:
            found[int(match[1])] = moment
    return found


def _moment(text: str) -> str | None:
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    return text_limits.utc_text(parsed) if parsed.tzinfo else None
