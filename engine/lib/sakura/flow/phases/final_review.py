"""The final review: one reviewer reads the whole branch after the last build task, before the checks.

It runs at the end of the build phase while the preset has `final_review` on. The reviewer reads the engine's head
against the base. On APPROVE the lane goes on. On a REVISE (or REJECT) a builder fixes the findings on the engine's
head, the full tests run again and the branch is reviewed again, at most `final_review_rounds` times; after that the
lane asks the person at the `final-review` gate. Everything is kept in `state["final_review"]`, so a second run of the
phase neither reviews a head again nor asks twice.
"""

from __future__ import annotations

from pathlib import Path

from sakura.flow import ask_words, engine, final_head, runner
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import git
from sakura.flow.phases import integrate, runs

GATE = engine.FINAL_REVIEW_GATE
STATE_KEY = "final_review"
LEDGER_TAG = "review"
STEP = "final_review"
FIX_TASK_PREFIX = "final-"
ZERO_WORKSPACE = "0"
BUILD_TASK_ENV = "SAKURA_FLOW_BUILD_TASK"


def run(ctx: FlowContext) -> PhaseOutcome | None:
    """Review the whole branch; None when it is approved (or the preset has no final review), else the outcome.

    The outcome is `await` at the `final-review` gate once the review still asks for changes after the configured
    rounds, `blocked` when a role fails or a fix breaks a rule, `stopped` when the lane is stopped.
    """
    if not engine.current_steps(ctx).final_review:
        return None
    limit = ctx.config.values["knobs"]["final_review_rounds"]
    while True:
        head = final_head.recorded(ctx.store.read_state())
        if _is_settled(ctx, head):
            return None
        verdict, findings, failure = _review(ctx, head)
        if failure:
            return failure
        if verdict == runs.APPROVE:
            _remember(ctx, approved_head=head, stuck=False)
            return None
        round_number = _count_round(ctx, findings)
        if round_number > limit:
            return _ask_the_person(ctx, head, round_number - 1, findings)
        failure = _send_back(ctx, findings, round_number)
        if failure:
            return failure


def _is_settled(ctx: FlowContext, head: str) -> bool:
    """Whether this head was approved by the reviewer, or by the person at the gate when the lane asked about it."""
    state = ctx.store.read_state()
    found = state.get(STATE_KEY) or {}
    if found.get("approved_head") == head:
        return True
    if found.get("asked_head") == head and GATE in state.get("approvals", {}):
        ctx.store.append_ledger(LEDGER_TAG, "final review: the person accepted the branch as it is")
        _remember(ctx, approved_head=head, stuck=False)
        return True
    return False


def _review(ctx: FlowContext, head: str) -> tuple[str, list[str], PhaseOutcome | None]:
    """One review of `base..head` in a sealed copy of its own; returns the verdict, the findings and any failure."""
    base = ctx.store.read_state()["base_commit"]
    with final_head.review_copy(ctx, "review-final", base, head) as (repository, copy):
        request = runner.RunRequest("code_reviewer", "review", runner.load_template("final_review"),
                                    _review_values(ctx, copy, base, head), copy, repository=repository,
                                    step=STEP)
        result = ctx.runner.run(request)
        failure = runs.failure_outcome(result, "code_reviewer", "review") or runs.workspace_changes(
            copy, "code_reviewer", "review", head)
    if failure:
        return "", [], failure
    verdict, findings = runs.parse_verdict(result.result)
    return verdict, findings, None


def _review_values(ctx: FlowContext, copy: Path, base: str, head: str) -> dict[str, str]:
    store = ctx.store
    return {"spec_path": str(store.spec), "contract_path": str(store.contract), "plan_md_path": str(store.plan_md),
            "repo_path": str(copy), "start": base, "branch": head,
            "flows": final_head.flows_review_text(store.read_state())}


def _count_round(ctx: FlowContext, findings: list[str]) -> int:
    """Record one review that asked for changes; returns how many there have been."""
    rounds = ctx.store.update_state(lambda state: state.setdefault(STATE_KEY, {}).update(
        rounds=(state.get(STATE_KEY) or {}).get("rounds", 0) + 1))[STATE_KEY]["rounds"]
    ctx.store.append_ledger(LEDGER_TAG, f"final review round {rounds}: REVISE", agent_values=findings)
    return rounds


def _ask_the_person(ctx: FlowContext, head: str, spent: int, findings: list[str]) -> PhaseOutcome:
    """Ask at the gate; the ask keeps how many fix rounds were spent and the findings of the review that still wanted changes."""
    _remember(ctx, asked_head=head, asked_rounds=spent, findings=findings)
    ctx.store.update_state(lambda state: state["approvals"].pop(GATE, None))
    ctx.store.append_ledger(LEDGER_TAG, f"final review still asks for changes after {_rounds(spent)}: the person decides")
    reason = f"the final review still asked for changes after {_rounds(spent)}; read the findings in the ledger"
    return PhaseOutcome("await", reason, gate=GATE)


def _rounds(count: int) -> str:
    return f"{count} round{'s' if count != 1 else ''}"


def _remember(ctx: FlowContext, **fields: str) -> None:
    ctx.store.update_state(lambda state: state.setdefault(STATE_KEY, {}).update(fields))


def _send_back(ctx: FlowContext, findings: list[str], round_number: int) -> PhaseOutcome | None:
    """One build round on the engine's head for the findings; its commit becomes the head and the full tests run on it."""
    ctx.store.append_ledger(LEDGER_TAG, f"final review sent back to build: round {round_number}")
    _remember(ctx, findings=findings, stuck=False)
    state = ctx.store.read_state()
    zero = Path(state["workspaces"][ZERO_WORKSPACE]["path"])
    final_head.show(ctx, zero, state["workspaces"][ZERO_WORKSPACE]["branch"])
    start = git(zero, "rev-parse", "HEAD").strip()
    task_id = f"{FIX_TASK_PREFIX}{round_number}"
    failure, said = _run_builder(ctx, zero, task_id, findings)
    if failure:
        return failure
    head = git(zero, "rev-parse", "HEAD").strip()
    if head == start:
        _remember(ctx, stuck=True)  # the ask shows the findings the builder did not act on
        ctx.store.append_ledger(LEDGER_TAG, f"{task_id}: builder made no commit")
        return PhaseOutcome("blocked", ask_words.fix_without_commit(said))
    try:
        integrate.check_protected(ctx, f"build task {task_id}", start, head)
        final_head.fix(ctx, task_id, head)
    except final_head.MergeBlocked as error:
        return PhaseOutcome("blocked", str(error))
    return integrate.run_full_tests(ctx)


def _run_builder(ctx: FlowContext, zero: Path, task_id: str, findings: list[str]) -> tuple[PhaseOutcome | None, str]:
    """Run the final fix builder; its failure, if any, and what it said at the end."""
    store, ports = ctx.store, ctx.store.read_state()["workspaces"][ZERO_WORKSPACE]["ports"]
    values = {"spec_path": str(store.spec), "contract_path": str(store.contract), "plan_md_path": str(store.plan_md),
              "task_id": task_id, "findings": "\n".join(f"- {item}" for item in findings),
              "port": str(ports[0]), "ports": f"{ports[0]}-{ports[-1]}"}
    request = runner.RunRequest("builder", "build", runner.load_template("final_fix"), values, zero,
                                {BUILD_TASK_ENV: task_id})
    result = ctx.runner.run(request)
    return runs.failure_outcome(result, "builder", "build"), result.final
