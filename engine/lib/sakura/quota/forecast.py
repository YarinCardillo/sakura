"""Weekly forecast of one account from the snapshots of its current weekly window.

The trend is the engine's own least-squares projection (`usage.projection`); this module adds what a router
needs on top of it: the cost of the next round of work and a confidence in the figure.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta

from sakura.usage import projection
from sakura.usage.model import Sample

PERCENT = 100
ROUND_HOURS = 5
ROUND_FLOOR_PCT = 1.0
HIGH_SAMPLES = 6
HIGH_SPAN = timedelta(hours=6)
MEDIUM_SAMPLES = 2
PROVIDER = "quota"

HIGH, MEDIUM, LOW, NONE = "high", "medium", "low", "none"

Point = tuple[datetime, float]


@dataclass(frozen=True)
class Forecast:
    """Where the weekly percentage is heading.

    `next_round_pct` is the weekly percentage after one more round of work, a round being the weekly rise of the
    last five hours (at least one point, the resolution of the figure). `exhausts_at` is None when the trend does
    not reach 100% before the reset. `confidence` is `none` without a reading, `low` without a usable trend.
    """

    seven_day_pct_at_reset: float | None
    next_round_pct: float | None
    exhausts_at: datetime | None
    confidence: str


NO_FORECAST = Forecast(None, None, None, NONE)


def forecast(history: Sequence[Point], resets_at: datetime | None, now: datetime) -> Forecast:
    """The forecast from `history` (weekly percentages of the current window, oldest first, the live reading last)."""
    if not history or resets_at is None:
        return NO_FORECAST
    current = history[-1][1]
    samples = [Sample(at, PROVIDER, projection.WEEK_WINDOW, pct / PERCENT, resets_at) for at, pct in history]
    found = projection.project(samples, now)
    at_reset = current if found is None else found.at_reset * PERCENT
    exhausts = None if found is None else found.limit_at
    return Forecast(round(at_reset, 1), round(current + _round_cost(history, now), 1), exhausts,
                    _confidence(history, found, now))


def _round_cost(history: Sequence[Point], now: datetime) -> float:
    since = now - timedelta(hours=ROUND_HOURS)
    window = [pct for at, pct in history if at >= since]
    rise = history[-1][1] - min(window) if window else 0.0
    return max(rise, ROUND_FLOOR_PCT)


def _confidence(history: Sequence[Point], found: projection.Projection | None, now: datetime) -> str:
    if found is None:
        return LOW
    recent = [at for at, _ in history if at >= now - projection.HISTORY]
    if len(recent) >= HIGH_SAMPLES and recent[-1] - recent[0] >= HIGH_SPAN:
        return HIGH
    return MEDIUM if len(recent) >= MEDIUM_SAMPLES else LOW
