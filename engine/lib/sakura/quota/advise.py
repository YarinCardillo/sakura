"""Where may this work run now: agent, model, effort and account, or when to retry.

A candidate is an (agent, account) pair the person's rules allow for the repository, in preference order. A
candidate is usable when its weekly forecast after one more round of work stays under the account's ceiling
(100 minus its weekly reserve in the account registry, else the engine's global reserve) and its five-hour window
is not full. The first usable candidate wins: `go` when it is the preferred one, `switch` when an earlier
candidate was blocked. With none usable the answer is `wait` until the earliest reset, or `deny` without one.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from sakura.accounts.registry import Registry
from sakura.budget.settings import effective_reserve
from sakura.flow.config import DEFAULTS
from sakura.quota import models
from sakura.quota.config import Allowed, QuotaConfig
from sakura.usage.archive import calibration

GO, SWITCH, WAIT, DENY = "go", "switch", "wait", "deny"
FULL_PERCENT = 100
FIVE_HOUR_FULL_PCT = 95
DEFAULT_RESERVE_PCT = DEFAULTS["budget"]["reserve_week_pct"]
ROLES = ("builder", "reviewer", "explorer", "planner", "evaluator")
TASK_CLASSES = ("mechanical", "integration", "design")
CODEX_FIRST_ROLES = ("builder", "reviewer")

Ceilings = Mapping[tuple[str, str], float]


@dataclass(frozen=True)
class Request:
    repo: Path
    role: str
    task_class: str
    agent: str | None = None


@dataclass(frozen=True)
class Advice:
    verdict: str
    agent: str
    model: str
    effort: str
    config_dir: str
    reason: str
    retry_at: str | None = None
    account: str = ""


@dataclass(frozen=True)
class _Block:
    reason: str
    until: datetime | None


def ceilings(registry: Registry) -> dict[tuple[str, str], float]:
    """The weekly ceiling in percent of every registered account: 100 minus its own reserve or the global one."""
    return {(a.provider, a.id): FULL_PERCENT - effective_reserve(a.reserve_week_pct, DEFAULT_RESERVE_PCT)
            for a in registry.accounts}


def advise(request: Request, document: dict, limits: Ceilings, config: QuotaConfig) -> Advice:
    """The advice for `request` against the status `document`."""
    project, allowed = config.allowed_for(request.repo)
    candidates = _candidates(request, document["accounts"], allowed)
    if not candidates:
        return Advice(DENY, request.agent or "", "", "", "", f"no account is allowed for project {project}"
                      + (f" on {request.agent}" if request.agent else ""))
    now = calibration.parse_moment(document["generated_at"])
    blocks: list[_Block] = []
    for account in candidates:
        block = _block(account, limits, now)
        if block is None:
            return _usable(request, config, account, blocks)
        blocks.append(block)
    return _blocked(request, config, candidates[0], blocks)


def _candidates(request: Request, accounts: Sequence[dict], allowed: Allowed) -> list[dict]:
    found: list[dict] = []
    for agent in _agents(request):
        names = allowed.codex if agent == models.CODEX else allowed.claude
        mine = [a for a in accounts if a["provider"] == agent]
        order = mine if names is None else [a for name in names for a in mine if a["id"] == name]
        found += order
    return found


def _agents(request: Request) -> tuple[str, ...]:
    if request.agent:
        return (request.agent,)
    return (models.CODEX, models.CLAUDE) if request.role in CODEX_FIRST_ROLES else (models.CLAUDE,)


def _block(account: dict, limits: Ceilings, now: datetime) -> _Block | None:
    five, week, cast = account["five_hour"], account["seven_day"], account["forecast"]
    name = account["label"]
    if five["pct"] is not None and five["pct"] >= FIVE_HOUR_FULL_PCT:
        return _Block(f"{name} five-hour window is at {five['pct']:.0f}%", _moment(five["resets_at"]))
    ahead = cast["next_round_pct"] if cast["next_round_pct"] is not None else week["pct"]
    ceiling = limits.get((account["provider"], account["id"]), FULL_PERCENT - DEFAULT_RESERVE_PCT)
    if ahead is not None and ahead > ceiling:
        return _Block(f"{name} is forecast at {ahead:.0f}% of its week after the next round, over its "
                      f"{ceiling:.0f}% ceiling", _moment(week["resets_at"]))
    return None


def _usable(request: Request, config: QuotaConfig, account: dict, blocks: list[_Block]) -> Advice:
    choice = models.choose(account["provider"], request.role, request.task_class, config)
    week = account["seven_day"]["pct"]
    here = f"{account['label']} ({account['provider']}) has room: " + (
        "no reading, allowed" if week is None else f"{week:.0f}% of its week")
    reason = here if not blocks else "; ".join(b.reason for b in blocks) + f"; using {here}"
    verdict = GO if not blocks else SWITCH
    return Advice(verdict, account["provider"], choice.model, choice.effort, account["config_dir"], reason,
                  None, account["id"])


def _blocked(request: Request, config: QuotaConfig, first: dict, blocks: list[_Block]) -> Advice:
    choice = models.choose(first["provider"], request.role, request.task_class, config)
    until = [b.until for b in blocks if b.until is not None]
    reason = "; ".join(b.reason for b in blocks)
    retry = min(until) if until else None
    verdict = WAIT if retry else DENY
    return Advice(verdict, first["provider"], choice.model, choice.effort, first["config_dir"], reason,
                  retry.isoformat() if retry else None, first["id"])


def _moment(text: str | None) -> datetime | None:
    return calibration.parse_moment(text) if text else None
