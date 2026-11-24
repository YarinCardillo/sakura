"""Read and snooze marks for the exact waits the person saw."""

import fcntl
import json
import os
from contextlib import contextmanager
from dataclasses import replace
from datetime import datetime, time, timedelta
from pathlib import Path

from sakura import engine_home, text_limits
from sakura.flow import gates, waits

RETENTION = timedelta(days=30)
FILE = Path("cockpit") / "attention.json"
ACTIONS = ("read", "unread", "snooze", "unsnooze")


def apply(found: list[waits.Wait], now: datetime) -> list[waits.Wait]:
    """Attach attention to each wait while keeping snoozed waits actionable."""
    marks = _read(engine_home.state_dir() / FILE)
    return [_attach(wait, marks.get(wait.id, {}), now) for wait in found]


def _attach(wait: waits.Wait, mark: dict, now: datetime) -> waits.Wait:
    try:
        until = datetime.fromisoformat(mark.get("snoozed_until") or "")
        if until.utcoffset() is None:
            until = None
    except (TypeError, ValueError):
        until = None
    return replace(wait, is_read=bool(mark.get("read_at")), snoozed_until=text_limits.utc_text(until) if until else None,
                   is_snoozed=bool(until and until > now))


def deadline(value: str, now: datetime) -> datetime:
    """Resolve an offset-aware ISO timestamp or a snooze preset to a future deadline."""
    if value == "1h":
        return now + timedelta(hours=1)
    if value in ("tomorrow", "nextweek"):
        local = now.astimezone()
        days = 1 if value == "tomorrow" else 7 - local.weekday()
        return datetime.combine(local.date() + timedelta(days=days), time.min).astimezone()
    try:
        until = datetime.fromisoformat(value)
        if until.utcoffset() is not None and until > now:
            return until
    except ValueError:
        pass
    raise ValueError("use a future ISO timestamp with an offset, 1h, tomorrow, or nextweek")


def update(wait_id: str, key: str, now: datetime, until: datetime | None) -> None:
    """Update the marks of the requested wait."""
    target = engine_home.state_dir() / FILE
    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with _locked(target):
        marks = _read(target)
        mark = marks.setdefault(wait_id, {"read_at": None, "snoozed_until": None})
        mark["updated_at"] = text_limits.utc_text(now)
        if key in ("snooze", "unsnooze"):
            mark["snoozed_until"] = text_limits.utc_text(until) if until is not None else None
        else:
            mark["read_at"] = text_limits.utc_text(now) if key == "read" else None
        gates.write_atomically(target, json.dumps(marks, sort_keys=True).encode())


def prune(now: datetime) -> None:
    """Remove attention marks past retention, keeping active snoozes."""
    target = engine_home.state_dir() / FILE
    if not target.is_file():
        return
    with _locked(target):
        marks = _read(target)
        kept = {wait_id: mark for wait_id, mark in marks.items() if _keep(mark, now)}
        if len(kept) < len(marks):
            gates.write_atomically(target, json.dumps(kept, sort_keys=True).encode())


def _keep(mark: dict, now: datetime) -> bool:
    for key, cutoff in (("updated_at", now - RETENTION), ("read_at", now - RETENTION), ("snoozed_until", now)):
        try:
            if datetime.fromisoformat(mark.get(key) or "") > cutoff:
                return True
        except (TypeError, ValueError):
            continue
    return False


@contextmanager
def _locked(target: Path):
    descriptor = os.open(target.with_suffix(".lock"), os.O_CREAT | os.O_RDWR, 0o600)
    with os.fdopen(descriptor, "rb") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        yield


def _read(target: Path) -> dict[str, dict]:
    try:
        raw = json.loads(target.read_text())
    except FileNotFoundError:
        return {}
    if not isinstance(raw, dict) or any(not isinstance(mark, dict) for mark in raw.values()):
        raise ValueError("Inbox attention file is invalid")
    return raw
