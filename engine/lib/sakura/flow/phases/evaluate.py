"""Evaluator phase: calibrate, gate and record evidence-backed contract verdicts."""

from __future__ import annotations

import contextlib
import json
import hashlib
import os
import shutil
import sys
import time
from pathlib import Path
from typing import Iterator

from sakura import beneath, slots, testing
from sakura.flow import (baseline, calibration, checks, completion, contract, final_head, flow_paths, flowfiles,
                         globs, owner_marks, plan, process, readonly_checkout, runner, stack, stacks, workspaces)
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow import gitutil
from sakura.flow.evidence import ROLE_EVIDENCE_DIR
from sakura.flow.gitutil import GitError, git
from sakura.flow.phases import build_task, integrate, runs
from sakura.flow.store import TaskStore

PHASE = "evaluate"
CALIBRATE = "calibrate"
ZERO_WORKSPACE = "0"
BASE_WORKSPACE = "base"
# Where the engine's own run of every calibration row logs, beside the evaluator's (`base/evaluator`).
CALIBRATION_DIR = f"{BASE_WORKSPACE}/engine"
EVALUATOR = "evaluator"
FIX_TASK_PREFIX = "fix-"
CHECKS_DIR = "checks"
EVALUATOR_DIR = "evaluator"
FILE_MODE = 0o644
EXECUTABLE_MODE = 0o755


def run(ctx: FlowContext) -> PhaseOutcome:
    """Prove the engine's head (`flow/final_head.py`): the engine runs the `check` rows, the evaluator proves its rows,
    with bounded fixes. No branch or workspace says what is proved: a commit a survivor adds to the task branch is not.

    `owner` rows are left to the person (`sakura flow mark`); delivery waits for them. A new head or other new
    inputs clear every pass and mark first, before any early return, whatever the preset.
    """
    record = ctx.store.read_state().get("workspaces", {}).get(ZERO_WORKSPACE)
    try:
        if record:
            _ensure_current_inputs(ctx)
            problem = _test_only_problem(ctx, final_head.recorded(ctx.store.read_state()))
            if problem:
                return PhaseOutcome("blocked", problem)
        if not ctx.config.values["knobs"]["checks"]:
            return _checks_off(ctx)
        if not ctx.steps.evaluate:
            return _checks_only(ctx, record is not None)
        return _run(ctx)
    except (contract.ContractError, GitError, workspaces.WorkspaceError, stack.StackError,
            completion.CompletionRefused, calibration.CalibrationRefused) as error:
        return PhaseOutcome("blocked", f"{error}; no receipt was written" if isinstance(
            error, completion.CompletionRefused) else str(error))
    except baseline.FlowInterrupted as interrupted:
        return PhaseOutcome("stopped" if interrupted.stopped else "blocked", str(interrupted))
    except EvaluationFailure as failure:
        return failure.outcome


def _checks_off(ctx: FlowContext) -> PhaseOutcome:
    """The checks step is switched off: no evaluator and no saved check command runs."""
    ctx.store.append_ledger(PHASE, "not verified by an evaluator")
    return PhaseOutcome("skip", "checks are switched off")


def _checks_only(ctx: FlowContext, built: bool) -> PhaseOutcome:
    """Without an evaluator the engine still runs the check rows; one that fails waits for the person."""
    ctx.store.append_ledger(PHASE, "not verified by an evaluator")
    rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    if not built or not contract.proven_by(rows, ctx.steps, contract.CHECK):
        return PhaseOutcome("skip")
    _ensure_current_inputs(ctx)
    with completion.opened(ctx) as checkout:
        run_checks(ctx, checkout)
    failing = [row for row in _unpassed(ctx) if contract.prover_of(row, ctx.steps) == contract.CHECK]
    if failing:
        return PhaseOutcome("blocked", _failing_checks_reason(ctx, failing))
    return PhaseOutcome("skip", "check rows passed")


def _failing_checks_reason(ctx: FlowContext, failing: list[contract.Row]) -> str:
    """The blocked line of failing check rows: where each log is and the commands the person has.

    This preset has no fix step and the saved contract cannot be edited after the plan, so the person can only
    run the checks again once what they need is fixed, or stop the task and start it over: a new task writes a new
    contract (a row can be the faulty part, not the code).
    """
    task = ctx.store.task_id
    logs = ", ".join(f"{row.id} ({ctx.store.evidence_dir / CHECKS_DIR / f'{row.id}-{row.viewport or baseline.API_LABEL}.log'})"
                     for row in failing)
    return (f"check rows fail on the task branch: {logs}. Nothing in this flow repairs a check row and the contract "
            f"cannot be edited after the plan. Fix what the rows need (a service, a tool) and run "
            f"`sakura flow resume {task}` to check again, or run `sakura flow stop {task}` and start the task over with "
            "`sakura flow start --new <file or text>`: a new task writes a new contract, which is the way out when "
            "the row itself is wrong (read its log first).")


def run_checks(ctx: FlowContext, checkout: completion.Checkout) -> None:
    """The engine runs every unpassed `check` row inside the checks cage and writes a receipt.

    The rows, and the application they exercise, run from the completion checkout of the engine's head
    (`flow/completion.py`) on read-only source; the receipt is bound to that revision, and no receipt is written when
    the checkout no longer holds exactly that revision after the run or the revision is not the engine's head. Only a
    receipt with exit 0 flips a row; its log is the evidence. Commands must be approved (see `flow/checks.py`).
    """
    state = ctx.store.read_state()
    every = contract.load(ctx.store, state["contract_sha256"])
    rows = [row for row in _unpassed(ctx) if contract.prover_of(row, ctx.steps) == contract.CHECK]
    if not rows:
        return
    try:
        listed = ctx.config.values["checks"]
        checks.require_approved(state, every, ctx.steps, listed["approved"], listed["remembered"])
    except checks.ChecksNeedApproval as waiting:
        # Checks turned on during a lane, or commands that changed: the person is asked, as at the contract (`Run these
        # N checks?`), and approving lets this step run them.
        summary = checks.wait_for_approval(ctx.store, waiting, every, ctx.steps)
        ctx.store.append_ledger("evaluate", f"{summary} wait for the person's approval")
        raise EvaluationFailure(PhaseOutcome("await", f"{summary} to approve", gate=checks.APPROVAL_GATE)) from waiting
    workspace = _workspace(ctx)
    with stack.start(ctx, workspace, checkout) as app:
        codes = baseline.run_rows(ctx, checkout.source, app.base_url, app.port, rows, CHECKS_DIR,
                                  build_dir=checkout.build)
    completion.require_unchanged(checkout)
    require_the_engines_head(ctx, checkout.revision)
    for row in rows:
        evidence = ctx.store.evidence_dir / CHECKS_DIR / f"{row.id}-{row.viewport or baseline.API_LABEL}.log"
        checks.record_receipt(ctx.store, row, contract.rows_digest(every), checkout.revision, codes[row.id], evidence)
        if codes[row.id] == 0:
            contract.mark_passed(ctx.store, ctx.store.read_state(), row.id, row.viewport, evidence)
        note = " (the checkout is read-only: write under $TMPDIR)" if _denied_a_write(evidence) else ""
        ctx.store.append_ledger(PHASE, f"check {row.id}: exit {codes[row.id]}{note} (receipt at {checkout.revision[:12]})")


def require_the_engines_head(ctx: FlowContext, revision: str) -> None:
    """Raise CompletionRefused unless `revision` is the engine's head, checked against its store: no other revision
    gets a receipt (review 11, finding 1)."""
    try:
        head = final_head.proved(ctx)[1]
    except GitError as error:
        raise completion.CompletionRefused(str(error)) from error
    if revision != head:
        raise completion.CompletionRefused(f"{revision[:12]} is not the engine's head {head[:12]}")


def _denied_a_write(log: Path) -> bool:
    """Whether a row's log carries the closing line `baseline` adds when the read-only checkout refused a write."""
    return readonly_checkout.HINT in log.read_text(errors="replace")


def inputs_hash(ctx: FlowContext) -> str:
    """Bind evaluation completion to its code, evaluator and runtime configuration."""
    state = ctx.store.read_state()
    head = final_head.recorded(state) if state.get("workspaces", {}).get(ZERO_WORKSPACE, {}).get("path") else ""
    values = ctx.config.values
    rows = contract.load(ctx.store, state["contract_sha256"])
    contract_inputs = [{**row.__dict__, "passes": False} for row in rows]
    inputs = {"contract": contract_inputs, "head": head,
              "evaluator": values["roles"][EVALUATOR], "evaluate": ctx.steps.evaluate,
              "familiarity": state.get("familiarity"), "dial": state.get("dial"),
              "viewports": values["tests"]["viewports"], "app": values["app"],
              "timeouts": {key: values["timeouts"][key] for key in ("calibrate", "evaluate", "idle_seconds")},
              "load": {key: values["load"][key] for key in ("slots", "idle_stack_seconds")},
              "test_patterns": values["brakes"]["test_patterns"],
              "test_only_markers": values["brakes"]["test_only_markers"]}
    return hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()


def _run(ctx: FlowContext) -> PhaseOutcome:
    state = ctx.store.read_state()
    rows = contract.load(ctx.store, state["contract_sha256"])
    record = state["workspaces"].get(ZERO_WORKSPACE)
    if not record:
        return PhaseOutcome("blocked", "workspace 0 is missing; the build phase must run first")
    zero = Path(record["path"])
    _ensure_current_inputs(ctx)
    state = ctx.store.read_state()
    rows = contract.load(ctx.store, state["contract_sha256"])
    problem = _test_only_problem(ctx, final_head.recorded(state))
    if problem:
        return PhaseOutcome("blocked", problem)
    if state.get("evaluation_fix", {}).get("pending"):
        return _finish_fixes(ctx, zero)
    role = ctx.config.values["roles"][EVALUATOR]
    rows = contract.proven_by(rows, ctx.steps, contract.EVALUATOR)
    if rows and (not calibration.is_current(ctx.repo, role) or state.get("familiarity") == "new"):
        failure = _calibrate(ctx, rows, EVALUATOR, role)
        if failure:
            return failure
    failure = _evaluate(ctx, zero)
    if failure:
        return failure
    return _finish_fixes(ctx, zero)


def _finish_fixes(ctx: FlowContext, zero: Path) -> PhaseOutcome:
    """Apply the configured number of fixes, testing and evaluating after each one."""
    limit = ctx.config.values["knobs"]["fix_rounds"]
    for _ in range(limit):
        progress = ctx.store.read_state().get("evaluation_fix", {})
        pending = progress.get("pending")
        attempts = progress.get("attempts", 0)
        if pending and attempts >= limit:
            number = pending["round"]
            failure = _resume_fix_candidate(ctx, zero, number, pending)
        else:
            number = attempts + 1
            if number > limit:
                break
            remaining = _unpassed(ctx)
            if not remaining:
                _clear_fix_progress(ctx)
                return PhaseOutcome("next")
            failure = _run_fix(ctx, zero, number, remaining)
        if failure:
            return failure
        # The full tests decide whether the reviewed candidate becomes the engine's head; then it is evaluated there.
        pending = (ctx.store.read_state().get("evaluation_fix") or {}).get("pending") or {}
        with completion.opened(ctx, pending.get("head", "")) as checkout:
            failure = integrate.run_full_tests(ctx, checkout)
            if failure:
                return failure
            _accept_fix_candidate(ctx)
            failure = _evaluate(ctx, zero, checkout)
        if failure:
            return failure
    remaining = _unpassed(ctx)
    if remaining:
        ids = ", ".join(f"{row.id}/{row.viewport or 'api'}" for row in remaining)
        return PhaseOutcome("blocked", f"fix rounds exhausted; rows still need work: {ids}", event="escalation")
    return PhaseOutcome("next")


def _calibrate(ctx: FlowContext, rows: list[contract.Row], role: str, role_spec: str) -> PhaseOutcome | None:
    """Calibrate the evaluator on the base build. The application and every row run from the engine's read-only
    checkout of its base build (`completion.base_build`), checked unchanged afterwards; the base workspace, with the
    engine's copies of the head's test files, is the evaluator's view only (review 12). The engine runs every row
    there itself and keeps a receipt of each result; the evaluator's verdicts are judged against those receipts, and
    calibration is recorded only with a receipt of every row (review 13, finding 2)."""
    state = ctx.store.read_state()
    base = workspaces.create(ctx, BASE_WORKSPACE, f"{state['branch']}-base", state["base_commit"])
    prepared_inputs = _sync_contract_flows(ctx, base.path)
    digest = contract.rows_digest(contract.load(ctx.store, state["contract_sha256"]))
    ctx.store.update_state(lambda current: current.pop(checks.CALIBRATION_RECEIPTS_KEY, None))
    with completion.base_build(ctx) as checkout:
        with stack.start(ctx, base, checkout) as app:
            _run_calibration_rows(ctx, checkout, (app, base), rows, digest)
            started_ns = time.time_ns()
            result = _agent(ctx, role, CALIBRATE, base, rows, app.base_url, "evidence/base", prepared_inputs)
        completion.require_unchanged(checkout)
    verdicts = _verdict_rows(result)
    receipts = ctx.store.read_state().get(checks.CALIBRATION_RECEIPTS_KEY, {})
    errors = calibration.judge(rows, verdicts, receipts, checkout.revision, digest)
    errors.extend(_validate_evidence(ctx, rows, verdicts, started_ns, require_pass=False))
    if errors:
        return PhaseOutcome("blocked", "evaluator miscalibrated: " + "; ".join(errors))
    calibration.record(ctx.repo, role_spec, ctx.store.task_id, rows, receipts, checkout.revision, digest)
    return None


def _run_calibration_rows(ctx: FlowContext, checkout: completion.Checkout,
                          app: tuple[stack.AppStack, workspaces.Workspace], rows: list[contract.Row],
                          digest: str) -> None:
    """Run every calibration row on the checkout of the base build, one at a time, and keep the engine's receipt of
    each result, a failing one too, once the checkout still holds exactly its revision."""
    running, workspace = app
    codes = {}
    for row in rows:
        codes[checks.row_key(row)] = baseline.run_rows(
            ctx, checkout.source, running.base_url, running.port, [row], CALIBRATION_DIR,
            f"{ctx.store.task_id}:{workspace.name}", build_dir=checkout.build)[row.id]
    completion.require_unchanged(checkout)
    for row in rows:
        code = codes[checks.row_key(row)]
        log = ctx.store.evidence_dir / CALIBRATION_DIR / f"{row.id}-{row.viewport or baseline.API_LABEL}.log"
        checks.record_receipt(ctx.store, row, digest, checkout.revision, code, log, checks.CALIBRATION_RECEIPTS_KEY)
        ctx.store.append_ledger(CALIBRATE, f"calibration {row.id}: exit {code} (receipt at {checkout.revision[:12]})")


def _evaluate(ctx: FlowContext, zero: Path, checkout: completion.Checkout | None = None) -> PhaseOutcome | None:
    """Run the check rows, then the evaluator on its rows, all against one completion checkout of the engine's head.
    The evaluator works in workspace 0, moved to that head first: its view, while what is proved is the checkout."""
    _ensure_current_inputs(ctx)
    if not _unpassed(ctx):
        return None
    with contextlib.nullcontext(checkout) if checkout else completion.opened(ctx) as current:
        run_checks(ctx, current)
        rows = _evaluator_rows(ctx)
        if not rows:
            return None
        _show_head(ctx, zero)
        workspace = _workspace(ctx)
        with stack.start(ctx, workspace, current) as app:
            started_ns = time.time_ns()
            result = _agent(ctx, EVALUATOR, PHASE, workspace, rows, app.base_url, "evidence")
        _store_passes(ctx, rows, _verdict_rows(result), started_ns, current)
    return None


def _show_head(ctx: FlowContext, zero: Path) -> None:
    """Move workspace 0 to the engine's head before a role works there; EvaluationFailure when Git refuses."""
    try:
        final_head.show(ctx, zero, ctx.store.read_state()["workspaces"][ZERO_WORKSPACE]["branch"])
    except GitError as error:
        raise EvaluationFailure(PhaseOutcome("blocked", f"workspace 0 cannot be moved to the engine's head: {error}"))


def _ensure_current_inputs(ctx: FlowContext) -> None:
    state = ctx.store.read_state()
    head = final_head.recorded(state)
    current_inputs = inputs_hash(ctx)
    if state.get("evaluation_inputs_sha256") == current_inputs:
        return
    contract.reset_passes(ctx.store, state)
    owner_marks.forget(ctx.store)
    _clear_current_evidence(ctx)
    ctx.store.update_state(lambda current: current.update(evaluation_head=head,
                                                          evaluation_inputs_sha256=current_inputs))


def _clear_current_evidence(ctx: FlowContext) -> None:
    root = ctx.store.evidence_dir
    if not root.exists():
        return
    if root.is_symlink():
        raise contract.ContractError("evidence directory cannot be a symlink")
    for path in root.iterdir():
        if path.name == "base":
            continue
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)


def _agent(ctx: FlowContext, role: str, phase: str, workspace: workspaces.Workspace, rows: list[contract.Row],
           base_url: str, evidence_dir: str,
           prepared_inputs: dict[str, tuple[bytes, bool]] | None = None) -> dict | None:
    values = {"task_id": ctx.store.task_id, "repo_path": str(workspace.path), "rows": _rows_json(rows),
              "viewports": ", ".join(ctx.config.values["tests"]["viewports"]), "base_url": base_url,
              "evidence_dir": str(ctx.store.root / evidence_dir / ROLE_EVIDENCE_DIR)}
    request = runner.RunRequest(role, phase, runner.load_template(PHASE), values, workspace.path,
                                {"SAKURA_FLOW_BASE_URL": base_url,
                                 "SAKURA_FLOW_STACK": f"{ctx.store.task_id}:{workspace.name}"})
    changes = (_prepared_workspace_changes(workspace.path, prepared_inputs) if prepared_inputs is not None
               else flowfiles.tracked_changes(workspace.path))
    if changes:
        reason = "preexisting workspace changes" if prepared_inputs is not None else "preexisting tracked changes"
        raise EvaluationFailure(PhaseOutcome(
            "blocked", f"{phase}: evaluator workspace has {reason}: {', '.join(changes)}"))
    before_head = git(workspace.path, "rev-parse", "HEAD").strip()
    before_files = flowfiles.workspace_snapshot(workspace.path)
    # The engine's own run logs of this run: an old copy must not be carried into the role's folder and back.
    shutil.rmtree(ctx.store.root / evidence_dir / EVALUATOR_DIR, ignore_errors=True)
    checks.clear_row_receipts(ctx.store)
    result = ctx.runner.run(request)
    after_head = git(workspace.path, "rev-parse", "HEAD").strip()
    after_files = flowfiles.workspace_snapshot(workspace.path)
    changed_files = {name for name in before_files.keys() | after_files.keys()
                     if before_files.get(name) != after_files.get(name)}
    changed_files.update(name for name in prepared_inputs or {}
                         if not _matches_prepared_input(workspace.path, name, prepared_inputs or {}))
    changed_files = sorted(changed_files)
    if after_head != before_head or changed_files:
        detail = f"; changed paths: {', '.join(changed_files)}" if changed_files else ""
        raise EvaluationFailure(PhaseOutcome("blocked", f"{phase}: evaluator modified the evaluated workspace{detail}"))
    failure = runs.failure_outcome(result, role, phase)
    if failure:
        raise EvaluationFailure(failure)
    return result.result


def _prepared_workspace_changes(workspace: Path, prepared_inputs: dict[str, tuple[bytes, bool]]) -> list[str]:
    allowed = lambda name: _matches_prepared_input(workspace, name, prepared_inputs)
    changed = {name for name in flowfiles.uncommitted(workspace) if not allowed(name)}
    changed.update(name for name in prepared_inputs if not allowed(name))
    return sorted(changed)


def _matches_prepared_input(workspace: Path, name: str, prepared_inputs: dict[str, tuple[bytes, bool]]) -> bool:
    """Whether workspace file `name` still is the input the engine prepared, read without following a link."""
    expected = prepared_inputs.get(name)
    if expected is None:
        return False
    content, executable = expected
    try:
        return beneath.read_bytes(workspace, name) == content and beneath.is_executable(workspace, name) == executable
    except (OSError, ValueError):
        return False


def run_row(ctx: FlowContext, row: contract.Row) -> int:
    """Run one saved evaluator row the way the engine runs a `check` row: approved, in the checks cage, logged.

    Called for the evaluator by its `run_row` tool (the engine's own process, outside the agent's sandbox). Only
    evaluator rows run, in a checkout the engine makes outside the agent's reach. On the final evaluation that is the
    engine's completion checkout of its own head (`flow/final_head.py`), the one the application under test runs from,
    whatever the workspace holds: no workspace or branch is read, and a receipt is written only while the revision is
    the engine's head, so a passing run is a statement about that head and nothing else; the evaluator's PASS only
    counts with it. On calibration it is the engine's checkout of its base build (`completion.base_build`), and no
    receipt is written. The log under `evidence/` is the evidence.
    """
    if contract.prover_of(row, ctx.steps) != contract.EVALUATOR:
        raise contract.ContractError(f"row {row.id} is not proven by the evaluator; run_row serves evaluator rows only")
    stack_id = os.environ.get("SAKURA_FLOW_STACK", "")
    stack_entry = _row_stack(ctx, row, stack_id)
    base_url = os.environ.get("SAKURA_FLOW_BASE_URL", "") if stack_entry else ""
    if stack_entry and not base_url:
        raise contract.ContractError("evaluator row has no app base URL")
    is_final = os.environ.get("SAKURA_FLOW_PHASE") == PHASE
    folder = EVALUATOR_DIR if is_final else f"{BASE_WORKSPACE}/{EVALUATOR_DIR}"
    try:
        with _row_checkout(ctx, is_final) as checkout:
            revision = checkout.revision
            code = baseline.run_rows(ctx, checkout.source, base_url, stack_entry["port"] if stack_entry else 0, [row],
                                     folder, stack_id,
                                     keepalive=lambda: _refresh_stack(ctx, stack_id) if stack_id else None,
                                     build_dir=checkout.build)[row.id]
            changed = completion.changes(checkout)
    except checks.ChecksNeedApproval:
        print(f"run-row {row.id}: the row's command is not approved; the person approves it with "
              f"sakura flow approve {ctx.store.task_id} checks", file=sys.stderr, flush=True)
        return 1
    except (baseline.FlowInterrupted, completion.CompletionRefused) as interrupted:
        print(f"run-row {row.id}: {interrupted}", file=sys.stderr, flush=True)
        return 1
    if changed:
        print(f"run-row {row.id}: the checkout of {revision[:12]} changed during the run ({changed}); no receipt",
              file=sys.stderr, flush=True)
        return 1
    log = ctx.store.evidence_dir / folder / f"{row.id}-{row.viewport or baseline.API_LABEL}.log"
    return _finish_row(ctx, row, revision, code, log, is_final)


def _finish_row(ctx: FlowContext, row: contract.Row, revision: str, code: int, log: Path, is_final: bool) -> int:
    """Report the run, which ran in the engine's checkout (no workspace is read); on the final evaluation leave the
    receipt, only while its revision still is the engine's head. Calibration leaves none."""
    if is_final:
        moved = _not_the_head(ctx, row, revision)
        if moved:
            print(moved + "; no receipt", file=sys.stderr, flush=True)
            return 1
        _record_row_receipt(ctx, row, revision, code, log)
    # The log's first line is `exit N`, then the command, then the output.
    print(log.read_text(errors="replace").split("\n", 2)[-1], end="", flush=True)
    print(f"run-row {row.id}: exit {code}\nevidence: {log.relative_to(ctx.store.root)}", file=sys.stderr, flush=True)
    return code


def _row_stack(ctx: FlowContext, row: contract.Row, stack_id: str) -> dict | None:
    """The registered app stack of this task's workspace the row runs against; refuses a stack that is not."""
    if stack_id and not stack_id.startswith(f"{ctx.store.task_id}:"):
        raise contract.ContractError("evaluator row has no matching registered app stack")
    stack_entry = _refresh_stack(ctx, stack_id) if stack_id else None
    if stack_entry and Path(stack_entry["workspace"]).resolve() != Path.cwd().resolve():
        raise contract.ContractError("evaluator row has no matching registered app stack")
    if not stack_entry and row.kind != "cli":
        raise contract.ContractError("evaluator row has no matching registered app stack")
    return stack_entry


def _not_the_head(ctx: FlowContext, row: contract.Row, revision: str) -> str:
    try:
        require_the_engines_head(ctx, revision)
    except completion.CompletionRefused as error:
        return f"run-row {row.id}: {error}"
    return ""


@contextlib.contextmanager
def _row_checkout(ctx: FlowContext, is_final: bool) -> Iterator[completion.Checkout]:
    """The checkout the engine holds while its evaluator runs, whatever the workspace holds: on the final evaluation
    the completion checkout of its head, on calibration the checkout of its base build."""
    checkout = completion.recorded(ctx.store.read_state())
    if checkout is None:
        held = "completion checkout of its head" if is_final else "checkout of its base build"
        raise completion.CompletionRefused(f"the engine holds no {held}")
    if is_final:
        require_the_engines_head(ctx, checkout.revision)
    else:
        _require_the_base_build(ctx, checkout.revision)
    changed = completion.changes(checkout)
    if changed:
        raise completion.CompletionRefused(f"the completion checkout of {checkout.revision[:12]} changed ({changed})")
    yield checkout


def _require_the_base_build(ctx: FlowContext, revision: str) -> None:
    """Raise CompletionRefused unless `revision` is the engine's base build, checked against its store."""
    try:
        build = final_head.base_build(ctx)[1]
    except GitError as error:
        raise completion.CompletionRefused(str(error)) from error
    if revision != build:
        raise completion.CompletionRefused(f"{revision[:12]} is not the engine's base build {build[:12]}")


def _record_row_receipt(ctx: FlowContext, row: contract.Row, revision: str, code: int, log: Path) -> None:
    state = ctx.store.read_state()
    digest = contract.rows_digest(contract.load(ctx.store, state["contract_sha256"]))
    checks.write_row_receipt(ctx.store, row, digest, revision, code, log)


def _refresh_stack(ctx: FlowContext, stack_id: str) -> dict | None:
    store = TaskStore(ctx.repo, ctx.store.task_id)
    with stacks.lifecycle_lock(store, stack_id):
        entry = store.read_state().get("stacks", {}).get(stack_id)
        if not entry:
            return None
        interval = min(stacks.MAX_TOUCH_INTERVAL_SECONDS,
                       max(0.2, ctx.config.values["load"]["idle_stack_seconds"] / 2))
        now = time.time()
        if now - entry["last_used"] >= interval:
            store.update_state(lambda state: state.get("stacks", {}).get(stack_id, {}).update(last_used=now))
            entry = store.read_state().get("stacks", {}).get(stack_id)
        return entry


class EvaluationFailure(RuntimeError):
    """An evaluator run stopped, stalled or exited unsuccessfully."""

    def __init__(self, outcome: PhaseOutcome) -> None:
        super().__init__(outcome.reason)
        self.outcome = outcome


def _store_passes(ctx: FlowContext, rows: list[contract.Row], verdicts: list[dict],
                  started_ns: int, checkout: completion.Checkout) -> None:
    """Flip the rows the evaluator passed with evidence and the engine's receipt of a passing run since it started.

    The receipt is written by the engine when the evaluator's `run_row` tool ran the row in the completion checkout,
    so the agent's word alone never passes a row; nothing passes when the checkout left its revision or the revision
    is not the engine's head.
    """
    errors = _validate_evidence(ctx, rows, verdicts, started_ns, require_pass=True)
    checks.ingest_row_receipts(ctx.store)
    completion.require_unchanged(checkout)
    require_the_engines_head(ctx, checkout.revision)
    state = ctx.store.read_state()
    digest = contract.rows_digest(contract.load(ctx.store, state["contract_sha256"]))
    revision = checkout.revision
    for row in rows:
        verdict = _verdict_for(verdicts, row)
        if verdict and verdict.get("verdict") == "PASS" and not any(error.startswith(f"{row.id}:") for error in errors):
            if not checks.receipt_passes(state, row, digest, revision, started_ns, checks.EVALUATOR_RECEIPTS_KEY,
                                         ctx.store.root):
                errors.append(f"{row.id}: no engine receipt of a passing run_row for this revision; PASS ignored")
                continue
            path = _evidence_path(ctx, verdict.get("evidence"))
            if path:
                contract.mark_passed(ctx.store, ctx.store.read_state(), row.id, row.viewport, path)
    for error in errors:
        ctx.store.append_ledger(PHASE, error)


def _validate_evidence(ctx: FlowContext, rows: list[contract.Row], verdicts: list[dict], started_ns: int,
                       require_pass: bool) -> list[str]:
    errors = []
    for row in rows:
        verdict = _verdict_for(verdicts, row)
        if not verdict or verdict.get("verdict") not in ("PASS", "NEEDS_WORK"):
            errors.append(f"{row.id}: evaluator returned no valid verdict")
            continue
        if verdict.get("evidence") and _evidence_path(ctx, verdict["evidence"]) is None:
            errors.append(f"{row.id}: evidence path is outside evidence/")
            continue
        if verdict["verdict"] != "PASS":
            continue
        path = _evidence_path(ctx, verdict.get("evidence"))
        reason = _evidence_problem(path, started_ns)
        if reason:
            errors.append(f"{row.id}: {reason}")
        elif require_pass and not row.passes:
            continue
    return errors


def _evidence_problem(path: Path | None, started_ns: int) -> str:
    if path is None or not path.is_file() or path.stat().st_size == 0:
        return "PASS evidence is missing, empty or outside evidence/"
    if path.stat().st_mtime_ns <= started_ns:
        return "PASS evidence was not modified after the evaluator run started"
    return ""


def _evidence_path(ctx: FlowContext, raw: object) -> Path | None:
    if not isinstance(raw, str) or not raw:
        return None
    root = ctx.store.evidence_dir.resolve()
    path = (ctx.store.root / raw).resolve()
    try:
        path.relative_to(root)
    except ValueError:
        return None
    return path


def _verdict_rows(result: dict | None) -> list[dict]:
    items = result.get("rows") if isinstance(result, dict) else None
    return [item for item in items if isinstance(item, dict)] if isinstance(items, list) else []


def _verdict_for(verdicts: list[dict], row: contract.Row) -> dict | None:
    return next((entry for entry in verdicts if entry.get("id") == row.id and entry.get("viewport") == row.viewport), None)


def _unpassed(ctx: FlowContext) -> list[contract.Row]:
    """The rows this phase still has to prove: checks and evaluator rows; owner rows wait for the person."""
    rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    return [row for row in contract.proven_by(rows, ctx.steps, *contract.ENGINE_RUN_PROVERS) if not row.passes]


def _evaluator_rows(ctx: FlowContext) -> list[contract.Row]:
    return [row for row in _unpassed(ctx) if contract.prover_of(row, ctx.steps) == contract.EVALUATOR]


def _rows_json(rows: list[contract.Row]) -> str:
    return json.dumps([row.__dict__ for row in rows], indent=2)


def _workspace(ctx: FlowContext) -> workspaces.Workspace:
    entry = ctx.store.read_state()["workspaces"][ZERO_WORKSPACE]
    return workspaces.Workspace(ZERO_WORKSPACE, Path(entry["path"]), entry["branch"],
                                range(entry["ports"][0], entry["ports"][1] + 1))


def _run_fix(ctx: FlowContext, zero: Path, number: int, rows: list[contract.Row]) -> PhaseOutcome | None:
    """One fix round in workspace 0: a fix is built on the engine's head (or on the candidate still pending), and the
    review and the brakes cover everything from that head to the fix, whatever the branch held."""
    state = ctx.store.read_state()
    progress = state.get("evaluation_fix", {})
    pending = progress.get("pending")
    accepted_head = final_head.recorded(state)
    if not pending:
        try:
            final_head.show(ctx, zero, state["workspaces"][ZERO_WORKSPACE]["branch"])
        except GitError as error:
            return PhaseOutcome("blocked", f"workspace 0 cannot be moved to the engine's head: {error}")
    start = git(zero, "rev-parse", "HEAD").strip()
    baseline = pending.get("baseline", accepted_head) if pending else accepted_head
    _begin_fix_candidate(ctx, accepted_head, baseline, start, number)
    values = {"task_id": ctx.store.task_id, "rows": _rows_json(rows), "findings": "", "files": "",
              "port": str(state["workspaces"][ZERO_WORKSPACE]["ports"][0]),
              "ports": "-".join(map(str, state["workspaces"][ZERO_WORKSPACE]["ports"]))}
    task_id = f"{FIX_TASK_PREFIX}{number}"
    request = runner.RunRequest("builder", "build", runner.load_template("fix"), values, zero,
                                {"SAKURA_FLOW_BUILD_TASK": task_id})
    result = ctx.runner.run(request)
    failure = runs.failure_outcome(result, "builder", "build")
    if failure:
        return failure
    head = git(zero, "rev-parse", "HEAD").strip()
    if head == start:
        return PhaseOutcome("blocked", f"{task_id}: builder made no commit")
    _update_fix_candidate_head(ctx, head)
    return _validate_fix_candidate(ctx, zero, task_id, number, baseline, head)


def _resume_fix_candidate(ctx: FlowContext, zero: Path, number: int, pending: dict) -> PhaseOutcome | None:
    task_id = f"{FIX_TASK_PREFIX}{number}"
    head = git(zero, "rev-parse", "HEAD").strip()
    if head != pending.get("head") or head == pending.get("baseline"):
        return PhaseOutcome("blocked", f"{task_id}: pending candidate head does not match workspace")
    return _validate_fix_candidate(ctx, zero, task_id, number, pending["baseline"], head)


def _validate_fix_candidate(ctx: FlowContext, zero: Path, task_id: str, number: int,
                            baseline: str, head: str) -> PhaseOutcome | None:
    try:
        test_code = _run_focused_tests(ctx, zero)
    except process.TestCommandInterrupted as error:
        if error.outcome.stopped:
            return PhaseOutcome("stopped", f"stopped during tests.focused ({task_id})")
        reason = ("timed out" if error.outcome.timed_out else process.LEFT_RUNNING if error.outcome.left_running
                  else "stalled")
        return PhaseOutcome("blocked", f"{reason}: tests.focused ({task_id})")
    if test_code != 0:
        return PhaseOutcome("blocked", f"{task_id}: focused tests failed")
    problem = _test_only_problem(ctx, head)
    if problem:
        return PhaseOutcome("blocked", f"{task_id}: {problem}")
    try:
        integrate.check_protected(ctx, f"build task {task_id}", baseline, head)
    except integrate.MergeBlocked as error:
        return PhaseOutcome("blocked", str(error))
    if ctx.steps.code_review:
        failure = _review_fix(ctx, task_id, number, baseline, head)
        if failure:
            return failure
    return None


def _begin_fix_candidate(ctx: FlowContext, accepted_head: str, baseline: str, head: str, number: int) -> None:
    def update(state: dict) -> None:
        progress = state.get("evaluation_fix", {})
        progress.update(accepted_head=accepted_head, attempts=number,
                        pending={"baseline": baseline, "head": head, "round": number})
        state["evaluation_fix"] = progress

    ctx.store.update_state(update)


def _update_fix_candidate_head(ctx: FlowContext, head: str) -> None:
    def update(state: dict) -> None:
        state["evaluation_fix"]["pending"]["head"] = head

    ctx.store.update_state(update)


def _accept_fix_candidate(ctx: FlowContext) -> None:
    """The reviewed candidate, its exact commit, becomes the engine's head (`final_head.fix`)."""
    progress = ctx.store.read_state().get("evaluation_fix") or {}
    pending = progress.get("pending")
    if not pending:
        return
    try:
        head = final_head.fix(ctx, f"{FIX_TASK_PREFIX}{pending['round']}", pending["head"])
    except integrate.MergeBlocked as error:
        raise EvaluationFailure(PhaseOutcome("blocked", str(error))) from error

    def update(state: dict) -> None:
        state["evaluation_fix"].update(accepted_head=head, pending=None)

    ctx.store.update_state(update)


def _clear_fix_progress(ctx: FlowContext) -> None:
    ctx.store.update_state(lambda state: state.pop("evaluation_fix", None))


def _review_fix(ctx: FlowContext, task_id: str, number: int, start: str, head: str) -> PhaseOutcome | None:
    """The reviewer judges the fix `start..head` in a sealed copy of its own (`final_head.review_copy`), never in
    workspace 0 or the task's repository."""
    with final_head.review_copy(ctx, f"review-{task_id}", start, head) as (repository, copy):
        values = {"task_id": task_id, "title": f"Evaluator fix {number}", "repo_path": str(copy),
                  "start": start, "branch": head, "spec_path": str(ctx.store.spec),
                  "contract_path": str(ctx.store.contract), "plan_md_path": str(ctx.store.plan_md),
                  "flows": final_head.flows_review_text(ctx.store.read_state())}
        request = runner.RunRequest("code_reviewer", "review", runner.load_template("review"), values, copy,
                                    repository=repository)
        result = ctx.runner.run(request)
        failure = runs.failure_outcome(result, "code_reviewer", "review") or runs.workspace_changes(
            copy, "code_reviewer", "review", head)
    if failure:
        return failure
    verdict, findings = runs.parse_verdict(result.result)
    build_task._record_review(ctx, _fix_task(task_id), number, verdict, findings)
    if verdict != "APPROVE":
        return PhaseOutcome("blocked", f"{task_id} code review needs revision: " + "; ".join(findings))
    return None


def _fix_task(task_id: str) -> plan.BuildTask:
    return plan.BuildTask(task_id, f"Evaluator fix {task_id}", [], [], [], False)


def _run_focused_tests(ctx: FlowContext, zero: Path) -> int:
    timeouts = ctx.config.values["timeouts"]
    try:
        with process.heavy_slot(ctx.config, "tests.focused", timeout_seconds=timeouts["build"],
                                stop_file=ctx.store.stop_file):
            return testing.run(zero, "focused", command_runner=lambda command, cwd:
                               process.run_test_command(ctx, command, cwd, budget_seconds=timeouts["build"],
                                                        idle_seconds=timeouts["idle_seconds"]))
    except (slots.SlotStopped, slots.SlotTimeout):
        return 1


def _test_only_problem(ctx: FlowContext, head: str) -> str:
    """A test-only marker the change from the base to `head` adds to a production path, read from the engine's
    store (`head` sealed there first), or ''."""
    state = ctx.store.read_state()
    patterns = ctx.config.values["brakes"]["test_patterns"]
    markers = ctx.config.values["brakes"]["test_only_markers"]
    store = final_head.sealed_candidate(ctx, head)
    for path, line in _diff(store, state["base_commit"], head):
        if globs.matches_any(path, patterns):
            continue
        marker = next((marker for marker in markers if marker in line), None)
        if marker:
            return f"test-only marker {marker} in production path {path}"
    return ""


def _diff(store: Path, base: str, head: str) -> list[tuple[str, str]]:
    raw = git(store, "diff", "--no-ext-diff", "--no-textconv", "--no-renames", "-U0", f"{base}..{head}")
    found = []
    path = ""
    for line in raw.splitlines():
        if line.startswith("+++ b/"):
            path = line[6:]
        elif line.startswith("+") and not line.startswith("+++"):
            found.append((path, line[1:]))
    return found


def _require_bounded_blobs(store: Path, head: str, names: list[str]) -> None:
    """Raise EvaluationFailure, before any blob is read, unless the files `names` of `head` keep to the flow-file
    limits (`flow/flow_paths.py`): their count, and each size and the total read from Git's object metadata
    (review 15, finding 5)."""
    refused = flow_paths.count_refusal(len(names))
    left = flow_paths.MAX_FLOW_TOTAL_BYTES
    for name in names if not refused else ():
        size = int(git(store, "cat-file", "-s", f"{head}:{name}").strip())
        refused = flow_paths.size_refusal(name, size, left)
        if refused:
            break
        left -= size
    if refused:
        raise EvaluationFailure(PhaseOutcome("blocked", f"calibration copies no test file of the head: {refused}"))


def _sync_contract_flows(ctx: FlowContext, base: Path) -> dict[str, tuple[bytes, bool]]:
    """Copy the test and contract-flow files the engine's head changed into the base checkout for calibration, read
    from the engine's store, never from a workspace."""
    state = ctx.store.read_state()
    store, head = final_head.proved(ctx)
    patterns = ctx.config.values["brakes"]["test_patterns"]
    changes = git(store, "diff", "--name-status", "--no-renames", "-z", f"{state['base_commit']}..{head}").split("\0")
    names = [name for status, name in zip(changes[0::2], changes[1::2])
             if name and status != "D" and globs.matches_any(name, patterns)
             and git(store, "ls-tree", head, "--", name).split(" ", 1)[0] in ("100644", "100755")]
    _require_bounded_blobs(store, head, names)
    prepared: dict[str, tuple[bytes, bool]] = {}
    for name in names:
        mode = git(store, "ls-tree", head, "--", name).split(" ", 1)[0]
        shown = gitutil.run(store, "show", f"{head}:{name}", text=False)
        if shown.returncode:
            raise GitError(f"git show {head[:12]}:{name} failed: {shown.stderr.decode(errors='replace').strip()}")
        executable = mode == "100755"
        # The base workspace is written by caged code too: never through a link it may have planted.
        beneath.write_bytes(base, name, shown.stdout, EXECUTABLE_MODE if executable else FILE_MODE)
        prepared[name] = (shown.stdout, executable)
    return prepared
