"""Weekly consumption per provider: tokens of the current reset period, utilization and its projection."""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

from sakura.flow.store import ReadState
from sakura.lanes.status import LaneRow
from sakura.usage import projection, sources
from sakura.usage.model import Sample, Usage
from sakura.usage.projection import Projection
from sakura.usage.sources import Source

PROVIDERS = ("claude", "codex")
WEEK = timedelta(days=7)
NO_USAGE = Usage(0, 0, 0, 0, None)


@dataclass(frozen=True)
class AgentUsage:
    """What one agent used in the period; the share is of the tokens observed on this machine, not of the limit."""

    item: tuple[str, ...]
    agent: str
    usage: Usage
    share_of_local_tokens: float


@dataclass(frozen=True)
class ProviderWeek:
    """One provider's current reset period; None means the figure is not readable from local logs."""

    provider: str
    period_start: datetime | None
    resets_at: datetime | None
    utilization: float | None
    utilization_at: datetime | None
    projection: Projection | None
    totals: Usage | None
    agents: tuple[AgentUsage, ...]


@dataclass(frozen=True)
class WeekUsage:
    now: datetime
    providers: tuple[ProviderWeek, ...]


def week(repos: Iterable[Path], lane_rows: Sequence[LaneRow], sessions_dir: Path, now: datetime, *,
         read_state: ReadState | None = None) -> WeekUsage:
    """Usage of the current reset period from flow run records and lane sessions; each session counts once."""
    found = sources.collect(repos, lane_rows, sessions_dir, read_state=read_state)
    return WeekUsage(now, tuple(_provider_week(name, [s for s in found if s.provider == name], now)
                                for name in PROVIDERS))


def sum_usage(usages: Iterable[Usage]) -> Usage:
    """Add up usages; the cost is the sum of the known costs, and unknown when none is known."""
    parts = list(usages)
    costs = [usage.cost_usd for usage in parts if usage.cost_usd is not None]
    return Usage(sum(u.uncached_input for u in parts), sum(u.cache_write for u in parts),
                 sum(u.cache_read for u in parts), sum(u.output for u in parts), sum(costs) if costs else None)


def _provider_week(provider: str, found: list[Source], now: datetime) -> ProviderWeek:
    samples = [sample for source in found for sample in source.samples]
    series = projection.current_series(samples, now, provider)
    resets_at = series[0].resets_at if series else _rolled_reset(samples, provider, now)
    if resets_at is None:
        return ProviderWeek(provider, None, None, None, None, None, None, ())
    start = resets_at - WEEK
    agents = _agents(found, start, resets_at)
    newest = series[-1] if series else None
    return ProviderWeek(provider, start, resets_at, newest.utilization if newest else None,
                        newest.at if newest else None, projection.project(samples, now, provider),
                        sum_usage(agent.usage for agent in agents), tuple(agents))


def _rolled_reset(samples: list[Sample], provider: str, now: datetime) -> datetime | None:
    """The reset after `now` implied by the latest weekly reset seen, when every sample's period is over."""
    resets = [s.resets_at for s in samples if s.provider == provider and s.window == projection.WEEK_WINDOW]
    if not resets:
        return None
    latest = max(resets)
    return latest + WEEK * ((now - latest) // WEEK + 1)


def _agents(found: list[Source], start: datetime, end: datetime) -> list[AgentUsage]:
    totals = [(source, sum_usage(event.usage for event in source.events if start <= event.at < end))
              for source in found]
    used = [(source, usage) for source, usage in totals if usage.total]
    grand = sum(usage.total for _, usage in used)
    return [AgentUsage(source.item, source.agent, usage, usage.total / grand) for source, usage in used]
