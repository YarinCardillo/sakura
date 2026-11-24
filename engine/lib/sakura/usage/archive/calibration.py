"""Quota calibration: weighted tokens per 1% of a limit window, per account, plan and model class.

Percentages have 1% resolution, so an interval runs from the last snapshot where the percentage changed to
the next one where it rose, inside one window (the weekly or the 5-hour one, told apart by its reset time).
Zero-change snapshots carry no information and their tokens flow into the next rise.
"""

from __future__ import annotations

import bisect
import re
import sqlite3
import statistics
from collections import defaultdict
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta, timezone
from typing import NamedTuple

from sakura.usage.archive import settings
from sakura.usage.archive.settings import Plan

# Anthropic list-price ratios relative to input tokens; the cache write weight is the 1-hour rate that
# Claude Code uses. The same for every Claude model.
INPUT_WEIGHT = 1.0
CACHE_WRITE_WEIGHT = 2.0
CACHE_READ_WEIGHT = 0.1
OUTPUT_WEIGHT = 5.0
DOMINANT_SHARE = 0.8
MIXED = "mixed"
OTHER = "other"
UNKNOWN_PLAN = "unknown"
CLASS_PATTERN = re.compile(r"^claude-([a-z]+)")
SEVEN_DAY = "seven_day"
FIVE_HOUR = "five_hour"
# window -> (percentage column, reset-time column) of `quota_snapshots`; the only names ever put in SQL.
WINDOW_COLUMNS = {SEVEN_DAY: ("seven_day_pct", "seven_day_resets_at"),
                  FIVE_HOUR: ("five_hour_pct", "five_hour_resets_at")}
# Codex accounts have Codex snapshots but no Claude turns, so they take no part in the Claude calibration.
CLAUDE_ACCOUNTS = f"SELECT DISTINCT account FROM quota_snapshots WHERE {settings.CLAUDE_ACCOUNT_ONLY} ORDER BY account"
SPREAD_LOW_INDEX, SPREAD_HIGH_INDEX, SPREAD_QUANTILES = 0, 3, 5
# A rise across a longer silence than this (Mac asleep, probe failing) cannot be tied to the turns of that span.
MAX_ATTRIBUTED_GAP = timedelta(hours=6)


class Ratio(NamedTuple):
    """Weighted tokens per 1% of `window`, from `intervals` intervals that together covered `pct` percent.

    `spread` is the 20th and 80th percentile of the per-interval weighted tokens per 1%, the edges of a band.
    The defaults suit hand-built ratios in tests; `compute` always sets both.
    """

    account: str
    plan: str
    model_class: str
    weighted_per_pct: float
    intervals: int
    pct: float
    window: str = SEVEN_DAY
    spread: tuple[float, float] = (0.0, 0.0)


class OutsideShare(NamedTuple):
    """One UTC day of one account: the attributed weekly rise, the part local turns explain, the rest as a share.

    A rise belongs to the day its snapshot interval ends. `unattributed` is the rise that came after a gap
    longer than `MAX_ATTRIBUTED_GAP` and is left out of `rise` and `share`. `local_pct` and `share` are None
    when local turns exist but the plan has no calibration for their classes, or nothing was attributed.
    """

    account: str
    day: date
    rise: float
    local_pct: float | None
    share: float | None
    unattributed: float = 0.0


@dataclass(frozen=True)
class Estimate:
    """A weekly percentage and where it comes from; `scaled` marks one carried over from an earlier plan."""

    pct: float
    basis: str
    scaled: bool = False


@dataclass(frozen=True)
class Analysis:
    ratios: list[Ratio]
    unexplained: Mapping[tuple[str, str], int] = field(default_factory=dict)


class _Snapshot(NamedTuple):
    at: datetime
    plan_hint: str | None
    pct: float
    resets_at: str


class _Interval(NamedTuple):
    start: datetime
    rise: float
    by_class: dict[str, float]


class _Span(NamedTuple):
    end: datetime
    rise: float
    by_class: dict[str, float]
    is_attributed: bool


@dataclass
class _Bucket:
    """The intervals of one (account, plan, class): their summed tokens and rise and each one's tokens per 1%."""

    tokens: float = 0.0
    rise: float = 0.0
    per_pct: list[float] = field(default_factory=list)

    def add(self, tokens: float, rise: float) -> None:
        self.tokens += tokens
        self.rise += rise
        self.per_pct.append(tokens / rise)


def model_class(model: str | None) -> str:
    """`claude-opus-5-5` gives `opus`: the word after `claude-`; anything else is `other`."""
    match = CLASS_PATTERN.match(model or "")
    return match.group(1) if match else OTHER


def weighted_tokens(input: int, cache_write: int, cache_read: int, output: int) -> float:
    """Token counts folded into one number by their relative price."""
    return (input * INPUT_WEIGHT + cache_write * CACHE_WRITE_WEIGHT
            + cache_read * CACHE_READ_WEIGHT + output * OUTPUT_WEIGHT)


def compute(conn: sqlite3.Connection, plans: Sequence[Plan], window: str = SEVEN_DAY) -> list[Ratio]:
    """The calibration ratios of every account in the archive, for the weekly or the 5-hour window."""
    return analyze(conn, plans, window).ratios


def analyze(conn: sqlite3.Connection, plans: Sequence[Plan], window: str = SEVEN_DAY) -> Analysis:
    """Ratios plus, per (account, plan), the intervals skipped because the rise had no local tokens."""
    if window not in WINDOW_COLUMNS:
        raise ValueError(f"unknown quota window {window!r}: expected {SEVEN_DAY!r} or {FIVE_HOUR!r}")
    buckets: dict[tuple[str, str, str], _Bucket] = defaultdict(_Bucket)
    unexplained: dict[tuple[str, str], int] = defaultdict(int)
    accounts = [row[0] for row in conn.execute(CLAUDE_ACCOUNTS)]
    for account in accounts:
        turns = TurnIndex(conn, account)
        for interval in _intervals(conn, account, turns, window):
            plan = settings.plan_at(tuple(plans), account, interval.start) or _hint(conn, account)
            if not interval.by_class:
                unexplained[(account, plan)] += 1
                continue
            buckets[(account, plan, _class_of(interval.by_class))].add(sum(interval.by_class.values()), interval.rise)
    ratios = [Ratio(*key, bucket.tokens / bucket.rise, len(bucket.per_pct), round(bucket.rise, 1), window,
                    _spread(bucket.per_pct)) for key, bucket in sorted(buckets.items())]
    return Analysis(ratios, dict(unexplained))


def outside_share(conn: sqlite3.Connection, plans: Sequence[Plan],
                  days: date | tuple[date, date]) -> list[OutsideShare]:
    """Per account and UTC day (one day or an inclusive first, last range) with a weekly rise: the share of
    the rise that local turns do not explain, by the account's calibrated ratios.

    Each rise is a snapshot interval (from the last snapshot where the percentage changed, as in calibration)
    counted with the local turns of that same interval and credited to the day it ends, so an interval across
    midnight is not split. An interval that follows a gap over `MAX_ATTRIBUTED_GAP` is `unattributed`. An
    attributed rise with no local turns is share 1.0; a day without a rise has no row.
    """
    first, last = (days, days) if isinstance(days, date) else days
    ratios = compute(conn, plans)
    accounts = [row[0] for row in conn.execute(CLAUDE_ACCOUNTS)]
    found: list[OutsideShare] = []
    for account in accounts:
        spans = _rise_spans(_snapshots(conn, account, SEVEN_DAY), TurnIndex(conn, account))
        for day in sorted({span.end.date() for span in spans}):
            if first <= day <= last:
                found.append(_day_share(conn, ratios, plans, account, day, [s for s in spans if s.end.date() == day]))
    return found


def estimate(ratios: Sequence[Ratio], plans: Sequence[Plan], weights: Mapping[str, float], account: str,
             at: datetime, tokens_by_class: Mapping[str, float]) -> Estimate | None:
    """The weekly percentage of a weighted token mix, from the plan's own calibration or, after a plan
    switch, the nearest earlier plan's calibration scaled by the plan weights. None when nothing applies."""
    period = settings.period_at(tuple(plans), account, at)
    if period is None:
        return None
    own = _percent(ratios, account, period.plan, tokens_by_class)
    if own is not None:
        return Estimate(own, _basis(f"{period.plan} calibration", period))
    for earlier in _earlier_periods(plans, account, period):
        old, new = weights.get(earlier.plan), weights.get(period.plan)
        pct = _percent(ratios, account, earlier.plan, tokens_by_class) if old and new else None
        if pct is not None:
            return Estimate(pct * old / new, _basis(f"scaled from {earlier.plan}", earlier, period), scaled=True)
    return None


class TurnIndex:
    """One account's Claude turns in time order, with weighted tokens summable over a time interval."""

    def __init__(self, conn: sqlite3.Connection, account: str) -> None:
        rows = conn.execute(
            "SELECT ts, model, input_tokens, cache_write_tokens, cache_read_tokens, output_tokens FROM turns"
            " WHERE provider = 'claude' AND account = ?", (account,)).fetchall()
        parsed = sorted((parse_moment(row[0]), model_class(row[1]), weighted_tokens(*row[2:])) for row in rows)
        self._times = [moment for moment, _, _ in parsed]
        self._turns = parsed

    def between(self, start: datetime, end: datetime) -> dict[str, float]:
        """Weighted tokens per class for turns with `start < ts <= end`."""
        low = bisect.bisect_right(self._times, start)
        high = bisect.bisect_right(self._times, end)
        totals: dict[str, float] = defaultdict(float)
        for _, name, weight in self._turns[low:high]:
            totals[name] += weight
        return {name: weight for name, weight in totals.items() if weight > 0}


def _intervals(conn: sqlite3.Connection, account: str, turns: TurnIndex, window: str) -> list[_Interval]:
    found: list[_Interval] = []
    anchor: _Snapshot | None = None
    for snap in _snapshots(conn, account, window):
        if anchor is None or snap.resets_at != anchor.resets_at or snap.pct < anchor.pct:
            anchor = snap
        elif snap.pct > anchor.pct:
            found.append(_Interval(anchor.at, snap.pct - anchor.pct, turns.between(anchor.at, snap.at)))
            anchor = snap
    return found


def _snapshots(conn: sqlite3.Connection, account: str, window: str) -> list[_Snapshot]:
    pct, resets = WINDOW_COLUMNS[window]
    rows = conn.execute(f"SELECT ts, subscription_type, {pct}, {resets} FROM quota_snapshots"
                        f" WHERE account = ? AND {pct} IS NOT NULL AND {resets} IS NOT NULL", (account,)).fetchall()
    return sorted((_Snapshot(parse_moment(ts), plan, pct, resets) for ts, plan, pct, resets in rows),
                  key=lambda snap: snap.at)


def _spread(per_pct: Sequence[float]) -> tuple[float, float]:
    """The 20th and 80th percentile (inclusive method, so within the data); one interval gives (x, x)."""
    if len(per_pct) == 1:
        return per_pct[0], per_pct[0]
    cuts = statistics.quantiles(per_pct, n=SPREAD_QUANTILES, method="inclusive")
    return cuts[SPREAD_LOW_INDEX], cuts[SPREAD_HIGH_INDEX]


def _rise_spans(snaps: Sequence[_Snapshot], turns: TurnIndex) -> list[_Span]:
    """The rises of a window: snapshots from the last change to the one that rose, with their local turns.

    A gap over `MAX_ATTRIBUTED_GAP` between two snapshots restarts the span there, so only the time after
    the gap is counted for a later rise; a rise across the gap itself is not attributed.
    """
    spans: list[_Span] = []
    anchor: _Snapshot | None = None
    previous: _Snapshot | None = None
    for snap in snaps:
        is_gap = previous is not None and snap.at - previous.at > MAX_ATTRIBUTED_GAP
        if anchor is None or snap.resets_at != anchor.resets_at or snap.pct < anchor.pct:
            anchor = snap
        elif snap.pct > anchor.pct:
            by_class = {} if is_gap else turns.between(anchor.at, snap.at)
            spans.append(_Span(snap.at, snap.pct - anchor.pct, by_class, not is_gap))
            anchor = snap
        elif is_gap:
            anchor = snap
        previous = snap
    return spans


def _day_share(conn: sqlite3.Connection, ratios: Sequence[Ratio], plans: Sequence[Plan], account: str, day: date,
               spans: Sequence[_Span]) -> OutsideShare:
    start = datetime.combine(day, time(), timezone.utc)
    kept = [span for span in spans if span.is_attributed]
    rise = sum(span.rise for span in kept)
    by_class: dict[str, float] = defaultdict(float)
    for span in kept:
        for name, weight in span.by_class.items():
            by_class[name] += weight
    plan = settings.plan_at(tuple(plans), account, start) or _hint(conn, account)
    local = _percent(ratios, account, plan, by_class) if by_class else 0.0
    share = None if local is None or not rise else max(0.0, rise - local) / rise
    return OutsideShare(account, day, rise, local, share, sum(span.rise for span in spans) - rise)


def _hint(conn: sqlite3.Connection, account: str) -> str:
    """The plan family the probe reported when no plan period is configured."""
    row = conn.execute("SELECT subscription_type FROM quota_snapshots WHERE account = ? AND"
                       " subscription_type IS NOT NULL ORDER BY ts DESC LIMIT 1", (account,)).fetchone()
    return row[0] if row else UNKNOWN_PLAN


def _class_of(by_class: Mapping[str, float]) -> str:
    """The class holding at least the dominant share of the weighted tokens, else `mixed`."""
    name, top = max(by_class.items(), key=lambda item: item[1])
    return name if top / sum(by_class.values()) >= DOMINANT_SHARE else MIXED


def _percent(ratios: Sequence[Ratio], account: str, plan: str, tokens_by_class: Mapping[str, float]) -> float | None:
    """The summed percentage of the mix under one plan's ratios, None when any class has no ratio."""
    known = {r.model_class: r.weighted_per_pct for r in ratios if (r.account, r.plan) == (account, plan)}
    total = 0.0
    for name, tokens in tokens_by_class.items():
        ratio = known.get(name) or known.get(MIXED)
        if ratio is None:
            return None
        total += tokens / ratio
    return total if known else None


def _earlier_periods(plans: Sequence[Plan], account: str, period: Plan) -> list[Plan]:
    earlier = [p for p in plans if p.account == account and p.since < period.since and p.plan != period.plan]
    return sorted(earlier, key=lambda p: p.since, reverse=True)


def _basis(text: str, *periods: Plan) -> str:
    return text + (", estimated plan period" if any(p.estimated for p in periods) else "")


def parse_moment(text: str) -> datetime:
    """An ISO timestamp (a trailing Z allowed) as an aware UTC datetime."""
    moment = datetime.fromisoformat(text.replace("Z", "+00:00"))
    return moment if moment.tzinfo else moment.replace(tzinfo=timezone.utc)
