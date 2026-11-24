"""Day and night gate mode: which events page the owner now, and which wait as deferred events."""

from __future__ import annotations

import json
import os
import tempfile
from datetime import datetime, time, timedelta
from pathlib import Path

from sakura.flow.config import DAY_WINDOW, FlowConfig

PAGED_EVENTS = frozenset({"escalation", "ready_for_review", "failed", "gate_red", "awaiting_approval",
                          "budget_warning"})
DAY, NIGHT, AUTO = "day", "night", "auto"
STATE_DIR = Path(".local/state/sakura/flow")
MODE_FILE_NAME = "mode"
DEFERRED_DIR_NAME = "deferred"
DEFERRED_PATTERN = "*.json"


def default_mode_file() -> Path:
    """The machine-wide mode override file, under the current user's home."""
    return Path.home() / STATE_DIR / MODE_FILE_NAME


def default_deferred_dir() -> Path:
    """The directory of events that would have paged during the night."""
    return Path.home() / STATE_DIR / DEFERRED_DIR_NAME


def effective_mode(config: FlowConfig, now: datetime, override_file: Path) -> tuple[str, str]:
    """Return (mode, source): the override file wins, then a pinned `gates.mode`, then the day window."""
    override = _read_override(override_file)
    if override:
        return override, "override"
    settings = config.values["gates"]
    if settings["mode"] != AUTO:
        return settings["mode"], "config"
    return (DAY if _in_day_window(settings["day"], now.time()) else NIGHT), AUTO


def set_override(override_file: Path, mode: str) -> None:
    """Pin the mode machine-wide, or clear the pin for `auto`."""
    if mode == AUTO:
        override_file.unlink(missing_ok=True)
        return
    write_atomically(override_file, f"{mode}\n".encode())


def deferred_since_last_day(deferred_dir: Path, config: FlowConfig, now: datetime) -> list[dict]:
    """List the deferred events from after the most recent start of a day period, oldest first."""
    since = _last_day_start(config.values["gates"]["day"], now)
    entries = (_read_entry(path) for path in sorted(deferred_dir.glob(DEFERRED_PATTERN)))
    return [entry for entry in entries if entry and _is_after(entry["at"], since)]


def write_atomically(target: Path, payload: bytes) -> None:
    """Create parent directories, then write `payload` so a reader never sees a partial file."""
    target.parent.mkdir(parents=True, exist_ok=True)
    handle, temp_name = tempfile.mkstemp(dir=target.parent, prefix=f".{target.name}.", suffix=".tmp")
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_name, target)
    except BaseException:
        Path(temp_name).unlink(missing_ok=True)
        raise


def _read_override(override_file: Path) -> str:
    try:
        text = override_file.read_text().strip()
    except OSError:
        return ""
    return text if text in (DAY, NIGHT) else ""


def _window(window: str) -> tuple[time, time]:
    match = DAY_WINDOW.fullmatch(window)
    return time.fromisoformat(match["start"]), time.fromisoformat(match["end"])


def _in_day_window(window: str, moment: time) -> bool:
    start, end = _window(window)
    return start <= moment < end if start < end else moment >= start or moment < end


def _last_day_start(window: str, now: datetime) -> datetime:
    start, _ = _window(window)
    local = now.astimezone().replace(tzinfo=None)
    candidate = datetime.combine(local.date(), start)
    return (candidate if candidate <= local else candidate - timedelta(days=1)).astimezone()


def _read_entry(path: Path) -> dict | None:
    try:
        entry = json.loads(path.read_text())
    except (OSError, ValueError):
        return None
    return entry if isinstance(entry, dict) and isinstance(entry.get("at"), str) else None


def _is_after(at: str, since: datetime) -> bool:
    try:
        return datetime.fromisoformat(at).astimezone() > since
    except ValueError:
        return False
