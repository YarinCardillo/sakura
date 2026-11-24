"""The model and effort a role gets on each agent.

Tiers: builders, reviewers and explorers run on the cheapest model that fits (the engine's `[budget.routes]`
table, capped at Sonnet); the strongest model only goes to the planner and the evaluator. Opus is otherwise reached
by escalation only, which this service never grants. `[roles]` of the quota config replaces a tier.
"""

from __future__ import annotations

from dataclasses import dataclass

from sakura.budget import routing
from sakura.flow.config import DEFAULTS
from sakura.quota.config import QuotaConfig

CLAUDE, CODEX = "claude", "codex"
STRONGEST = "opus"
STRONG_ROLES = ("planner", "evaluator")
MECHANICAL = "mechanical"
DEFAULT_CODEX_EFFORT = "medium"
ROUTES = routing.load_routes(DEFAULTS)


@dataclass(frozen=True)
class Choice:
    model: str
    effort: str


def choose(agent: str, role: str, task_class: str, config: QuotaConfig) -> Choice:
    """Model and effort of `role` for a `task_class` task on `agent`. Codex keeps its configured model unless
    `codex_model` is set."""
    if role in config.roles:
        model, _, effort = config.roles[role].partition(":")
        return _for_agent(agent, model, effort, config)
    if role in STRONG_ROLES:
        return _for_agent(agent, STRONGEST, "", config)
    model, effort = _tier(role, task_class)
    return _for_agent(agent, model, effort, config)


def _tier(role: str, task_class: str) -> tuple[str, str]:
    if role == "builder":
        return ROUTES[task_class]
    if role == "reviewer":
        return ROUTES["reviewer"]
    return ROUTES[MECHANICAL] if task_class == MECHANICAL else ROUTES["integration"]


def _for_agent(agent: str, model: str, effort: str, config: QuotaConfig) -> Choice:
    if agent == CODEX:
        return Choice(config.codex_model, effort or DEFAULT_CODEX_EFFORT)
    return Choice(model, effort)
