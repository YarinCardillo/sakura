"""Where a weekly limit is heading: a least-squares line over the last day of utilization samples."""

import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from sakura.usage.archive.calibration import parse_moment
from sakura.usage.model import Sample

WEEK_WINDOW = "seven_day"
HISTORY = timedelta(hours=24)
STALE_AFTER = timedelta(hours=6)
FULL = 1.0
SECONDS_PER_HOUR = 3600
PERCENT = 100
SNAPSHOT_PROVIDER = "claude"
RECENT_SNAPSHOTS = ("SELECT ts, seven_day_pct, seven_day_resets_at FROM quota_snapshots WHERE account = ?"
                    " AND seven_day_pct IS NOT NULL AND seven_day_resets_at IS NOT NULL AND julianday(ts) >= julianday(?)"
                    " ORDER BY julianday(ts)")


@dataclass(frozen=True)
class Projection:
    """Utilization now, expected at the reset, and when the limit would be reached (None if it would not)."""

    current: float
    at_reset: float
    limit_at: datetime | None
    trend: str


def current_series(samples: list[Sample], now: datetime, provider: str | None = None,
                   window: str = WEEK_WINDOW) -> list[Sample]:
    """The samples of the current reset period of one window, oldest first; empty when that period is over.

    Samples are partitioned by (provider, window, resets_at); the period still open with the latest reset wins.
    """
    partitions: dict[tuple, list[Sample]] = {}
    for sample in samples:
        if sample.window == window and provider in (None, sample.provider) and sample.resets_at > now:
            partitions.setdefault((sample.provider, sample.window, sample.resets_at), []).append(sample)
    if not partitions:
        return []
    latest = max(partitions, key=lambda key: key[2])
    return sorted(partitions[latest], key=lambda sample: sample.at)


def project(samples: list[Sample], now: datetime, provider: str | None = None,
            window: str = WEEK_WINDOW) -> Projection | None:
    """Projection of the current period, or None with fewer than two recent samples or a newest one over 6 h old."""
    recent = [sample for sample in current_series(samples, now, provider, window) if sample.at >= now - HISTORY]
    if len(recent) < 2 or now - recent[-1].at > STALE_AFTER:
        return None
    slope = _slope_per_hour(recent)
    if slope is None:
        return None
    return _extrapolate(recent[-1], slope, now)


def project_week(conn: sqlite3.Connection, account: str, now: datetime) -> float | None:
    """The weekly percentage expected at the reset of `account`, from the trend of its quota snapshots.

    Reads only `quota_snapshots` (the same figures the plan limit shows), so use outside Claude Code, which no
    local transcript records, is part of the trend. None with fewer than two snapshots of the current week in
    the last day, or a newest one older than six hours.
    """
    since = (now - HISTORY).astimezone(timezone.utc).isoformat()
    samples = [Sample(parse_moment(ts), SNAPSHOT_PROVIDER, WEEK_WINDOW, pct / PERCENT, parse_moment(resets))
               for ts, pct, resets in conn.execute(RECENT_SNAPSHOTS, (account, since))]
    found = project(samples, now)
    return None if found is None else found.at_reset * PERCENT


def _slope_per_hour(samples: list[Sample]) -> float | None:
    hours = [(sample.at - samples[0].at).total_seconds() / SECONDS_PER_HOUR for sample in samples]
    mean_x, mean_y = sum(hours) / len(hours), sum(sample.utilization for sample in samples) / len(samples)
    spread = sum((x - mean_x) ** 2 for x in hours)
    if spread == 0:
        return None
    return sum((x - mean_x) * (sample.utilization - mean_y) for x, sample in zip(hours, samples)) / spread


def _extrapolate(newest: Sample, slope: float, now: datetime) -> Projection:
    current = newest.utilization
    if slope <= 0:
        return Projection(current, current, None, "flat")
    hours_left = (newest.resets_at - now).total_seconds() / SECONDS_PER_HOUR
    at_reset = current + slope * hours_left
    limit_at = None
    if at_reset > FULL:
        limit_at = now + timedelta(hours=max(FULL - current, 0.0) / slope)
    return Projection(current, at_reset, limit_at, "rising")
