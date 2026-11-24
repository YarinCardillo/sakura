"""Pre-flight estimate: a flow's predicted weighted tokens as a share of the account's week and 5-hour window.

The conversion reuses `calibration.estimate` (the plan's own calibration, or the previous plan's scaled by the
plan weights). The band combines the prediction's low to high with the spread of the calibration ratios, and is
widened when the ratio is scaled from another plan. A quota the snapshots cannot vouch for is unknown, never
zero: the flow then does not fit and has to ask.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import NamedTuple

from sakura.accounts.providers import CODEX_ACCOUNT_PREFIX
from sakura.budget.predict import NO_HISTORY, POST_BUILD_ROLES, FlowPrediction
from sakura.budget.predict import Band as TokenBand
from sakura.budget.settings import BudgetSettings
from sakura.usage.archive import calibration
from sakura.usage.archive import settings as archive_settings
from sakura.usage.archive.settings import Settings as UsageSettings

OK, WARN, ASK = "ok", "warn", "ask"
NO_CALIBRATION = "no calibration"
SCALED_WIDE_LOW, SCALED_WIDE_HIGH = 0.5, 2.0
SPREAD_LOW_INDEX, SPREAD_HIGH_INDEX = 0, 1
FULL_PERCENT = 100


class Band(NamedTuple):
    """A share of a limit window in percent: the low and high edges around the middle estimate."""

    low: float
    mid: float
    high: float


@dataclass(frozen=True)
class Preflight:
    """What a flow is expected to cost, whether it fits above the reserve, and why not.

    A band is None when no calibration or no history lets the tokens be converted to percent. `quota_pct` is
    the latest weekly percentage the snapshots vouch for, None when the quota is unknown. `off` marks a flow
    that was not estimated because no usage archive is configured (it fits, and carries no bands).
    `post_build_share` is the part of the high estimate that the roles after the build phase account for.
    `reserve_pct` is the weekly reserve `fits` was judged against (the account's own or the global one).
    """

    week_pct: Band | None
    five_hour_pct: Band | None
    basis: str
    fits: bool
    reason: str
    quota_pct: float | None = None
    off: bool = False
    post_build_share: float = 0.0
    reserve_pct: float | None = None


def estimate(prediction: FlowPrediction, account: str, now: datetime, conn: sqlite3.Connection,
             settings: BudgetSettings, usage: UsageSettings) -> Preflight:
    """The pre-flight estimate of `prediction` for `account` at `now`.

    `usage` supplies the plan periods and plan weights. The weekly band decides `fits`: it is false when the
    latest weekly percentage plus the high edge passes `100 - reserve_week_pct`, and also when the quota is
    unknown (no snapshot, or one older than `snapshot_max_age_minutes`) or no percentage can be computed.
    """
    quota, unknown = _quota(conn, account, now, settings.snapshot_max_age_minutes)
    tokens = {name: band for name, band in prediction.by_class.items() if name != calibration.OTHER}
    week, week_basis = _converted(tokens, conn, usage, account, now, calibration.SEVEN_DAY)
    five_hour, _ = _converted(tokens, conn, usage, account, now, calibration.FIVE_HOUR)
    basis = _basis(week, week_basis, prediction.basis)
    fits, reason = _fits(week, quota, unknown, settings.reserve_week_pct, basis)
    return Preflight(week, five_hour, basis, fits, reason, quota, post_build_share=_post_build_share(prediction),
                     reserve_pct=settings.reserve_week_pct)


def unavailable(reason: str) -> Preflight:
    """A pre-flight that could not be computed at all (unreadable archive): nothing fits, the flow asks."""
    return Preflight(None, None, reason, False, reason)


def off(reason: str) -> Preflight:
    """The budget is not configured (no usage archive): nothing was estimated, nothing is asked."""
    return Preflight(None, None, "off", True, reason, off=True)


def level(result: Preflight, settings: BudgetSettings) -> str:
    """`ask` when the flow does not fit or its high edge passes `ask_flow_week_pct`, `warn` above
    `warn_flow_week_pct`, else `ok`."""
    high = result.week_pct.high if result.week_pct else 0.0
    if not result.fits or high > settings.ask_flow_week_pct:
        return ASK
    return WARN if high > settings.warn_flow_week_pct else OK


def summary(result: Preflight, settings: BudgetSettings) -> dict:
    """The pre-flight as plain data (bands as [low, mid, high]) for state and JSON output."""
    return {"week_pct": _listed(result.week_pct), "five_hour_pct": _listed(result.five_hour_pct),
            "basis": result.basis, "fits": result.fits, "reason": result.reason, "quota_pct": result.quota_pct,
            "level": level(result, settings), "text": text(result), "off": result.off,
            "post_build_pct": result.week_pct.high * result.post_build_share if result.week_pct else 0.0}


def text(result: Preflight) -> str:
    """The one line shown to the owner, with the band and the basis."""
    week, five = result.week_pct, result.five_hour_pct
    if result.off:
        return f"budget off: {result.reason}"
    if week is None:
        return f"no percent estimate; {result.basis}"
    window = f"{five.mid:.0f}% of the 5-hour window" if five else "5-hour window not calibrated"
    return f"about {week.mid:.0f}% of your week ({week.low:.0f}-{week.high:.0f}), {window}; {result.basis}"


def _post_build_share(prediction: FlowPrediction) -> float:
    """The share of the flow's high token estimate that belongs to the evaluator and the reporter."""
    total = sum(part.high for part in prediction.parts.values())
    after = sum(prediction.parts[role].high for role in POST_BUILD_ROLES if role in prediction.parts)
    return after / total if total else 0.0


def _listed(band: Band | None) -> list[float] | None:
    return None if band is None else list(band)


def _quota(conn: sqlite3.Connection, account: str, now: datetime, max_age_minutes: int) -> tuple[float | None, str]:
    """The latest weekly percentage and, when it cannot be trusted, why the quota is unknown."""
    row = conn.execute("SELECT ts, seven_day_pct FROM quota_snapshots WHERE account = ? AND seven_day_pct IS NOT NULL"
                       " ORDER BY ts DESC LIMIT 1", (account,)).fetchone()
    if row is None:
        return None, f"quota unknown: no snapshot of {account}"
    age = now - calibration.parse_moment(row[0])
    if age > timedelta(minutes=max_age_minutes):
        minutes = int(age.total_seconds() // 60)
        return None, f"quota unknown: the latest snapshot of {account} is from {row[0]} ({minutes} min old, limit {max_age_minutes})"
    return float(row[1]), ""


def _converted(tokens: Mapping[str, TokenBand], conn: sqlite3.Connection, usage: UsageSettings, account: str,
               now: datetime, window: str) -> tuple[Band | None, str]:
    """The band in percent of `window` and the calibration basis; (None, basis) when it cannot be computed."""
    if not tokens:
        return None, NO_HISTORY
    return percent_band(tokens, calibration.compute(conn, usage.plans, window), usage, account, now)


def percent_band(tokens: Mapping[str, TokenBand], ratios: Sequence[calibration.Ratio], usage: UsageSettings,
                 account: str, now: datetime) -> tuple[Band | None, str]:
    """The band in percent of the window `ratios` were computed for, and the calibration basis.

    `tokens` is not empty. (None, basis) when no ratio applies to the account's plan at `now`.
    """
    mid = _percent(ratios, usage, account, now, {name: band.weighted for name, band in tokens.items()})
    if mid is None:
        return None, no_calibration(usage, account, now)
    low = _percent(_stretched(ratios, SPREAD_HIGH_INDEX), usage, account, now, {n: b.low for n, b in tokens.items()})
    high = _percent(_stretched(ratios, SPREAD_LOW_INDEX), usage, account, now, {n: b.high for n, b in tokens.items()})
    band = Band(min(low.pct, mid.pct), mid.pct, max(high.pct, mid.pct))
    if mid.scaled:
        band = Band(min(band.low, mid.pct * SCALED_WIDE_LOW), mid.pct, max(band.high, mid.pct * SCALED_WIDE_HIGH))
        return band, f"{mid.basis}, band widened"
    return band, mid.basis


def no_calibration(usage: UsageSettings, account: str, now: datetime) -> str:
    """The basis of a figure nothing calibrates: plain for a Claude key, naming the plan for a Codex key.

    Claude output is kept as it always was; a Codex account (key `codex:<id>`) with a plan period at `now`
    reads `no calibration for <plan>`, so a missing weight or missing intervals of that plan are visible.
    """
    period = archive_settings.period_at(usage.plans, account, now)
    named = account.startswith(CODEX_ACCOUNT_PREFIX) and period is not None
    return f"{NO_CALIBRATION} for {period.plan}" if named else NO_CALIBRATION


def _percent(ratios: Sequence[calibration.Ratio], usage: UsageSettings, account: str, now: datetime,
             tokens: Mapping[str, float]) -> calibration.Estimate | None:
    return calibration.estimate(ratios, usage.plans, usage.plan_weights, account, now, tokens)


def _stretched(ratios: Sequence[calibration.Ratio], index: int) -> list[calibration.Ratio]:
    """Ratios with the weighted tokens per 1% moved to an edge of their spread; a zero-width spread is no spread."""
    return [r._replace(weighted_per_pct=r.spread[index]) if all(r.spread) else r for r in ratios]


def _basis(week: Band | None, week_basis: str, prediction_basis: str) -> str:
    if week is None:
        return week_basis
    return f"{week_basis}, {prediction_basis}"


def _fits(week: Band | None, quota: float | None, unknown: str, reserve: float, basis: str) -> tuple[bool, str]:
    if unknown:
        return False, unknown
    if week is None:
        return False, f"cannot convert the cost to percent: {basis}"
    ceiling = FULL_PERCENT - reserve
    if quota + week.high > ceiling:
        return False, f"week at {quota:.0f}% plus up to {week.high:.0f}% passes the {reserve:.0f}% reserve"
    return True, ""
