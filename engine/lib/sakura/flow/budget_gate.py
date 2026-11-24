"""The budget gate of a flow: its predicted cost as a share of the account's week, asked about before build.

`check` runs at the end of the plan phase. The numbers go to `state.budget.preflight` (the guard of a later task
reads them to hold the flow's expected spend); a flow above `ask_flow_week_pct`, one that does not fit above the
reserve, or one whose quota is unknown waits at the `budget` approve gate. The approval belongs to one plan digest.
"""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime, timezone

from sakura.budget import flow_cost, preflight
from sakura.budget import settings as budget_settings
from sakura.budget.guard import Decision
from sakura.flow import budget_guard, plan
from sakura.flow.engine import BUDGET_GATE, FlowContext, PhaseOutcome

LEDGER_PHASE = "budget"
ASK_EVENT, WARN_EVENT = "budget_ask", "budget_warning"
PREFLIGHT_KIND = "preflight asks"
OPUS_EXTRA_PREFIX = " Extra for the Opus run: "


def check(ctx: FlowContext, plan_digest: str, inputs_digest: str = "", now: datetime | None = None) -> PhaseOutcome:
    """Compute and store the pre-flight of the accepted plan; `await` at the `budget` gate when it must ask.

    `inputs_digest` is the plan phase's own inputs digest; it is stored so the phase can tell that a plan
    waiting at the gate was made for the inputs it still has.

    Already approved for this plan digest gives `next`. With no usage archive configured the budget is off: one
    ledger line, the `off` preflight stored, no ask. An empty `budget.account` is the engine's own account;
    when that cannot be resolved the quota is unknown and the flow asks.
    """
    settings = budget_settings.load(ctx.config.values)
    if _is_approved(ctx, plan_digest):
        return PhaseOutcome("next")
    now = now or datetime.now(timezone.utc)
    result = compute(ctx, settings, plan.load(ctx.store, plan_digest), now)
    decision, line = preflight.level(result, settings), preflight.text(result)
    _store(ctx, result, settings, plan_digest, inputs_digest, now)
    if result.off:
        ctx.store.append_ledger(LEDGER_PHASE, f"off ({result.reason})")
        return PhaseOutcome("next")
    if decision == preflight.WARN:
        ctx.store.append_ledger(LEDGER_PHASE, f"preflight warns: {line}")
        ctx.notifier.notify(WARN_EVENT, ctx.store.task_id, line)
    if decision == preflight.ASK:
        record_request(ctx, result, line)
        return PhaseOutcome("await", _gate_reason(result, line), gate=BUDGET_GATE)
    return PhaseOutcome("next")


def record_request(ctx: FlowContext, result: preflight.Preflight | Decision, line: str, kind: str = PREFLIGHT_KIND,
                   event: str = ASK_EVENT) -> None:
    """The flow asks the owner, or waits for a budget reason: a ledger line and an event with the numbers.

    `result` is the pre-flight or the guard decision; its `reason` follows `line` in parentheses. The single
    place a `request` alert hooks into, for both the pre-flight gate and the live guard: the alert is written
    here and not pushed again (the flow's notifier above is the push).
    """
    why = f" ({result.reason})" if result.reason else ""
    ctx.store.append_ledger(LEDGER_PHASE, f"{kind}: {line}{why}")
    ctx.notifier.notify(event, ctx.store.task_id, f"{line}{why}")
    asks = " asks" if kind == PREFLIGHT_KIND else ""
    budget_guard.open_request_alert(ctx.store, ctx.config, f"{ctx.store.repo.name}/{ctx.store.task_id}{asks}: {line}",
                                    f"{line}{why}")


def record_decision(ctx: FlowContext, decision: Decision, now: datetime | None = None) -> None:
    """Park the flow on a guard decision (`waiting: budget`, `until`); a wait that is new or moved is ruled and notified.

    The ruling is a ledger line `Ruling: <action> until <time> (<reason with the numbers>)` and an event
    `budget_<action>`; a repeat of the same wait adds nothing.
    """
    if budget_guard.park(ctx.store, decision, now or budget_guard.local_now()):
        record_request(ctx, decision, budget_guard.ruling_line(decision), budget_guard.RULING, f"budget_{decision.action}")


def compute(ctx: FlowContext, settings: budget_settings.BudgetSettings, tasks: Sequence[plan.BuildTask],
            now: datetime) -> preflight.Preflight:
    """The pre-flight of a plan's tasks under the flow's config."""
    return flow_cost.for_tasks(ctx.config, tasks, settings, now)


def opus_extra_text(ctx: FlowContext, task: plan.BuildTask) -> str:
    """The cost of one Opus run of `task` as a share of the week, to append to the Opus gate reason."""
    settings = budget_settings.load(ctx.config.values)
    result = flow_cost.for_opus_run(task, settings, datetime.now(timezone.utc))
    return "" if result.off else OPUS_EXTRA_PREFIX + preflight.text(result)


def _is_approved(ctx: FlowContext, plan_digest: str) -> bool:
    return ctx.store.read_state()["approvals"].get(BUDGET_GATE, {}).get("plan_sha256") == plan_digest


def _gate_reason(result: preflight.Preflight, line: str) -> str:
    return f"{line}; {result.reason}" if result.reason and result.reason not in line else line


def _store(ctx: FlowContext, result: preflight.Preflight, settings: budget_settings.BudgetSettings,
           plan_digest: str, inputs_digest: str, now: datetime) -> None:
    record = {**preflight.summary(result, settings), "plan_sha256": plan_digest, "inputs_sha256": inputs_digest,
              "computed_at": now.isoformat(timespec="seconds")}
    ctx.store.update_state(lambda state: state.setdefault("budget", {}).update(preflight=record))
