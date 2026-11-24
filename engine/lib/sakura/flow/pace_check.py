"""Pace before a step runs: the step's account judged against the line, as a decision of the budget guard.

`decision` is called by `budget_guard.evaluate` for a role run. It asks the advisor (`flow/pace.py`), keeps the lane
running while fewer than `lanes_min` lanes run, and turns the answer into the guard's own words: a wait is a `defer`
with the rule `pace` (which an approval of the wait lifts like any rule), no account that fits is an `ask`. Without an
advisor, or when it does not answer, nothing here stops the step: the reserve rules of the guard still apply.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path

from sakura.budget import guard
from sakura.flow import pace, pace_accounts, pipeline, registry, store as flow_store
from sakura.flow.config import FlowConfig
from sakura.flow.store import TaskStore

PACE_RULE = guard.PACE_RULE
# The step of the pipeline each role runs; a role that is not here (the reporter) runs on the project's account.
STEP_OF_ROLE = {"planner": "plan", "plan_reviewer": "plan_review", "builder": "build", "code_reviewer": "task_review",
                "evaluator": "checks"}
UNAVAILABLE_NOTED_KEY = "pace_unavailable_noted"
ACCOUNT_KEY = "pace_account"
STATUS_RUNNING = "running"
WAITING_KEY = "waiting"


def step_of(role: str, step: str = "") -> str | None:
    """The pipeline step a run serves: `step` when the request names one (the final review), else the role's own."""
    return step or STEP_OF_ROLE.get(role)


def decision(store: TaskStore, cfg: FlowConfig, *, role: str, now: datetime, lifted: str = "",
             advisor: pace.Advisor | None = None, step: str = "") -> guard.Decision | None:
    """The guard decision pace makes for `role` to run now, or None when pace lets it run (or says nothing)."""
    step = step_of(role, step)
    if step is None or advisor is None or lifted == PACE_RULE:
        return None
    knobs = cfg.values["knobs"]
    account = cfg.values["step_accounts"].get(step, pace.PROJECT_ACCOUNT)
    allowed = pace_accounts.allowed(store.repo, None)
    try:
        found = pace.decide(account, knobs["lane_pace"], knobs["pace"], advisor, now, allowed)
    except pace.AdvisorUnavailable as error:
        _note_unavailable(store, error)
        return None
    found = pace.admit(found, others_running(store), knobs["lanes_min"])
    return _as_guard_decision(store, step, found)


# The role names Leeway's advisor knows, for the roles of a lane; the task class is the middle one.
LEEWAY_ROLE = {"planner": "planner", "plan_reviewer": "reviewer", "builder": "builder", "code_reviewer": "reviewer",
               "evaluator": "evaluator"}
LEEWAY_TASK_CLASS = "integration"


def default_advisor(repo: Path, role: str) -> pace.Advisor | None:
    """Leeway's advisor bound to `repo` and `role`, or None when no helper is found (only the reserve rules apply)."""
    from sakura.usage import leeway

    if role not in LEEWAY_ROLE or not leeway.is_available():
        return None
    return lambda account, target, candidates: leeway.advise(
        str(repo), LEEWAY_ROLE[role], LEEWAY_TASK_CLASS, account, target, candidates)


def others_running(asking: TaskStore) -> int:
    """How many lanes of the registered projects run now: running, not waiting on anything, and not `asking`."""
    count = 0
    for repo in registry.repos():
        for other in flow_store.task_stores(repo):
            if other.task_id == asking.task_id and other.repo == asking.repo:
                continue
            count += _is_running(other)
    return count


def _is_running(other: TaskStore) -> bool:
    try:
        state = other.read_state()
    except (flow_store.StateError, OSError):
        return False
    return state.get("status") == STATUS_RUNNING and not state.get(WAITING_KEY)


def _as_guard_decision(store: TaskStore, step: str, found: pace.Decision) -> guard.Decision | None:
    if found.kind == pace.WAIT:
        return guard.Decision(guard.DEFER, found.until, f"the {step} step {found.reason}", PACE_RULE, found.account)
    if found.kind == pace.DENY:
        return guard.Decision(guard.ASK, None, f"the {step} step: {found.reason}", PACE_RULE, found.account)
    if found.kind == pace.SWITCH:
        _keep_account(store, step, found.account)
    return None


def drop_disallowed_move(store: TaskStore, role: str, step: str = "") -> None:
    """Forget the account pace moved the step of `role` to when the person no longer allows it for the project: the
    step goes back to the project's account. The guard calls it before it reads the step's account."""
    step = step_of(role, step)
    if step is None:
        return
    chosen = (store.read_state().get("budget") or {}).get(ACCOUNT_KEY, {})
    if step not in chosen or chosen[step] in pace_accounts.allowed(store.repo, None):
        return
    store.update_state(lambda state: state["budget"][ACCOUNT_KEY].pop(step, None))
    store.append_ledger("pace", f"{step} moves to the account {pace.PROJECT_ACCOUNT}")


def _keep_account(store: TaskStore, step: str, account: str) -> None:
    """Keep the account a step moves to, where the launcher reads it, and say so once per change."""
    chosen = (store.read_state().get("budget") or {}).get(ACCOUNT_KEY, {})
    if chosen.get(step) == account:
        return
    store.update_state(lambda state: state.setdefault("budget", {}).setdefault(ACCOUNT_KEY, {}).update({step: account}))
    store.append_ledger("pace", f"{step} moves to the account {account}")


def _note_unavailable(store: TaskStore, error: Exception) -> None:
    if (store.read_state().get("budget") or {}).get(UNAVAILABLE_NOTED_KEY):
        return
    store.update_state(lambda state: state.setdefault("budget", {}).update({UNAVAILABLE_NOTED_KEY: True}))
    store.append_ledger("pace", f"{error}; steps run on the reserve rules alone")
