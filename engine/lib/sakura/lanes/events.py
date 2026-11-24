"""Append-only JSON-lines log of lane commands, one line per attempt with its actor."""

from __future__ import annotations

import fcntl
import json
import time
from pathlib import Path

from sakura import engine_home
from sakura.flow.render import strip_controls

DEFAULT_FILE = Path("lanes") / "events.jsonl"
MAX_DETAIL = 500


def append(kind: str, group: str, name: str, actor: str, outcome: str, detail: str = "",
           path: Path | None = None) -> None:
    """Write one event line under an exclusive lock; text fields lose terminal controls and `detail` is capped."""
    target = _resolve(path)
    row = {"time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "kind": kind, "group": group,
           "name": name, "actor": actor, "outcome": outcome, "detail": detail[:MAX_DETAIL]}
    line = json.dumps({key: strip_controls(value) for key, value in row.items()}) + "\n"
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a", encoding="utf-8") as log:
        fcntl.flock(log, fcntl.LOCK_EX)
        try:
            log.write(line)
            log.flush()
        finally:
            fcntl.flock(log, fcntl.LOCK_UN)


def recent(count: int, path: Path | None = None) -> list[dict]:
    """Return the last `count` events, oldest first; lines that are not JSON objects are skipped."""
    try:
        with _resolve(path).open(encoding="utf-8") as log:
            rows = [row for row in map(_parse, log) if row is not None]
    except FileNotFoundError:
        return []
    return rows[-count:] if count > 0 else []


def _parse(line: str) -> dict | None:
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        return None
    return row if isinstance(row, dict) else None


def _resolve(path: Path | None) -> Path:
    return Path(path).expanduser() if path else engine_home.state_dir() / DEFAULT_FILE
