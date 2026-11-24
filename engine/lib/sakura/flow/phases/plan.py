"""Plan phase: the planner writes the build plan and a second model reviews it."""

from __future__ import annotations

import hashlib
from datetime import datetime, timezone
from pathlib import Path

from sakura.flow import ask_words, budget_gate, contract, pipeline, plan, runner
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git
from sakura.flow.notify import ESCALATION_EVENT
from sakura.flow.phases import runs

MAX_PLAN_ATTEMPTS = 2
WORKSPACE_NAME = "0"
ROUND_KEY = "plan_review"
FINDINGS_KEY = "plan_review_findings"


def run(ctx: FlowContext) -> PhaseOutcome:
    """Produce a validated plan and, when the preset reviews plans, an approved one; bind it in state."""
    workspace = ctx.store.read_state()["workspaces"].get(WORKSPACE_NAME)
    if not workspace:
        return PhaseOutcome("blocked", f"workspace {WORKSPACE_NAME} is missing; the contract phase must run first")
    cwd = Path(workspace["path"])
    bound = _bound_digest(ctx)
    if bound:
        return budget_gate.check(ctx, bound, inputs_hash(ctx))
    ctx.store.update_state(lambda state: state["rounds"].update({ROUND_KEY: 0}))
    try:
        return _plan_and_review(ctx, cwd, list(ctx.store.read_state().get(FINDINGS_KEY, [])))
    except GitError as error:
        return PhaseOutcome("blocked", str(error))


def inputs_hash(ctx: FlowContext) -> str:
    """Digest of the approved spec, the saved contract and the base commit."""
    state = ctx.store.read_state()
    spec = state["approvals"].get("spec", {}).get("sha256") or hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    return hashlib.sha256(f"{spec}\0{state['contract_sha256']}\0{state['base_commit']}".encode()).hexdigest()


def _bound_digest(ctx: FlowContext) -> str:
    """The digest of a plan that waits at the budget gate for the inputs the phase still has; else empty.

    A plan phase that comes back after the gate (approval, resume) must not plan again, but a changed spec,
    contract or base commit must.
    """
    state = ctx.store.read_state()
    bound, saved = state.get("plan_sha256", ""), (state.get("budget") or {}).get("preflight") or {}
    made_for_these = saved.get("plan_sha256") == bound and saved.get("inputs_sha256") == inputs_hash(ctx)
    files = ctx.store.plan_json.is_file() and ctx.store.plan_md.is_file()
    unchanged = bool(bound) and made_for_these and files and plan.digest(ctx.store) == bound
    return bound if unchanged and not _plan_errors(ctx) else ""


def _plan_and_review(ctx: FlowContext, cwd: Path, findings: list[str]) -> PhaseOutcome:
    if not pipeline.of_values(ctx.config.values).step("plan").on:
        return _goal_plan(ctx)
    while True:
        failure = _write_plan(ctx, cwd, findings)
        if failure:
            return failure
        if not ctx.steps.plan_review:
            _record_skip(ctx)
            return _bind(ctx, plan.digest(ctx.store))
        before = plan.digest(ctx.store)
        verdict, findings, failure = _review(ctx, cwd)
        if failure:
            return failure
        if verdict == "APPROVE":
            return _bind(ctx, before)
        escalation = _count_round(ctx, findings)
        if escalation:
            return escalation


def _goal_plan(ctx: FlowContext) -> PhaseOutcome:
    """The plan step is off: one task builds the goal, no planner runs and nothing is reviewed."""
    state = ctx.store.read_state()
    rows = contract.load(ctx.store, state["contract_sha256"])
    plan.write_goal_plan(ctx.store, rows, ctx.store.spec.read_text())
    ctx.store.append_ledger("plan", "skip: the plan step is off, one task builds the goal")
    _record_skip(ctx)
    return _bind(ctx, plan.digest(ctx.store))


def _bind(ctx: FlowContext, reviewed_digest: str) -> PhaseOutcome:
    """Re-validate the plan, require it to be the reviewed one, and store its digest."""
    if plan.digest(ctx.store) != reviewed_digest:
        return PhaseOutcome("blocked", "plan files changed during plan review")
    errors = _plan_errors(ctx)
    if errors:
        return PhaseOutcome("blocked", "plan invalid at approval: " + "; ".join(errors))
    ctx.store.update_state(lambda state: state.update(plan_sha256=reviewed_digest, **{FINDINGS_KEY: []}))
    return budget_gate.check(ctx, reviewed_digest, inputs_hash(ctx))


def _write_plan(ctx: FlowContext, cwd: Path, findings: list[str]) -> PhaseOutcome | None:
    error, said = "", ""
    for _ in range(MAX_PLAN_ATTEMPTS):
        failure, said = _run_planner(ctx, cwd, findings, error)
        if failure:
            return failure
        error = "; ".join(_plan_errors(ctx))
        if not error:
            return None
    if not (ctx.store.plan_md.is_file() and ctx.store.plan_json.is_file()):
        ctx.store.append_ledger("plan", f"plan invalid after {MAX_PLAN_ATTEMPTS} attempts: {error}")
        return PhaseOutcome("blocked", ask_words.nothing_written("plan", said))
    return PhaseOutcome("blocked", f"plan invalid after {MAX_PLAN_ATTEMPTS} attempts: {error}")


def _plan_errors(ctx: FlowContext) -> list[str]:
    if not ctx.store.plan_md.is_file() or not ctx.store.plan_md.read_text().strip():
        return ["plan.md was not written"]
    try:
        tasks = plan.load(ctx.store, None)
        rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    except (plan.PlanError, contract.ContractError) as error:
        return [str(error)]
    return plan.validate(tasks, rows)


def _run_planner(ctx: FlowContext, cwd: Path, findings: list[str], error: str) -> tuple[PhaseOutcome | None, str]:
    """Run the planner; its failure, if any, and what it said at the end."""
    values = {**_paths(ctx, cwd), "review_findings": _findings_text(findings),
              "previous_error": f"\nYour previous attempt was rejected: {error}. Fix that and write the files again." if error else ""}
    request = runner.RunRequest("planner", "plan", runner.load_template("plan"), values, cwd, {})
    return _guarded_run(ctx, request, with_final=True)


def _review(ctx: FlowContext, cwd: Path) -> tuple[str, list[str], PhaseOutcome | None]:
    request = runner.RunRequest("plan_reviewer", "plan_review", runner.load_template("plan_review"), _paths(ctx, cwd), cwd, {})
    outcome, result = _guarded_run(ctx, request, with_result=True)
    if outcome:
        return "", [], outcome
    verdict, findings = runs.parse_verdict(result)
    return verdict, findings, None


def _guarded_run(ctx: FlowContext, request: runner.RunRequest, with_result: bool = False, with_final: bool = False):
    """Run an agent in workspace 0; blocks when it failed or left the workspace changed. With `with_result` the outcome comes
    with the result the agent returned, with `with_final` with its last message."""
    head = git(request.cwd, "rev-parse", "HEAD")
    result = ctx.runner.run(request)
    failure = runs.failure_outcome(result, request.role, request.phase) or _workspace_changes(request, head)
    if with_final:
        return failure, result.final
    return (failure, result.result) if with_result else failure


def _workspace_changes(request: runner.RunRequest, head: str) -> PhaseOutcome | None:
    return runs.workspace_changes(request.cwd, request.role, request.phase, head)


def _count_round(ctx: FlowContext, findings: list[str]) -> PhaseOutcome | None:
    """Record one REVISE round; returns the blocking outcome once the dial's fix rounds are spent."""
    def change(state: dict) -> None:
        state["rounds"][ROUND_KEY] = state["rounds"].get(ROUND_KEY, 0) + 1
        state[FINDINGS_KEY] = findings

    rounds = ctx.store.update_state(change)["rounds"][ROUND_KEY]
    limit = ctx.config.values["knobs"]["fix_rounds"]
    ctx.store.append_ledger("plan_review", f"round {rounds} REVISE", agent_values=findings)
    if rounds <= limit:
        return None
    reason = f"plan review still asked for changes after {limit} fix rounds ({rounds} reviews): " + "; ".join(findings)
    return PhaseOutcome("blocked", reason, event=ESCALATION_EVENT)


def _record_skip(ctx: FlowContext) -> None:
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    entry = {"phase": "plan_review", "status": "skip", "at": at, "detail": "plan review is off for this preset or role setup"}

    def change(state: dict) -> None:
        if not any(h["phase"] == "plan_review" and h["status"] == "skip" for h in state["history"]):
            state["history"].append(entry)

    ctx.store.update_state(change)
    ctx.store.append_ledger("plan_review", "skip: plan review is off")


def _paths(ctx: FlowContext, cwd: Path) -> dict[str, str]:
    store = ctx.store
    return {"spec_path": str(store.spec), "contract_path": str(store.contract), "repo_path": str(cwd),
            "plan_md_path": str(store.plan_md), "plan_json_path": str(store.plan_json)}


def _findings_text(findings: list[str]) -> str:
    if not findings:
        return ""
    return "\nThe plan reviewer asked for these changes; address every one:\n" + "\n".join(f"- {item}" for item in findings)
