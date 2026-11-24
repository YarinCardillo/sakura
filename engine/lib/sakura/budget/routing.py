"""Model routing: the cheapest Claude model that fits a build-task class or a role.

The table lives in `[budget.routes]`; a value is `model` or `model:effort`. Opus is never a table value:
it is only reached by escalation, which needs the person's approval.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from sakura.accounts.registry import Account
    from sakura.flow.config import FlowConfig

TASK_CLASSES = ("mechanical", "integration", "design")
DEFAULT_TASK_CLASS = "integration"
ROUTE_KEYS = (*TASK_CLASSES, "reviewer", "reporter", "intake")
MODEL_LADDER = ("haiku", "sonnet", "opus")
EFFORTS = ("low", "medium", "high", "xhigh")
FAILURES_PER_STEP = 2
DEFAULTS_ORIGIN = "defaults"
BUILDER = "builder"
INTAKE = "intake"
KEY_BY_ROLE = {"plan_reviewer": "reviewer", "code_reviewer": "reviewer", "reporter": "reporter", INTAKE: INTAKE}
CONFIGURED_ROLE_OF = {INTAKE: "planner"}


class RoutingError(ValueError):
    """The `[budget.routes]` table is malformed; the message names the offending key."""


@dataclass(frozen=True)
class Route:
    """The model decided for one run; an empty `model` means the role keeps its configured model.

    `key` is the table row that decided it (a task class, `reviewer`, `reporter` or `intake`).
    """

    model: str
    effort: str
    reason: str
    needs_approval: bool
    key: str = ""


KEPT = Route("", "", "", False)


def route(role: str, task_class: str | None, failures: int, cfg: FlowConfig) -> Route:
    """Pick the model for `role`, or `INTAKE` for the intake phase of the planner.

    `task_class` matters for the builder only (None means `integration`, the class of an old plan).
    Every `FAILURES_PER_STEP` failed review rounds of the task move one step up the haiku, sonnet,
    opus ladder; reaching opus sets `needs_approval`. The route's model is capped by the dial: a routed role
    never runs on a model dearer than the dial's Claude model (`save` keeps integration builders on haiku,
    `strong` and `power` do not lift them to opus). A role the owner configured (any layer above
    the defaults), a role that is not Claude, the planner and the evaluator keep their own model.

    Raises ValueError for an unknown task class.
    """
    key = _key(role, task_class)
    if key is None or not _is_left_to_the_router(role, cfg):
        return KEPT
    routed, effort = load_routes(cfg.values)[key]
    model = _capped_by_dial(routed, cfg)
    top = min(MODEL_LADDER.index(model) + failures // FAILURES_PER_STEP, len(MODEL_LADDER) - 1)
    label = f"{key} task" if key in TASK_CLASSES else f"{key} role"
    if model != routed:
        label += f" (dial {cfg.values['dial']} caps the route's {routed})"
    if MODEL_LADDER[top] == model:
        return Route(model, effort, f"{label}: {model}" + (f" {effort} effort" if effort else ""), False, key)
    reason = f"{label}, {failures} failed review rounds: {model} -> {MODEL_LADDER[top]}"
    return Route(MODEL_LADDER[top], effort, reason, MODEL_LADDER[top] == MODEL_LADDER[-1], key)


def load_routes(values: dict) -> dict[str, tuple[str, str]]:
    """Each route key of the effective flow config `values` as (model, effort); effort is empty for the dial's.

    Raises RoutingError naming the key when a value is not `haiku|sonnet[:effort]`.
    """
    table = values["budget"]["routes"]
    return {key: _parse(key, table[key]) for key in ROUTE_KEYS}


def _capped_by_dial(model: str, cfg: FlowConfig) -> str:
    """The cheaper of `model` and the dial's Claude model."""
    from sakura.flow.config import DIAL

    dial_model = DIAL[cfg.values["dial"]].claude_model
    return min(model, dial_model, key=MODEL_LADDER.index)


def _key(role: str, task_class: str | None) -> str | None:
    if role != BUILDER:
        return KEY_BY_ROLE.get(role)
    chosen = task_class or DEFAULT_TASK_CLASS
    if chosen not in TASK_CLASSES:
        raise ValueError(f"unknown task class {chosen!r}; use one of {', '.join(TASK_CLASSES)}")
    return chosen


def _is_left_to_the_router(role: str, cfg: FlowConfig) -> bool:
    configured = CONFIGURED_ROLE_OF.get(role, role)
    provider = str(cfg.values["roles"][configured]).partition(":")[0]
    return provider == "claude" and cfg.origins[f"roles.{configured}"] == DEFAULTS_ORIGIN


def _parse(key: str, value: object) -> tuple[str, str]:
    model, _, effort = value.partition(":") if isinstance(value, str) else ("", "", "")
    routable = MODEL_LADDER[:-1]
    if model not in routable or (effort and effort not in EFFORTS):
        raise RoutingError(f"budget.routes.{key} must be one of {', '.join(routable)}, optionally followed by "
                           f":{'|'.join(EFFORTS)}; got {value!r}")
    return model, effort


@dataclass(frozen=True)
class GooseRoute:
    """The selected Goose account, upstream provider and requested model."""

    account: Account
    provider: str
    model: str
    effort: str = ""


def goose_route(account_id: str | None, model: str) -> GooseRoute:
    """Resolve the explicit Goose route without consulting subscription accounts or the Claude model ladder."""
    from sakura import programs
    from sakura.accounts import goose

    account = goose.lane_account(account_id)
    if not account.goose_provider:
        raise programs.ProgramError("goose_account_setup_required")
    return GooseRoute(account, account.goose_provider, model)
