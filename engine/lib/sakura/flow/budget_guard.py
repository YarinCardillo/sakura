"""A running flow under the budget guard: the check before a role run, the waiting it leaves, and its hold.

Engine-free on purpose (the runner and the engine both import it). With no usage.toml under the engine's home the
budget is off and nothing here asks or waits; with one, an account, archive or snapshot that cannot be used asks.

The flow's own hold is its pending amount: while it is at or before the build phase the guard counts the flow's
preflight `high` week share on top of the live week and the holds of other flows, less what the live week gained
since the hold was taken (the flow's own spend is already in it). The evaluator and the reporter run after the build:
their share of the estimate (`post_build_pct`) stays counted until the report phase ends, so they are checked against
the reserve too. The hold stays until the flow finishes or is cleaned up.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone

from sakura.budget import account as budget_account
from sakura.budget import alerts, flow_cost, guard
from sakura.budget import settings as budget_settings
from sakura.budget import leeway_numbers
from sakura.flow import pace, pace_check, step_accounts
from sakura.usage import leeway
from sakura.flow.config import FlowConfig
from sakura.flow.store import TaskStore
from sakura.usage.archive import settings as archive_settings
from sakura.usage.archive import store as archive_store

WAIT_KEY, WAIT_ON = "waiting", "budget"
# Counts the waits of a task: a confirmation shown for one wait never releases another (`waits.approval_revision`).
WAIT_GENERATION_KEY = "wait_generation"
RELEASED_UNTIL, RELEASED_ACTION, RELEASED_RULE = "released_until", "released_action", "released_rule"
PENDING_PHASES = ("intake", "contract", "plan", "build")
POST_BUILD_PHASES = ("evaluate", "report")
HIGH_EDGE = 2
RULING = "Ruling"
CODEX = "codex"
CODEX_UNGUARDED_KEY = "codex_unguarded_noted"


class BudgetWait(Exception):
    """The guard says this flow may not run now; carries its decision (never `run`)."""

    def __init__(self, decision: guard.Decision) -> None:
        super().__init__(decision.reason)
        self.decision = decision


def local_now() -> datetime:
    """The clock in the machine's timezone: the `budget.night` window is local time."""
    return datetime.now(timezone.utc).astimezone()


def task_key(store: TaskStore) -> str:
    """The key of a flow in `budget_holds`: its repository and task id."""
    return f"{store.repo.resolve()}:{store.task_id}"


def evaluate(store: TaskStore, cfg: FlowConfig, now: datetime | None = None, provider: str = "claude", *,
             role: str = "", advisor: pace.Advisor | None = None, step: str = "",
             on_account: step_accounts.StepAccount | None = None) -> guard.Decision:
    """The guard's decision for the next run of a flow; `run` when the budget is off.

    The reserve rules decide first. When they let the run start, pace (`flow/pace_check.py`) judges the account of the
    step `role` runs (a decision with no `role` is the reserve alone). An owner release lifts only the rule that
    stopped the flow, until the release ends; the other rules still apply.

    `step` names the pipeline step the run serves when it is not the role's own (the final review).

    `on_account` is the account a conversation being continued ran on (`flow/resume_account.py`): the reserve and the
    numbers are those of that account, whatever the lane is configured to use now.

    `provider` is the provider of the role about to run: `codex` is judged on the Codex account's snapshot
    (see `_codex_decision`), anything else on the flow's Claude account.
    """
    now = now or local_now()
    if role:
        pace_check.drop_disallowed_move(store, role, step)
    state = store.read_state()
    if role and on_account is None and (reason := step_accounts.problem(cfg.values, state, role, provider, step)):
        return guard.Decision(guard.ASK, None, reason, step_accounts.ACCOUNT_RULE)
    reserve = _reserve_decision(store, cfg, now, provider, role, step, on_account)
    if reserve.action != guard.RUN or not role:
        return reserve
    advisor = advisor or pace_check.default_advisor(store.repo, role)
    lifted = _lifted_rule(state, now)
    return pace_check.decision(store, cfg, role=role, now=now, lifted=lifted, advisor=advisor, step=step) or reserve


def _reserve_decision(store: TaskStore, cfg: FlowConfig, now: datetime, provider: str, role: str,
                      step: str = "", on_account: step_accounts.StepAccount | None = None) -> guard.Decision:
    state = store.read_state()
    if not archive_settings.config_exists():
        return guard.Decision(guard.RUN, None, "")
    lifted = _lifted_rule(state, now)
    settings = budget_settings.load(cfg.values)
    try:
        with _archive() as (conn, usage):
            if provider == CODEX:
                return _codex_decision(store, conn, usage, settings, state, now, lifted, cfg, role, step, on_account)
            return _claude_decision(store, conn, usage, settings, state, now, lifted, cfg, provider, role, step, on_account)
    except step_accounts.StepAccountProblem as error:
        return guard.Decision(guard.ASK, None, str(error), step_accounts.ACCOUNT_RULE)
    except budget_account.UnknownAccountError as error:
        return guard.Decision(guard.ASK, None, f"quota unknown: {error}")
    except flow_cost.ARCHIVE_ERRORS as error:
        return guard.Decision(guard.ASK, None, f"quota unknown: usage archive unreadable: {type(error).__name__}: {error}")


def _claude_decision(store: TaskStore, conn: sqlite3.Connection, usage: archive_settings.Settings,
                     settings: budget_settings.BudgetSettings, state: dict, now: datetime, lifted: str, cfg: FlowConfig,
                     provider: str, role: str, step: str,
                     on_account: step_accounts.StepAccount | None = None) -> guard.Decision:
    """The reserve decision on the account the run is launched on: the step's own account when the preset (or pace)
    pins one or the book holds only one, with its reserve and Leeway readings; else the flow's budget account. A
    conversation being continued is judged on the account it ran on (`on_account`)."""
    resolved = on_account or (step_accounts.resolve(cfg.values, state, role, provider, step) if role else None)
    pending, tail = _pending(state)
    if resolved is not None:
        account, snapshot = resolved.key, resolved.snapshot
    else:
        account = budget_account.resolve(settings.account, usage.sources, disabled=usage.disabled)
        snapshot = _leeway_snapshot(account)
    return guard.check(account, now, conn, budget_settings.for_account(settings, usage.reserves.get(account)),
                       pending, state.get("urgent", True), task_key(store), tail, lifted, snapshot)


def leeway_snapshot(account: str) -> guard.Snapshot | None:
    """The numbers Leeway reports for `account`; None (the archive answers) when Leeway is absent or does not list it."""
    return _leeway_snapshot(account)


def _leeway_snapshot(account: str) -> guard.Snapshot | None:
    """The numbers Leeway reports for the flow's budget account; None (the archive answers) when Leeway is absent or
    does not list it."""
    try:
        rows = leeway.status()["accounts"]
    except leeway.LeewayError:
        return None
    return leeway_numbers.snapshot_of(rows, account)


def require(store: TaskStore, cfg: FlowConfig, now: datetime | None = None, provider: str = "claude", *,
            role: str = "", advisor: pace.Advisor | None = None, step: str = "",
            on_account: step_accounts.StepAccount | None = None) -> None:
    """Raise BudgetWait when the flow may not run now; a flow that may run drops any waiting it had."""
    decision = evaluate(store, cfg, now, provider, role=role, advisor=advisor, step=step, on_account=on_account)
    if decision.action != guard.RUN:
        raise BudgetWait(decision)
    if WAIT_KEY in store.read_state():
        store.update_state(lambda state: state.pop(WAIT_KEY, None))
        resolve_request_alerts(store, cfg, now)


def park(store: TaskStore, decision: guard.Decision, now: datetime) -> bool:
    """Record `waiting: budget` with `until` in the state, keeping everything else; True when the wait is new or moved.

    A decision whose `until` is already due (a reset that is unknown or past) is not a move: it would change on
    every look and rule and notify every time. A weekly pause gives the flow's hold back: it will spend after the
    reset, and a paused flow must not make the others wait for spend that is not coming this week.
    """
    until = decision.until.isoformat(timespec="seconds") if decision.until else None
    record = {"on": WAIT_ON, "action": decision.action, "until": until, "reason": decision.reason,
              "rule": decision.rule, **({"account": decision.account} if decision.account else {})}
    before = store.read_state().get(WAIT_KEY) or {}

    def record_wait(state: dict) -> None:
        state[WAIT_KEY] = record
        if not before:
            state[WAIT_GENERATION_KEY] = int(state.get(WAIT_GENERATION_KEY, 0)) + 1

    store.update_state(record_wait)
    if decision.action == guard.PAUSE:
        release_hold(store)
    moved = decision.until is not None and decision.until > now and until != before.get("until")
    return before.get("action") != decision.action or moved


def postpone(store: TaskStore, until: datetime) -> None:
    """Look at a timed wait again at `until` (its snapshot was stale, so nothing new is known)."""
    moved = until.isoformat(timespec="seconds")
    store.update_state(lambda state: state[WAIT_KEY].update(until=moved))


def release_wait(store: TaskStore, settings: budget_settings.BudgetSettings, now: datetime) -> bool:
    """The owner approved the budget gate while the flow waits: the rule that stopped it is lifted until the wait's end.

    Only that rule (the weekly reserve for a pause, the 5-hour reserve for a defer, the age of the snapshot for an
    ask) is lifted; every other rule keeps applying. A released pause lasts to the weekly reset and a released defer
    to the reset (or night) it waited for; a released ask, or a wait whose end is already due, lasts
    `snapshot_max_age_minutes`. Past that the guard applies again. False when the flow did not wait.
    """
    waiting = store.read_state().get(WAIT_KEY)
    if not waiting:
        return False
    until = waiting.get("until") and datetime.fromisoformat(waiting["until"])
    if not until or until <= now:
        until = now + timedelta(minutes=settings.snapshot_max_age_minutes)

    def release(state: dict) -> None:
        state.pop(WAIT_KEY, None)
        rule = waiting.get("rule") or guard.RULE_OF_ACTION.get(waiting["action"], "")
        state.setdefault("budget", {}).update({RELEASED_UNTIL: until.isoformat(timespec="seconds"),
                                               RELEASED_ACTION: waiting["action"], RELEASED_RULE: rule})

    store.update_state(release)
    return True


def waiting_until(state: dict) -> datetime | None:
    """When a flow waiting on the budget should be looked at again; None when it does not wait or only the owner decides."""
    waiting = state.get(WAIT_KEY) or {}
    if waiting.get("on") != WAIT_ON or not waiting.get("until"):
        return None
    return datetime.fromisoformat(waiting["until"])


def ruling_line(decision: guard.Decision) -> str:
    """The head of the ledger ruling: the action and, when there is one, the time it waits for."""
    return f"{decision.action} until {decision.until.isoformat(timespec='seconds')}" if decision.until \
        else f"{decision.action}, the owner decides"


def take_hold(store: TaskStore, cfg: FlowConfig, now: datetime | None = None) -> None:
    """Reserve the flow's preflight `high` week share; nothing to reserve means no row.

    The hold notes the live week figure it starts from, and starts afresh when the week it was taken in is over.
    """
    amount = _held_amount(store.read_state())
    if not amount or not archive_settings.config_exists():
        return
    now = now or datetime.now(timezone.utc)
    settings = budget_settings.load(cfg.values)
    try:
        with _archive() as (conn, usage):
            account = budget_account.resolve(settings.account, usage.sources, disabled=usage.disabled)
            week, week_start = guard.week_position(conn, account, now)
            guard.hold(conn, task_key(store), account, amount, now, week, week_start)
    except (budget_account.UnknownAccountError, *flow_cost.ARCHIVE_ERRORS) as error:
        store.append_ledger("budget", f"hold not taken: {type(error).__name__}: {error}")


def open_request_alert(store: TaskStore, cfg: FlowConfig, title: str, reason: str, now: datetime | None = None) -> None:
    """Open (or refresh the numbers of) the `request` alert of this flow; nothing when the budget is off."""
    now = now or datetime.now(timezone.utc)
    _with_account(store, cfg, lambda conn, account: alerts.open_request(
        conn, account, store.repo.name, store.task_id, title, reason, now))


def resolve_request_alerts(store: TaskStore, cfg: FlowConfig, now: datetime | None = None) -> None:
    """Close the `request` alerts of this flow: it was released, resumed or finished."""
    now = now or datetime.now(timezone.utc)
    _with_account(store, cfg, lambda conn, account: alerts.resolve_requests(
        conn, account, store.repo.name, store.task_id, now))


def settle(store: TaskStore, cfg: FlowConfig) -> None:
    """The flow finished: give its hold back and close its alerts."""
    release_hold(store)
    resolve_request_alerts(store, cfg)


def release_hold(store: TaskStore) -> None:
    """Give the flow's hold back (it finished, was cleaned up, or paused for the week)."""
    if not archive_settings.config_exists():
        return
    try:
        with _archive() as (conn, _):
            guard.release(conn, task_key(store))
    except flow_cost.ARCHIVE_ERRORS as error:
        store.append_ledger("budget", f"hold not released: {type(error).__name__}: {error}")


def _with_account(store: TaskStore, cfg: FlowConfig, action: Callable[[sqlite3.Connection, str], object]) -> None:
    """Run `action(conn, account)` on the usage archive; a failure is a ledger line, never an error for the flow."""
    if not archive_settings.config_exists():
        return
    settings = budget_settings.load(cfg.values)
    try:
        with _archive() as (conn, usage):
            action(conn, budget_account.resolve(settings.account, usage.sources, disabled=usage.disabled))
    except (budget_account.UnknownAccountError, *flow_cost.ARCHIVE_ERRORS) as error:
        store.append_ledger("budget", f"alert not recorded: {type(error).__name__}: {error}")


@contextmanager
def _archive() -> Iterator[tuple[sqlite3.Connection, archive_settings.Settings]]:
    usage = archive_settings.load()
    conn = archive_store.open_db(usage.db)
    try:
        yield conn, usage
    finally:
        conn.close()


def _codex_decision(store: TaskStore, conn: sqlite3.Connection, usage: archive_settings.Settings,
                    settings: budget_settings.BudgetSettings, state: dict, now: datetime,
                    lifted: str, cfg: FlowConfig, role: str, step: str = "",
                    on_account: step_accounts.StepAccount | None = None) -> guard.Decision:
    """The decision for a Codex role: the Codex account's snapshot as a lower bound, or `run` unguarded (said once).

    A step the preset pins to a Codex account is judged on that account, the one the launcher points the agent at;
    otherwise the Codex account paired with the flow's Claude budget account answers."""
    pinned = on_account or (step_accounts.resolve(cfg.values, state, role, CODEX, step) if role else None)
    if pinned is not None:
        return _pinned_codex_decision(conn, usage, settings, state, now, lifted, pinned)
    try:
        claude = budget_account.resolve(settings.account, usage.sources, disabled=usage.disabled)
    except budget_account.UnknownAccountError:
        claude = None  # no Claude account to name; the only Codex source, if there is one, still applies
    account = budget_account.codex_account(claude, usage.sources)
    if account is None:
        _note_codex_unguarded(store, state)
        return guard.Decision(guard.RUN, None, "")
    return guard.check_lower_bound(account, now, conn, budget_settings.for_account(settings, usage.reserves.get(account)),
                                   state.get("urgent", True), lifted)


def _pinned_codex_decision(conn: sqlite3.Connection, usage: archive_settings.Settings,
                           settings: budget_settings.BudgetSettings, state: dict, now: datetime, lifted: str,
                           pinned: step_accounts.StepAccount) -> guard.Decision:
    account = pinned.key
    return guard.check_lower_bound(account, now, conn, budget_settings.for_account(settings, usage.reserves.get(account)),
                                   state.get("urgent", True), lifted, pinned.snapshot)


def _note_codex_unguarded(store: TaskStore, state: dict) -> None:
    if (state.get("budget") or {}).get(CODEX_UNGUARDED_KEY):
        return
    store.append_ledger("budget", "codex roles run unguarded: no Codex source of usage.toml matches the budget account")
    store.update_state(lambda current: current.setdefault("budget", {}).update({CODEX_UNGUARDED_KEY: True}))


def _lifted_rule(state: dict, now: datetime) -> str:
    """The rule the owner released, while the release lasts; empty when none is."""
    budget = state.get("budget") or {}
    until = budget.get(RELEASED_UNTIL)
    if not until or now >= datetime.fromisoformat(until):
        return ""
    return budget.get(RELEASED_RULE) or guard.RULE_OF_ACTION.get(budget.get(RELEASED_ACTION), "")


def _held_amount(state: dict) -> float:
    """The preflight `high` week share of the flow, 0 when it has none (budget off, or no percentage)."""
    band = ((state.get("budget") or {}).get("preflight") or {}).get("week_pct")
    return float(band[HIGH_EDGE]) if band else 0.0


def _tail_amount(state: dict) -> float:
    """The share of the preflight `high` that the roles after the build account for, 0 when it is not known."""
    preflight = (state.get("budget") or {}).get("preflight") or {}
    return min(float(preflight.get("post_build_pct") or 0.0), _held_amount(state))


def _pending(state: dict) -> tuple[float, float]:
    """What the flow is still expected to spend, and the part of it kept for its phases after the build."""
    phase, tail = state.get("phase"), _tail_amount(state)
    if phase in PENDING_PHASES:
        return _held_amount(state), tail
    return (tail, tail) if phase in POST_BUILD_PHASES else (0.0, 0.0)
