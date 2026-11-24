"""Limit hits written by older Claude Code versions: no `quotaLimits`, only a "hit your X limit" message.

Only the first text block is read, only for the window word and the wall-clock reset time; the text itself
is never kept.
"""

from __future__ import annotations

import re
from datetime import datetime, time, timedelta, timezone
from zoneinfo import ZoneInfo

LIMIT_PATTERN = re.compile(r"hit your (\w+) limit", re.IGNORECASE)
RESET_PATTERN = re.compile(r"resets (\d{1,2})(?::(\d{2}))?\s*(am|pm) \(([^)]+)\)", re.IGNORECASE)
WINDOW_BY_WORD = {"session": "five_hour", "weekly": "seven_day"}
MAX_CLOCK_HOUR = 12
RESET_FORMAT = "%Y-%m-%dT%H:%M:%SZ"


def legacy_window_and_reset(message: object, hit_at: datetime) -> tuple[str, str | None] | None:
    """(window, reset time as ISO UTC with Z or None) of a legacy limit message, None without the limit phrase."""
    text = _first_text(message)
    limit = LIMIT_PATTERN.search(text) if text else None
    if not limit:
        return None
    word = limit.group(1)
    return WINDOW_BY_WORD.get(word.lower(), word), _reset(text, hit_at)


def _first_text(message: object) -> str | None:
    content = message.get("content") if isinstance(message, dict) else None
    for block in content if isinstance(content, list) else []:
        if isinstance(block, dict) and block.get("type") == "text":
            return block["text"] if isinstance(block.get("text"), str) else None
    return None


def _reset(text: str, hit_at: datetime) -> str | None:
    """The first occurrence of the named wall-clock time in the named zone after the hit."""
    match = RESET_PATTERN.search(text)
    if not match:
        return None
    hour, minute, meridiem, zone_name = match.groups()
    clock = _clock(int(hour), int(minute or 0), meridiem.lower())
    zone = _zone(zone_name)
    if clock is None or zone is None:
        return None
    local_hit = hit_at.astimezone(zone)
    reset = datetime.combine(local_hit.date(), clock, zone)
    if reset <= local_hit:
        reset = datetime.combine(local_hit.date() + timedelta(days=1), clock, zone)
    return reset.astimezone(timezone.utc).strftime(RESET_FORMAT)


def _clock(hour: int, minute: int, meridiem: str) -> time | None:
    if not 1 <= hour <= MAX_CLOCK_HOUR or minute > 59:
        return None
    return time(hour % MAX_CLOCK_HOUR + (MAX_CLOCK_HOUR if meridiem == "pm" else 0), minute)


def _zone(name: str) -> ZoneInfo | None:
    try:
        return ZoneInfo(name)
    except (ValueError, OSError, KeyError):
        return None
