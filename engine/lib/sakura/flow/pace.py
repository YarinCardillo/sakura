"""Pace: where each account lands at its reset, and what a step does when its account would land above the line.

The engine's pace is one of five levels, each a line in percent of an account's week at its reset: the account's
forecast at the reset is judged against it by Leeway's advisor before every step. Above the line a step waits, or,
when the preset leaves the step's account to the project, moves to another account of the same provider that is
under it and that the person allowed for the project (`flow/pace_accounts.py`); with none allowed it waits. A lane has a pace of its own, one of the same five levels: the one chosen when it was made, or the engine's then, and changed
only by `sakura flow pace` on that lane; its line is the line of that level. The reserve and the
five-hour limit of the budget guard stay in force at every level; pace only adds waits to them.

This module holds the decision and nothing else: the advisor is passed in, so it is a function of its arguments.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Callable, Sequence

LEVELS = ("slowest", "slow", "normal", "fast", "fastest")
TARGETS = {"slowest": 50, "slow": 70, "normal": 90, "fast": 100, "fastest": None}
LANE_LEVELS = LEVELS
DEFAULT_LEVEL = "normal"
PROJECT_ACCOUNT = "project"
GO, SWITCH, WAIT, DENY = "go", "switch", "wait", "deny"
# Without a time from the advisor, a waiting step looks again after this long.
RECHECK = timedelta(minutes=15)

# `advise(account, target_pct, candidates)` is Leeway's advisor bound to a repository, a role and a task class: the
# account to judge (None: let Leeway choose among the project's), the line, and the accounts to fall back to.
Advisor = Callable[[str | None, int, Sequence[str] | None], dict]


class PaceError(ValueError):
    """Raised when a pace level is not one the engine knows."""


class AdvisorUnavailable(RuntimeError):
    """Raised by an advisor that could not answer (not installed, did not reply). The step runs: the reserve rules apply."""


@dataclass(frozen=True)
class Decision:
    """What pace says about starting a step: `go`, `switch` to `account`, `wait` until `until`, or `deny` (no account fits)."""

    kind: str
    account: str
    until: datetime | None
    reason: str


def lane_level(knobs: dict) -> str:
    """The pace a lane runs at: its own (`lane_pace`) when it has one, else the engine's. The one place that says it."""
    return knobs["lane_pace"] or knobs["pace"]


def landing_target(lane_level: str, engine_level: str) -> int | None:
    """The line, in percent of the week at the reset, for a lane: the line of its own level, or of the engine's when the lane
    has none (`""`); None means no line (only the reserve stops it). Raises PaceError for a level that is not known."""
    _require(engine_level, LEVELS, "pace")
    level = lane_level or engine_level
    _require(level, LEVELS, "lane pace")
    return TARGETS[level]


def decide(step_account: str, lane_level: str, engine_level: str, advise: Advisor,
           now: datetime | None = None, allowed: Sequence[str] = ()) -> Decision:
    """Ask the advisor where the step's account lands and turn its verdict into a decision.

    A step on the project's account may switch to the account the advisor names when `allowed` lists it; a switch
    to any other account, and a switch for a step pinned to an account, is a wait. Raises PaceError for an unknown
    level and whatever the advisor raises.
    """
    target = landing_target(lane_level, engine_level)
    if target is None:
        return Decision(GO, "", None, "no line: only the reserve stops this lane")
    pinned = step_account != PROJECT_ACCOUNT
    advice = advise(step_account if pinned else None, target, None)
    verdict, account = advice["verdict"], advice.get("account") or ""
    if verdict == "go":
        return Decision(GO, account, None, f"lands under {target}% of its week at the reset")
    if verdict == "switch" and not pinned and account in allowed:
        return Decision(SWITCH, account, None, f"{account} lands under {target}% of its week at the reset")
    if verdict == "deny":
        return Decision(DENY, "", None, "no account can run this step")
    held = step_account if pinned else account
    return Decision(WAIT, held, _until(advice, now), f"lands above {target}% of its week at the reset")


def admit(decision: Decision, others_running: int, lanes_min: int) -> Decision:
    """Never let a wait hold a lane back while fewer than `lanes_min` other lanes run."""
    if decision.kind == WAIT and others_running < lanes_min:
        return Decision(GO, decision.account, None, f"{decision.reason}; kept running: fewer than {lanes_min} lanes run")
    return decision


def _until(advice: dict, now: datetime | None) -> datetime | None:
    retry_at = advice.get("retry_at")
    if retry_at:
        return datetime.fromisoformat(str(retry_at).replace("Z", "+00:00"))
    return now + RECHECK if now is not None else None


def _require(level: str, allowed: Sequence[str], name: str) -> None:
    if level not in allowed:
        raise PaceError(f"{name} must be one of {', '.join(allowed)}; got {level!r}")
