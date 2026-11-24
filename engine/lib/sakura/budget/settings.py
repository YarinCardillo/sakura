"""Typed `[budget]` settings, read from the effective flow config."""

from __future__ import annotations

import re
from dataclasses import dataclass, replace
from typing import TypeVar

TABLE = "budget"
NIGHT_WINDOW = re.compile(r"(?P<start>([01]\d|2[0-3]):[0-5]\d)-(?P<end>([01]\d|2[0-3]):[0-5]\d)")
MAX_PERCENT = 100
MAX_SHARE = 1
Reserve = TypeVar("Reserve")


class BudgetSettingsError(ValueError):
    """The `[budget]` settings are malformed; the message names the offending key."""


@dataclass(frozen=True)
class BudgetSettings:
    """Reserve floors, flow cost thresholds and snapshot freshness, all in the units of their name."""

    account: str
    reserve_week_pct: float
    reserve_five_hour_pct: float
    warn_flow_week_pct: float
    ask_flow_week_pct: float
    outside_share_alert: float
    night: str
    snapshot_max_age_minutes: int


def load(values: dict) -> BudgetSettings:
    """Settings from the effective flow config `values` (the whole dict, its `budget` table is read).

    Raises BudgetSettingsError naming the key when a type or range is wrong.
    """
    table = values.get(TABLE)
    if not isinstance(table, dict):
        raise BudgetSettingsError("budget must be a table")
    settings = BudgetSettings(
        account=_text("account", table["account"]),
        reserve_week_pct=_percent("reserve_week_pct", table["reserve_week_pct"]),
        reserve_five_hour_pct=_percent("reserve_five_hour_pct", table["reserve_five_hour_pct"]),
        warn_flow_week_pct=_percent("warn_flow_week_pct", table["warn_flow_week_pct"]),
        ask_flow_week_pct=_percent("ask_flow_week_pct", table["ask_flow_week_pct"]),
        outside_share_alert=_share("outside_share_alert", table["outside_share_alert"]),
        night=_night("night", table["night"]),
        snapshot_max_age_minutes=_minutes("snapshot_max_age_minutes", table["snapshot_max_age_minutes"]),
    )
    if settings.warn_flow_week_pct > settings.ask_flow_week_pct:
        raise BudgetSettingsError("budget.warn_flow_week_pct must not exceed budget.ask_flow_week_pct")
    return settings


def effective_reserve(own: float | None, default: Reserve) -> float | Reserve:
    """The reserve in force for an account: its own `reserve_week_pct` when set, else `default` (the global one).

    The one rule behind the pace alert, the live guard, the pre-flight and the cockpit `stop_at_pct`.
    """
    return default if own is None else own


def for_account(settings: BudgetSettings, own: float | None) -> BudgetSettings:
    """`settings` with the weekly reserve of an account whose own reserve is `own` (None: the global one stays)."""
    return replace(settings, reserve_week_pct=effective_reserve(own, settings.reserve_week_pct))


def _number(key: str, value: object) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise BudgetSettingsError(f"budget.{key} must be a number; got {value!r}")
    return float(value)


def _percent(key: str, value: object) -> float:
    number = _number(key, value)
    if not 0 <= number <= MAX_PERCENT:
        raise BudgetSettingsError(f"budget.{key} must be between 0 and {MAX_PERCENT}; got {value!r}")
    return number


def _share(key: str, value: object) -> float:
    number = _number(key, value)
    if not 0 <= number <= MAX_SHARE:
        raise BudgetSettingsError(f"budget.{key} must be between 0 and {MAX_SHARE}; got {value!r}")
    return number


def _minutes(key: str, value: object) -> int:
    if type(value) is not int or value < 1:
        raise BudgetSettingsError(f"budget.{key} must be a positive integer; got {value!r}")
    return value


def _text(key: str, value: object) -> str:
    if not isinstance(value, str):
        raise BudgetSettingsError(f"budget.{key} must be a string; got {value!r}")
    return value


def _night(key: str, value: object) -> str:
    window = NIGHT_WINDOW.fullmatch(value) if isinstance(value, str) else None
    if window is None or window["start"] == window["end"]:
        raise BudgetSettingsError(f"budget.{key} must be HH:MM-HH:MM with different times; got {value!r}")
    return value
