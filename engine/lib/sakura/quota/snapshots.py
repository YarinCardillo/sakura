"""Reads of the usage archive's `quota_snapshots` that the quota service needs; writing is `quota.record`."""

from __future__ import annotations

import sqlite3
from datetime import datetime, timedelta

from sakura.usage.archive import calibration
from sakura.usage.archive.quota import Snapshot

WEEK = timedelta(days=7)
SAME_RESET_WITHIN = timedelta(hours=1)
COLUMNS = ("account, ts, subscription_type, status, five_hour_pct, five_hour_resets_at, seven_day_pct,"
           " seven_day_resets_at, overage_in_use")
LATEST = f"SELECT {COLUMNS} FROM quota_snapshots WHERE account = ? ORDER BY julianday(ts) DESC LIMIT 1"
WINDOW = ("SELECT ts, seven_day_pct, seven_day_resets_at FROM quota_snapshots WHERE account = ?"
          " AND seven_day_pct IS NOT NULL AND seven_day_resets_at IS NOT NULL AND julianday(ts) >= julianday(?)"
          " ORDER BY julianday(ts)")


def latest(conn: sqlite3.Connection, key: str) -> Snapshot | None:
    """The newest snapshot of the account `key`, None when it has none."""
    row = conn.execute(LATEST, (key,)).fetchone()
    return Snapshot(*row) if row else None


def window_history(conn: sqlite3.Connection, key: str, resets_at: datetime) -> list[tuple[datetime, float]]:
    """(time, weekly percentage) of the snapshots in the weekly window that ends at `resets_at`, oldest first.

    A reset time within an hour of `resets_at` is the same window: a probe may report it a few seconds apart.
    """
    start = (resets_at - WEEK).isoformat()
    points = [(calibration.parse_moment(ts), pct) for ts, pct, resets in conn.execute(WINDOW, (key, start))
              if abs(calibration.parse_moment(resets) - resets_at) <= SAME_RESET_WITHIN]
    return list(dict(points).items())
