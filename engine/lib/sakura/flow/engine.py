"""Phase registry and the advance loop."""

from __future__ import annotations

import hashlib
from contextlib import ExitStack
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Literal

from sakura.flow import budget_guard, checks, config as flow_config, final_head, git_config, gitutil, stacks, task_repository
from sakura.flow import exits, inbox, lane_steps, launchers, notify, process, roles, run_records, runner, status_page, surface, yield_person
from sakura.flow.render import strip_controls
from sakura.flow.store import StateTampered, TaskStore
from sakura.tracks.domain import Refusal as TrackRefusal

PHASE_ORDER = ["intake", "contract", "plan", "build", "evaluate", "pr", "report", "cleanup"]
STATUS_BY_KIND = {"await": "awaiting_approval", "blocked": "blocked", "failed": "failed", "stopped": "stopped"}
EXIT_BY_KIND = {"await": exits.OK, "blocked": exits.BLOCKED, "failed": exits.FAILED, "stopped": exits.STOPPED}
STATUS_RUNNING = "running"
STATUS_CLEANED = "cleaned"
STATUS_BLOCKED = "blocked"
UNGUARDED_PHASES = ("pr", "cleanup")
STEPS_BOUNDARY = "steps"


@dataclass(frozen=True)
class PhaseOutcome:
    """What a phase handler decided."""

    kind: Literal["next", "await", "skip", "blocked", "failed", "stopped"]
    reason: str = ""
    gate: str = ""
    event: str = ""


@dataclass(frozen=True)
class FlowContext:
    """Everything a phase handler may use."""

    repo: Path
    store: TaskStore
    config: flow_config.FlowConfig
    runner: runner.AgentRunner
    notifier: notify.Notifier
    steps: flow_config.PresetSteps
    surface: surface.Surface = surface.NullSurface()


Handler = Callable[[FlowContext], PhaseOutcome]
InputsHash = Callable[[FlowContext], str]
_HANDLERS: dict[str, Handler] = {}
_INPUTS: dict[str, InputsHash] = {}
TAMPERED_REASON = "state.json changed outside the engine"
SPEC_GATE = "spec"
BUDGET_GATE = "budget"  # the preflight asks before the build
BUDGET_WAIT_GATE = "budget-wait"  # the guard stopped a running flow; approving it releases the rule that stopped it
OPUS_GATE_PREFIX = "opus:"
FINAL_REVIEW_GATE = "final-review"  # the final review still asked for changes after its rounds
OVERLAP_GATE = "overlap"  # delivery would conflict with a lane of the project that delivered first


def opus_gate(task_id: str) -> str:
    """The approve gate that lets build task `task_id` run its builder on Opus."""
    return f"{OPUS_GATE_PREFIX}{task_id}"


def register(name: str, handler: Handler, inputs: InputsHash | None = None) -> None:
    """Register the handler of a phase from PHASE_ORDER.

    `inputs` returns a digest of everything the phase depends on. When it equals
    the digest recorded after the phase last completed, the handler is skipped.
    """
    _HANDLERS[name] = handler
    if inputs is not None:
        _INPUTS[name] = inputs
    else:
        _INPUTS.pop(name, None)


def open_context(repo: Path, store: TaskStore) -> FlowContext:
    """Build the context of a stored task, using the overrides saved at start.

    `repo` is the person's repository, never a task workspace: an operator command run in a workspace and the
    evaluator's row tool both resolve it first (`task_repository.operator_repository`, `open_for_helper`), so the
    configuration, workspace roots, transport, `gh`, profiles and reports are the person's (review 10, finding 4).
    """
    state = store.read_state()
    cfg = flow_config.load(repo, overrides=state.get("overrides") or None)
    person = task_repository.person_common_dir(repo)
    # A task started before the digest was recorded is pinned to its config as this process finds it.
    gitutil.pin_config(person, state.get("git_config_sha256") or git_config.digest(person))
    task_repository.pin(state)
    shown = surface.build_surface(cfg)
    store.set_state_observer(lambda written: status_page.refresh(written, cfg))
    return FlowContext(repo, store, cfg, runner.AgentRunner(store, cfg, surface=shown), notify.build_notifier(store, cfg),
                       flow_config.steps(cfg.values), shown)


def apply_inbox(ctx: FlowContext, boundary: str) -> FlowContext:
    """Apply the waiting operator commands and return the context to continue with.

    The same context comes back when nothing changed; otherwise it is rebuilt from the
    stored overrides so the config, the steps and the runner see every dial and role effect.
    """
    consumed = inbox.consume(ctx.store, ctx.repo, boundary)
    if not consumed and not _overrides_moved(ctx):
        return ctx
    return open_context(ctx.repo, ctx.store)


def current_steps(ctx: FlowContext) -> flow_config.PresetSteps:
    """The steps the lane runs with now: a preset or a step the person switched while a phase ran (the runner takes the
    command in at the next agent run) reaches the reviews and checks of the tasks still to come, not only the next phase.

    Commands still waiting in the mailbox are taken in first: a switch queued while the last builder ran must decide
    whether the final review runs, and no agent run comes between the last build and that decision."""
    inbox.consume(ctx.store, ctx.repo, STEPS_BOUNDARY)
    if _overrides_moved(ctx):
        cfg = flow_config.load(ctx.repo, overrides=ctx.store.read_state().get("overrides") or None)
        object.__setattr__(ctx, "config", cfg)
        object.__setattr__(ctx, "steps", flow_config.steps(cfg.values))
    return ctx.steps


def _overrides_moved(ctx: FlowContext) -> bool:
    """Whether the lane's overrides name another preset, or another value of a step, than the context was built with (an
    operator switch)."""
    state = ctx.store.read_state()
    named = (state.get("overrides") or {}).get("preset")
    return (bool(named) and named != ctx.config.values["preset"]) or lane_steps.stored_knobs_moved(ctx.config.values, state)


def advance(ctx: FlowContext) -> int:
    """Run phases until one does not say `next`; returns the exit code."""
    with ExitStack() as admission:
        try:
            return _advance(ctx, admission)
        except TrackRefusal:
            raise
        except StateTampered:
            return _block_on_tamper(ctx)
        except Exception as error:
            _block_on_error(ctx, error)
            raise


def _advance(ctx: FlowContext, admission: ExitStack) -> int:
    from sakura.flow import resumed_work  # imported here: it imports this module
    from sakura.tracks.starts import admit_reactivation

    status = ctx.store.read_state()["status"]
    if status == STATUS_CLEANED:
        return exits.OK
    if status == "report_ready":
        admission.enter_context(admit_reactivation(ctx.store))
    ctx.surface.task_opened(ctx.store)
    kill_stale_run(ctx)
    if _spec_changed_after_approval(ctx):
        return _stop_at(ctx, "intake", PhaseOutcome("await", gate=SPEC_GATE))
    _reopen_stale_verification(ctx)
    _set_status(ctx, STATUS_RUNNING, "")
    admission.close()
    yield_person.clear(ctx.store)
    while True:
        held = resumed_work.settle(ctx)
        if held:
            return _stop_at(ctx, ctx.store.read_state()["phase"], held)
        phase = ctx.store.read_state()["phase"]
        ctx = apply_inbox(ctx, phase)
        outcome = (_stopped_by_operator(ctx, phase) or _yield_to_person(ctx, phase) or _budget_stop(ctx, phase)
                   or _run_phase(ctx, phase))
        if outcome.kind in ("next", "skip"):
            _complete(ctx, phase, outcome)
            if ctx.store.read_state()["status"] == "report_ready":
                budget_guard.settle(ctx.store, ctx.config)
                ctx.surface.task_settled(ctx.store)
                return exits.OK
            if phase == PHASE_ORDER[-1]:
                _set_status(ctx, STATUS_CLEANED, "")
                budget_guard.settle(ctx.store, ctx.config)
                return exits.OK
            continue
        code = _stop_at(ctx, phase, outcome)
        if outcome.kind == "failed":
            ctx.surface.task_settled(ctx.store)
        return code


def _reopen_stale_verification(ctx: FlowContext) -> None:
    """Reopen build or evaluation when a later phase is resumed with stale verification inputs."""
    from sakura.flow.phases import evaluate, integrate

    state = ctx.store.read_state()
    current = state.get("phase", PHASE_ORDER[0])
    current_index = PHASE_ORDER.index(current)
    if state.get("status") == "report_ready" and current == "cleanup":
        return
    target = None
    if current_index > PHASE_ORDER.index("build") and state.get("full_green_head"):
        # The engine's own head, never workspace 0's (`flow/final_head.py`): a survivor may move that branch.
        current_head = final_head.recorded(state)
        if (not current_head or current_head != state["full_green_head"]
                or state.get("full_green_inputs_sha256") != integrate.full_tests_inputs_hash(ctx)):
            target = "build"
    completed_evaluation = state.get("completed", {}).get("evaluate")
    if target is None and ctx.steps.evaluate and current_index > PHASE_ORDER.index("evaluate") and completed_evaluation:
        workspace = state.get("workspaces", {}).get("0", {})
        if not workspace.get("path") or not state.get("contract_sha256"):
            target = "build"
        elif completed_evaluation != evaluate.inputs_hash(ctx):
            target = "evaluate"
    if target is not None:
        reopen_phase(ctx, target, "reopened after verification inputs changed")


def reopen_phase(ctx: FlowContext, target: str, why: str) -> None:
    """Put the lane back at `target`: that phase and every later one run again (`why` goes to the ledger)."""
    def reopen(current_state: dict) -> None:
        current_state["phase"] = target
        current_state["status"] = STATUS_RUNNING
        current_state["reason"] = ""
        for phase in PHASE_ORDER[PHASE_ORDER.index(target):]:
            current_state.get("completed", {}).pop(phase, None)
        if target == "build":
            current_state["full_green_head"] = None
            current_state["full_green_inputs_sha256"] = None

    ctx.store.update_state(reopen)
    ctx.store.append_ledger(target, why)


def kill_stale_run(ctx: FlowContext) -> None:
    """Stop the agents a dead engine left running, each through the launcher of its host, then orphan their records.

    A run whose host has no launcher here is reported and left in `active_runs`.
    """
    state = ctx.store.read_state()
    runs = [*state.get("active_runs", []), *([state["active_run"]] if state.get("active_run") else [])]
    unique = {(run.get("host", launchers.LOCAL_HOST), run.get("run") or run.get("pid")): run for run in runs}
    chats = [run for run in unique.values() if _is_live_chat_run(state, run)]
    left = [run for run in unique.values() if run not in chats and not _stop_stale(ctx, run)] + chats
    if runs:
        ctx.store.update_state(lambda current: current.update(active_run=left[-1] if left else None, active_runs=left))
    run_records.reconcile(ctx.store, launchers.entry_is_alive)
    from sakura.flow import agent_resume  # imported here: it imports the runner

    agent_resume.reconcile_retained(ctx.store)
    for active in ctx.store.external_runs():
        pid, command = active["pid"], active["command"]
        killed = process.kill_tree(pid, command)
        if active.get("nonce"):
            process.end_group(active["pgid"], active["nonce"])
        if killed or not process.is_alive(pid):
            ctx.store.remove_external_run(pid, command)
            ctx.store.append_ledger("engine", f"cleared stale external run {pid}")


def _is_live_chat_run(state: dict, entry: dict) -> bool:
    """Whether `entry` is a run that continues an agent's conversation (`flow/agent_resume.py`) and is still alive: its own
    worker supervises it, so the lane starting again is no reason to end it."""
    record = state.get("runs", {}).get(entry.get("run"), {})
    return bool(record.get("resumes")) and launchers.entry_is_alive(entry)


def _stop_stale(ctx: FlowContext, entry: dict) -> bool:
    """Stop one leftover run; return False when this machine has no launcher for its host."""
    host, label = entry.get("host", launchers.LOCAL_HOST), entry.get("run") or entry.get("pid")
    launcher = launchers.for_host(host)
    if launcher is None:
        ctx.store.append_ledger("engine", f"run {label} on {host} has no launcher here")
        return False
    if launcher.is_alive(entry):
        launcher.kill(entry)
        ctx.store.append_ledger("engine", f"killed stale run {label}")
    elif entry.get("nonce"):
        # The leader is gone, but what it left in its group or carrying its nonce may not be.
        launcher.kill(entry)
        ctx.store.append_ledger("engine", f"ended what was left of stale run {label}")
    return True


def _stopped_by_operator(ctx: FlowContext, phase: str) -> PhaseOutcome | None:
    if ctx.store.stop_file.exists():
        return PhaseOutcome("stopped", f"stopped before {phase}")
    return None


def _yield_to_person(ctx: FlowContext, phase: str) -> PhaseOutcome | None:
    """Before a phase that spends, wait while the person works with an agent elsewhere (`yield_person`, a host switch,
    off by default); the operator's stop file ends the wait and stops the lane here."""
    if phase in UNGUARDED_PHASES:
        return None
    yield_person.hold(ctx.store, yield_person.real_ports())
    return _stopped_by_operator(ctx, phase)


def _budget_stop(ctx: FlowContext, phase: str) -> PhaseOutcome | None:
    """The mailbox safe point of the budget: take the hold when the flow starts building, stop when the guard says so."""
    if phase in UNGUARDED_PHASES:
        return None
    if phase == "build":
        budget_guard.take_hold(ctx.store, ctx.config)
    try:
        budget_guard.require(ctx.store, ctx.config)
    except budget_guard.BudgetWait as wait:
        return _budget_outcome(ctx, wait.decision)
    return None


def _budget_outcome(ctx: FlowContext, decision) -> PhaseOutcome:
    """Wait at the `budget` gate with the state intact; the owner may approve it, and the wake command resumes it at `until`."""
    from sakura.flow import budget_gate

    budget_gate.record_decision(ctx, decision)
    return PhaseOutcome("await", decision.reason, gate=BUDGET_WAIT_GATE)


def _block_on_error(ctx: FlowContext, error: Exception) -> None:
    """A driver that dies on an unexpected error leaves the lane blocked and naming it, never `running` with nothing behind
    it: Stop takes, and Resume is admitted (a retry may clear it). A failure to write that is not allowed to hide the error."""
    reason = f"the engine stopped on an error: {type(error).__name__}: {strip_controls(str(error))[:300]}"
    try:
        _set_status(ctx, STATUS_BLOCKED, reason)
        ctx.store.append_ledger("engine", reason)
    except Exception:  # noqa: BLE001 (the original error is re-raised by the caller)
        pass


def _block_on_tamper(ctx: FlowContext) -> int:
    """Restore the last state the engine wrote, marked blocked; forged content is discarded."""
    trusted = ctx.store.last_written()
    if trusted is not None:
        trusted.update(status="blocked", reason=TAMPERED_REASON)
        ctx.store.write_state(trusted)
    ctx.store.append_ledger("engine", TAMPERED_REASON)
    ctx.notifier.notify(notify.ESCALATION_EVENT, ctx.store.task_id, TAMPERED_REASON)
    return exits.BLOCKED


def _spec_changed_after_approval(ctx: FlowContext) -> bool:
    state = ctx.store.read_state()
    approved = state["approvals"].get(SPEC_GATE, {}).get("sha256")
    if not approved or not ctx.store.spec.exists():
        return False
    if hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest() == approved:
        return False

    def reopen(current: dict) -> None:
        current["approvals"].pop(SPEC_GATE, None)
        current["completed"].pop("intake", None)
        current["phase"] = "intake"

    ctx.store.update_state(reopen)
    ctx.store.append_ledger("intake", "spec changed after approval")
    return True


def _run_phase(ctx: FlowContext, phase: str) -> PhaseOutcome:
    stacks.sweep(ctx)
    handler = _HANDLERS.get(phase)
    if handler is None:
        return PhaseOutcome("blocked", f"phase {phase} not available")
    if _already_completed(ctx, phase):
        return PhaseOutcome("next", "inputs unchanged")
    try:
        try:
            outcome = handler(ctx)
        except roles.RoleRefused as refusal:
            return PhaseOutcome("blocked", str(refusal))
        except budget_guard.BudgetWait as wait:
            return _budget_outcome(ctx, wait.decision)
    finally:
        stacks.sweep(ctx)
    if outcome.kind == "next":
        _record_completed(ctx, phase)
    return outcome


def _already_completed(ctx: FlowContext, phase: str) -> bool:
    inputs = _INPUTS.get(phase)
    recorded = ctx.store.read_state()["completed"].get(phase)
    return inputs is not None and recorded is not None and recorded == inputs(ctx)


def _record_completed(ctx: FlowContext, phase: str) -> None:
    inputs = _INPUTS.get(phase)
    if inputs is not None:
        digest = inputs(ctx)
        ctx.store.update_state(lambda state: state["completed"].update({phase: digest}))


def _complete(ctx: FlowContext, phase: str, outcome: PhaseOutcome) -> None:
    status = "skipped" if outcome.kind == "skip" else "done"
    following = PHASE_ORDER[min(PHASE_ORDER.index(phase) + 1, len(PHASE_ORDER) - 1)]

    def change(state: dict) -> None:
        _append_history(state, phase, status, outcome.reason)
        if phase != PHASE_ORDER[-1]:
            state["phase"] = following

    ctx.store.update_state(change)
    ctx.store.append_ledger(phase, f"{status}" + (f": {outcome.reason}" if outcome.reason else ""))


def _stop_at(ctx: FlowContext, phase: str, outcome: PhaseOutcome) -> int:
    status = STATUS_BY_KIND[outcome.kind]
    reason = outcome.reason
    if outcome.kind == "await":
        reason = f"awaiting approval: sakura flow approve {ctx.store.task_id} {outcome.gate}"
        details = [detail for detail in (outcome.reason, _read_first(ctx, outcome.gate)) if detail]
        reason += f" ({'; '.join(details)})" if details else ""

    fresh: list[bool] = []

    def change(state: dict) -> None:
        state.update(status=status, reason=reason)
        fresh.append(_append_history(state, phase, status, reason))

    ctx.store.update_state(change)
    if fresh[0]:
        ctx.notifier.notify(_event_of(outcome, status), ctx.store.task_id, reason)
    return EXIT_BY_KIND[outcome.kind]


def _read_first(ctx: FlowContext, gate: str | None) -> str:
    """The file the person should read before approving `gate`, as a clause for the waiting line."""
    store = ctx.store
    targets = {SPEC_GATE: store.spec, checks.APPROVAL_GATE: store.contract, BUDGET_GATE: store.plan_md,
               BUDGET_WAIT_GATE: store.ledger}
    target = store.plan_md if gate and gate.startswith(OPUS_GATE_PREFIX) else targets.get(gate)
    return f"read {target} first" if target else ""


def _event_of(outcome: PhaseOutcome, status: str) -> str:
    """Every block is an escalation unless the phase named a more specific event."""
    if outcome.event:
        return outcome.event
    return notify.ESCALATION_EVENT if outcome.kind == "blocked" else status


def _set_status(ctx: FlowContext, status: str, reason: str) -> None:
    ctx.store.update_state(lambda state: state.update(status=status, reason=reason))


def _append_history(state: dict, phase: str, status: str, detail: str) -> bool:
    """Append a history entry; returns whether it was added.

    Only a repeated `awaiting_approval` wait at the same gate is dropped; blocked
    and failed entries are always recorded so every escalation is announced.
    """
    last = state["history"][-1] if state["history"] else None
    repeated_wait = status == "awaiting_approval" and last and (last["phase"], last["status"], last["detail"]) == (phase, status, detail)
    if repeated_wait:
        return False
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    state["history"].append({"phase": phase, "status": status, "at": at, "detail": detail})
    return True
