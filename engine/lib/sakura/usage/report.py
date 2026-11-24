"""Text and JSON views of a WeekUsage; every string from a log, a repository or a lane name loses its controls."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

from sakura.flow.render import strip_controls
from sakura.usage.aggregate import AgentUsage, ProviderWeek, WeekUsage
from sakura.usage.model import Usage
from sakura.usage.projection import FULL, Projection

NOT_AVAILABLE = "not available"
NOT_ENOUGH_DATA = "not enough data"
SHARE_LABEL = "share of locally observed tokens"
TIME_FORMAT = "%Y-%m-%d %H:%M UTC"
LINE_BREAKS_TO_SPACE = str.maketrans("\r\n\t", "   ")


def render(week: WeekUsage) -> str:
    """Return the weekly figures as text; a figure the local logs cannot give prints `not available`."""
    lines: list[str] = []
    for entry in week.providers:
        lines += _provider_lines(entry)
    return strip_controls("\n".join(lines))


def week_json(week: WeekUsage) -> dict:
    """The week as a JSON-ready dict, the document `render_json` prints."""
    return {"now": week.now.isoformat(), "providers": [_provider_json(entry) for entry in week.providers]}


def render_json(week: WeekUsage) -> str:
    """Return the weekly figures as JSON; an unreadable figure is null."""
    return json.dumps(week_json(week), indent=2)


def _provider_lines(entry: ProviderWeek) -> list[str]:
    lines = [f"{entry.provider}  week {_period(entry)}",
             f"  utilization  {_utilization(entry)}",
             f"  projection   {_projection(entry)}",
             f"  tokens       {_tokens(entry.totals)}"]
    if entry.agents:
        lines.append(f"  per agent ({SHARE_LABEL}, not of the limit)")
        lines += [_agent_line(agent) for agent in entry.agents]
    return lines


def _period(entry: ProviderWeek) -> str:
    if entry.period_start is None or entry.resets_at is None:
        return NOT_AVAILABLE
    return f"{_time(entry.period_start)} to {_time(entry.resets_at)}"


def _utilization(entry: ProviderWeek) -> str:
    if entry.utilization is None or entry.utilization_at is None:
        return NOT_AVAILABLE
    return f"{_percent(entry.utilization)} (sampled {_time(entry.utilization_at)})"


def _projection(entry: ProviderWeek) -> str:
    if entry.utilization is None:
        return NOT_AVAILABLE
    return _describe(entry.projection) if entry.projection else NOT_ENOUGH_DATA


def _describe(found: Projection) -> str:
    if found.trend == "flat":
        return f"flat at {_percent(found.current)}"
    text = f"{_percent(found.at_reset)} at the reset"
    if found.limit_at is not None:
        text += f", reaches {_percent(FULL)} at {_time(found.limit_at)}"
    return text


def _tokens(totals: Usage | None) -> str:
    if totals is None:
        return NOT_AVAILABLE
    text = f"{totals.total:,}"
    return f"{text} (cost ${totals.cost_usd:.2f})" if totals.cost_usd is not None else text


def _agent_line(agent: AgentUsage) -> str:
    return (f"    {_label(agent.item)} {_one_line(agent.agent)}  {agent.usage.total:,}  "
            f"{_percent(agent.share_of_local_tokens)}")


def _label(item: tuple[str, ...]) -> str:
    kind, scope, name = (_one_line(part) for part in item)
    return f"flow {name} ({Path(scope).name})" if kind == "flow" else f"lane {scope}/{name}"


def _one_line(text: str) -> str:
    """Turn CR, LF and tab into spaces: strip_controls keeps them, and a name must not forge report lines."""
    return text.translate(LINE_BREAKS_TO_SPACE)


def _percent(value: float) -> str:
    return f"{value * 100:.1f}%"


def _time(moment: datetime) -> str:
    return moment.strftime(TIME_FORMAT)


def _provider_json(entry: ProviderWeek) -> dict:
    return {
        "provider": entry.provider, "period_start": _iso(entry.period_start), "resets_at": _iso(entry.resets_at),
        "utilization": entry.utilization, "utilization_at": _iso(entry.utilization_at),
        "projection": _projection_json(entry.projection), "totals": usage_json(entry.totals),
        "agents": [_agent_json(agent) for agent in entry.agents],
    }


def _projection_json(found: Projection | None) -> dict | None:
    if found is None:
        return None
    return {"current": found.current, "at_reset": found.at_reset, "limit_at": _iso(found.limit_at),
            "trend": found.trend}


def usage_json(usage: Usage | None) -> dict | None:
    if usage is None:
        return None
    return {"uncached_input": usage.uncached_input, "cache_write": usage.cache_write, "cache_read": usage.cache_read,
            "output": usage.output, "total": usage.total, "cost_usd": usage.cost_usd}


def _agent_json(agent: AgentUsage) -> dict:
    return {"item": [strip_controls(part) for part in agent.item], "agent": strip_controls(agent.agent),
            "usage": usage_json(agent.usage), "share_of_local_tokens": agent.share_of_local_tokens}


def _iso(moment: datetime | None) -> str | None:
    return moment.isoformat() if moment else None
