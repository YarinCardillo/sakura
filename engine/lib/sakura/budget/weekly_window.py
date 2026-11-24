"""Weekly quota windows: the account's own weekly reset times from its snapshots, and how a window is chosen."""

from __future__ import annotations

import re
import sqlite3
from datetime import date, datetime, timedelta, tzinfo
from typing import NamedTuple

from sakura.usage.archive import calibration

WEEK = timedelta(days=7)
ISO_WEEK = re.compile(r"(\d{4})-W(\d{2})")
MONDAY = 1
MERGE_RESETS_WITHIN = timedelta(hours=1)


class Window(NamedTuple):
    """A weekly quota window: turns with `start < ts <= end` belong to it."""

    start: datetime
    end: datetime


def windows(conn: sqlite3.Connection, account: str) -> list[Window]:
    """The weekly windows the account's snapshots name, oldest first.

    Reset times within an hour of each other are one window, the latest of them: a probe may report the same
    reset a few seconds apart.
    """
    rows = conn.execute("SELECT DISTINCT seven_day_resets_at FROM quota_snapshots"
                        " WHERE account = ? AND seven_day_resets_at IS NOT NULL", (account,))
    ends: list[datetime] = []
    for moment in sorted({calibration.parse_moment(row[0]) for row in rows}):
        if ends and moment - ends[-1] <= MERGE_RESETS_WITHIN:
            ends[-1] = moment
        else:
            ends.append(moment)
    return [Window(end - WEEK, end) for end in ends]


def pick_window(conn: sqlite3.Connection, account: str, week: str | None, now: datetime,
                tz: tzinfo | None = None) -> Window | None:
    """The last window that ended by `now`, or with `week` (`YYYY-Www`) the one whose start falls in that ISO week
    in `tz` (local time by default), the week its page is named after. The latter may still be running.
    None when there is none."""
    found = windows(conn, account)
    if week is None:
        return max((window for window in found if window.end <= now), default=None, key=lambda window: window.end)
    wanted = parse_week(week)
    return next((window for window in found if week_label(window, tz) == wanted), None)


def parse_week(text: str) -> tuple[int, int]:
    """(ISO year, ISO week) of `YYYY-Www`; ValueError for anything else."""
    match = ISO_WEEK.fullmatch(text)
    try:
        if match:
            date.fromisocalendar(int(match[1]), int(match[2]), MONDAY)
            return int(match[1]), int(match[2])
    except ValueError:
        pass
    raise ValueError(f"week must be YYYY-Www with an ISO week of that year (such as 2026-W43); got {text!r}")


def week_label(window: Window, tz: tzinfo | None = None) -> tuple[int, int]:
    """(ISO year, ISO week) of the window's start."""
    year, number, _ = window.start.astimezone(tz).isocalendar()
    return year, number
