"""The snapshot's `accounts` and `usage` as Leeway reports them (`usage/leeway.py`), in the documents of `docs/cockpit-api.md`.

The documents keep every field the engine's own account rows had (the old ones that Leeway cannot answer are empty), and
add the ones Leeway knows: `plan`, `forecast_at_reset_pct`, `exhausts_at`, `stale` and `hidden`. When Leeway cannot be
read the view says why in one line, and the caller keeps the engine's own documents.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field

from sakura.cockpit.account_actions import AccountTarget
from sakura.usage import leeway

UNAVAILABLE_PREFIX = "Usage is unavailable: "
# The account controls Leeway can carry out: the others (a reserve, a plan period) stay on the engine's own registry.
LEEWAY_ACTIONS = ("l", "n", "e", "X")
STATE_DISABLED, STATE_MISSING, STATE_OK = "disabled", "missing", "ok"
PERCENT = 100


@dataclass(frozen=True)
class LeewayView:
    """What Leeway says: the account documents and the usage document, or `problem` (one line) when it cannot be read."""

    accounts: list[dict] = field(default_factory=list)
    usage: dict = field(default_factory=dict)
    problem: str = ""

    @property
    def is_available(self) -> bool:
        """Whether the documents are Leeway's."""
        return not self.problem


def read(describe_actions) -> LeewayView:
    """Ask Leeway for the status and the account book. `describe_actions(target)` gives the actions of an account target."""
    try:
        status, book = leeway.status(), leeway.accounts()
    except leeway.LeewayNotFound:
        return LeewayView(problem=f"{UNAVAILABLE_PREFIX}{leeway.NOT_FOUND_TEXT}")
    except leeway.LeewayError as error:
        return LeewayView(problem=f"{UNAVAILABLE_PREFIX}{error}")
    shown = {row["id"]: row for row in status["accounts"]}
    return LeewayView([_account(entry, shown.get(entry["id"]), describe_actions) for entry in book],
                      _usage(status))


def _account(entry: dict, row: dict | None, describe_actions) -> dict:
    target = AccountTarget(entry["provider"], entry["id"])
    reading = row or {}
    week, five, forecast = (reading.get(name) or {} for name in ("seven_day", "five_hour", "forecast"))
    return {"provider": entry["provider"], "id": entry["id"], "key": entry["id"],
            "label": reading.get("label") or entry.get("label") or entry["id"], "state": _state(entry),
            "enabled": not entry["hidden"], "week_pct": week.get("pct"), "week_resets_at": week.get("resets_at"),
            "five_hour_pct": five.get("pct"), "reserve_week_pct": None, "reserve_week_pct_own": None,
            "stop_at_pct": None, "plans": [], "plan_choices": [], "open_alerts": 0,
            "plan": reading.get("plan"), "forecast_at_reset_pct": forecast.get("seven_day_pct_at_reset"),
            "exhausts_at": forecast.get("exhausts_at"), "stale": bool(reading.get("stale", False)),
            "hidden": entry["hidden"], "target": _target_id(target),
            "actions": [action for action in describe_actions(target) if action["key"] in LEEWAY_ACTIONS]}


def _state(entry: dict) -> str:
    if entry["hidden"]:
        return STATE_DISABLED
    return STATE_OK if entry.get("folder_exists", True) else STATE_MISSING


def _target_id(target: AccountTarget) -> str:
    return json.dumps(list(target.key), separators=(",", ":"))


def _usage(status: dict) -> dict:
    return {"now": status["generated_at"], "as_of": leeway.newest_reading(status),
            "providers": [_provider(row) for row in status["accounts"]]}


def _provider(row: dict) -> dict:
    week, forecast = row["seven_day"], row["forecast"]
    used = None if week["pct"] is None else week["pct"] / PERCENT
    at_reset = forecast.get("seven_day_pct_at_reset")
    projection = None if used is None or at_reset is None else {
        "current": used, "at_reset": at_reset / PERCENT, "limit_at": forecast.get("exhausts_at"),
        "trend": "rising" if at_reset > week["pct"] else "flat"}
    return {"provider": row["provider"], "account": row["id"], "period_start": None, "resets_at": week["resets_at"],
            "utilization_at": row["as_of"], "utilization": used, "projection": projection, "totals": None, "agents": []}
