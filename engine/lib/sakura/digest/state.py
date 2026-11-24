"""What the digest remembers between runs, in `<engine home>/digest/state.json`.

Per source: the cursor its program returned and the time of its last good run. A run's cursors are committed only
after the whole run succeeded (the model's output validated and published), so a run that fails halfway finds the same
items again. Also the local date of the last finished run, which is the done-today mark of the scheduled run.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field, replace
from datetime import date, datetime
from pathlib import Path

from sakura import beneath
from sakura.digest import config
from sakura.digest.schema import SourceReport

MAX_CURSOR = 2000
STATE_VERSION = 1


class StateError(OSError):
    """Raised when the state file cannot be read or written."""


@dataclass(frozen=True)
class SourceState:
    """One source's memory: its cursor ("" when it has none) and the UTC time of its last good run ("" when never)."""

    cursor: str = ""
    at: str = ""


@dataclass(frozen=True)
class State:
    """The whole memory: the sources by id and the local date (`YYYY-MM-DD`) of the last finished run."""

    sources: dict[str, SourceState] = field(default_factory=dict)
    done_date: str = ""
    last_run_at: str = ""

    def of(self, source_id: str) -> SourceState:
        """The memory of one source; an unknown source has none."""
        return self.sources.get(source_id, SourceState())


def ensure_folder() -> Path:
    """The digest folder, created with mode 0700 when missing, and returned."""
    folder = config.digest_folder()
    try:
        folder.mkdir(mode=config.FOLDER_MODE, parents=True, exist_ok=True)
        os.chmod(folder, config.FOLDER_MODE)
    except OSError as error:
        raise StateError(f"{folder} cannot be created: {error}") from error
    return folder


def load(*, strict: bool = False) -> State:
    """The saved state; a missing file is an empty state, an unreadable one raises StateError."""
    try:
        data = json.loads(beneath.read_bytes(config.digest_folder(), config.STATE_FILE))
        if strict:
            _validate(data)
        found = {name: SourceState(str(entry.get("cursor", "")), str(entry.get("at", "")))
                 for name, entry in data["sources"].items()}
        return State(found, str(data.get("done_date", "")), str(data.get("last_run_at", "")))
    except FileNotFoundError:
        return State()
    except (OSError, ValueError, KeyError, AttributeError, TypeError) as error:
        raise StateError(f"{config.digest_folder() / config.STATE_FILE} cannot be read: {error}") from error



def _validate(data: dict) -> None:
    if (not isinstance(data, dict) or data.get("version", STATE_VERSION) != STATE_VERSION
            or not isinstance(data.get("sources"), dict)):
        raise ValueError("invalid digest state version or sources")
    if any(not isinstance(data.get(key, ""), str) for key in ("done_date", "last_run_at")):
        raise ValueError("invalid digest dates")
    if data.get("done_date"):
        date.fromisoformat(data["done_date"])
    _validate_at(data.get("last_run_at", ""))
    for entry in data["sources"].values():
        if not isinstance(entry, dict) or any(not isinstance(entry.get(key, ""), str) for key in ("cursor", "at")):
            raise ValueError("invalid source checkpoint")
        _validate_at(entry.get("at", ""))


def _validate_at(value: str) -> None:
    if value and datetime.fromisoformat(value).utcoffset() is None:
        raise ValueError("digest timestamp must include its time zone")

def save(state: State) -> None:
    """Write the state whole, atomically."""
    document = {"version": STATE_VERSION, "done_date": state.done_date, "last_run_at": state.last_run_at,
                "sources": {name: {"cursor": entry.cursor, "at": entry.at} for name, entry in state.sources.items()}}
    try:
        beneath.write_bytes(ensure_folder(), config.STATE_FILE, json.dumps(document, indent=1).encode(),
                            config.FILE_MODE)
    except OSError as error:
        raise StateError(f"the digest state cannot be written: {error}") from error


def committed(state: State, reports: list[SourceReport], cursors: dict[str, str], until: datetime, today: str,
              sinces: dict[str, str] | None = None) -> State:
    """The state after a finished run: each source that ran well moves to its new cursor and to `until`; a source that
    failed keeps what it had, so its next run looks again from there, and one that had nothing saved keeps the window
    start it used (`sinces`), so the next run retries that window. `today` is the local date of this run."""
    moment = until.isoformat(timespec="seconds")
    sources = dict(with_windows(state, {report.id: (sinces or {}).get(report.id, "") for report in reports}).sources)
    for report in reports:
        if report.ok:
            sources[report.id] = SourceState(cursors.get(report.id) or state.of(report.id).cursor, moment)
    return replace(state, sources=sources, done_date=today, last_run_at=moment)


def with_windows(state: State, sinces: dict[str, str]) -> State:
    """`state` where each source with nothing saved remembers the window start it used: a run that gave it no result
    (it failed, was withheld, or the day did not finish) must not let the next run recompute a later look-back."""
    sources = dict(state.sources)
    for source_id, since in sinces.items():
        if since and source_id not in sources:
            sources[source_id] = SourceState("", since)
    return replace(state, sources=sources)
