"""The `status` document: every account with live percentages, reset times and a weekly forecast.

Accounts that report the same weekly window are one account reached through two directories (an Orca profile
and a registered one): they are merged, the first target winning. Live readings are written to the usage archive
under the winner's key, so history and forecasts read from one place.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime

from sakura.quota import forecast, snapshots
from sakura.quota.readings import Collector, Reading
from sakura.quota.targets import CLAUDE, Target
from sakura.usage.archive import calibration, quota

SAME_ACCOUNT_PCT = 1.0
HISTORY_POINTS = 200


@dataclass(frozen=True)
class Entry:
    """One distinct account: the target that names it, what it reported, and the other directories that reach it."""

    target: Target
    reading: Reading | None
    aliases: tuple[Target, ...] = ()


def build(targets: Sequence[Target], collector: Collector, conn: sqlite3.Connection, now: datetime,
          refresh: bool = False) -> dict:
    """The status document for `targets`, probing the ones whose archive snapshot is stale (all with `refresh`)."""
    entries = merge([(target, collector.read(target, refresh)) for target in targets])
    for entry in entries:
        if entry.reading is not None and entry.reading.live:
            quota.record(conn, entry.reading.snapshot)
    accounts = [_account(entry, conn, now) for entry in entries]
    return {"generated_at": now.isoformat(), "accounts": [a for a, _ in accounts],
            "history": {a["id"]: points for a, points in accounts}}


def merge(read: Sequence[tuple[Target, Reading | None]]) -> list[Entry]:
    """Fold Claude targets with the same weekly window into the first of them; others stay as they are."""
    kept: list[Entry] = []
    for target, reading in read:
        twin = next((i for i, entry in enumerate(kept) if _same_account(entry, target, reading)), None)
        if twin is None:
            kept.append(Entry(target, reading))
        else:
            kept[twin] = Entry(kept[twin].target, kept[twin].reading, (*kept[twin].aliases, target))
    return kept


def _same_account(entry: Entry, target: Target, reading: Reading | None) -> bool:
    if CLAUDE not in (target.provider, entry.target.provider) or entry.target.provider != target.provider:
        return False
    mine, theirs = _week(reading), _week(entry.reading)
    return mine is not None and theirs is not None and mine[1] == theirs[1] and abs(mine[0] - theirs[0]) <= SAME_ACCOUNT_PCT


def _week(reading: Reading | None) -> tuple[float, str] | None:
    window = reading.seven_day if reading else None
    return (window.pct, window.resets_at) if window and window.pct is not None and window.resets_at else None


def _account(entry: Entry, conn: sqlite3.Connection, now: datetime) -> tuple[dict, list]:
    target, reading = entry.target, entry.reading
    week = reading.seven_day if reading else None
    resets = calibration.parse_moment(week.resets_at) if week and week.resets_at else None
    history = snapshots.window_history(conn, target.key, resets) if resets else []
    found = forecast.forecast(history, resets, now)
    document = {"id": target.id, "label": target.label, "provider": target.provider,
                "config_dir": str(target.config_dir), "registered": target.registered,
                "aliases": [str(alias.config_dir) for alias in entry.aliases],
                "plan": reading.snapshot.subscription_type if reading else None,
                "five_hour": _window(reading.five_hour if reading else None, now),
                "seven_day": _window(week, now), "forecast": _forecast(found),
                "source": reading.source if reading else None, "stale": reading.stale if reading else True,
                "as_of": reading.snapshot.ts if reading else None}
    return document, _points(history)


def _window(window, now: datetime) -> dict:
    """The window as `{pct, resets_at}`; a window whose reset time has passed no longer says anything."""
    if window is None or window.pct is None or window.resets_at is None:
        return {"pct": None, "resets_at": None}
    if calibration.parse_moment(window.resets_at) <= now:
        return {"pct": None, "resets_at": None}
    return {"pct": window.pct, "resets_at": window.resets_at}


def _forecast(found: forecast.Forecast) -> dict:
    return {"seven_day_pct_at_reset": found.seven_day_pct_at_reset, "next_round_pct": found.next_round_pct,
            "exhausts_at": found.exhausts_at.isoformat() if found.exhausts_at else None,
            "confidence": found.confidence}


def _points(history: list[tuple[datetime, float]]) -> list[list]:
    step = max(1, -(-len(history) // HISTORY_POINTS))
    thinned = history[::step]
    if history and thinned[-1] != history[-1]:
        thinned.append(history[-1])
    return [[at.isoformat(), pct] for at, pct in thinned]
