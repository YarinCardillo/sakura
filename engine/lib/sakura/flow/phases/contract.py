"""Contract phase: planner writes rows and flows, the base build proves them, flows are committed."""

from __future__ import annotations

import hashlib
import os
import shutil
from pathlib import Path

from sakura import beneath, slots, testing
from sakura.flow import (ask_words, baseline, checks, completion, contract, engine, final_head, flow_paths, flowfiles,
                         globs, process, repository_commands, runner, stack, task_repository, waits, workspaces)
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow import gitutil
from sakura.flow.gitutil import GitError, git, tail
from sakura.flow.render import strip_controls
from sakura.flow.phases import runs

MAX_CONTRACT_ATTEMPTS = 2
WORKSPACE_NAME = "0"
# The new flow files as the engine read them right before their base run, and the commit it made of them on the base:
# the base run runs on that commit, and that commit becomes the flows step.
FLOWS_KEY = "contract_flows"
FLOWS_COMMIT_KEY = "contract_flows_commit"
BASE_SETUP_NAME = "contract-base"
# The heads the engine itself left workspace 0 at (its flows commit, the engine's head shown there): the only heads the
# recovery may reset (`_return_to_base`); a head that is not in this list is the person's.
ENGINE_HEADS_KEY = "contract_engine_heads"
# Flows that are wrong on the base build, or that the code reviewer did not approve, go back to the planner once in
# all, with what the base run showed or what the reviewer found: the state keeps that finding until a contract is
# saved, so a second wrong contract fails or blocks the phase even after an approval wait. The planner thus writes
# the contract at most twice (each time with up to MAX_CONTRACT_ATTEMPTS runs for an invalid one).
BASE_FINDING_KEY = "contract_base_finding"
REVIEW_FINDING_KEY = "contract_review_finding"
FINDING_KEYS = (BASE_FINDING_KEY, REVIEW_FINDING_KEY)
PLANNER_ROUNDS = 2
FINDING_MAX_CHARS = 4000


def run(ctx: FlowContext) -> PhaseOutcome:
    """Produce a validated, base-proven, committed and saved contract."""
    try:
        # Nothing of the repository's own configured commands runs (not even workspace setup) before the person approved them.
        failure = _await_command_approval(ctx, _pending_rows(ctx) or [], settle=False)
        if failure:
            return failure
        state = ctx.store.read_state()
        completed = state.get("completed", {}).get("contract")
        if completed and completed != inputs_hash(ctx):
            contract.authorize_replan(ctx.store, _begin_replan)
        else:
            saved_rows = contract.recover_initial(ctx.store)
            if saved_rows is not None:
                return _resume_saved_contract(ctx)
        _discard_stale_contract_files(ctx)
        state = ctx.store.read_state()
        workspace = workspaces.create(ctx, WORKSPACE_NAME, state["branch"], state["base_commit"])
        _return_to_base(ctx, workspace)
        return _settle(ctx, workspace)
    except baseline.FlowInterrupted as interrupted:
        kind = "stopped" if interrupted.stopped else "blocked"
        return PhaseOutcome(kind, str(interrupted))
    except process.TestCommandInterrupted as interrupted:
        if interrupted.outcome.stopped:
            return PhaseOutcome("stopped", "stopped during focused tests")
        reason = ("timed out" if interrupted.outcome.timed_out else process.LEFT_RUNNING
                  if interrupted.outcome.left_running else "stalled")
        return PhaseOutcome("blocked", f"{reason}: focused tests")
    except (contract.ContractError, workspaces.WorkspaceError, stack.StackError, GitError, CommitError,
            FlowsNotApproved, repository_commands.CommandNotApproved) as error:
        return PhaseOutcome("blocked", str(error))


def _return_to_base(ctx: FlowContext, workspace: workspaces.Workspace) -> None:
    """Planning starts from the base, as after a send back: a blocked earlier attempt left its flows committed in
    workspace 0, and a planner editing those tracked files left edits the engine's head could never be checked out
    over (real proof, D6). The reset keeps the work tree, so the planner's files stay as new files.

    Only a head the engine left there (`ENGINE_HEADS_KEY`) is reset, on the task's own branch, with nothing staged, and by
    compare-and-swap on that exact head: a commit of the person's, another branch or staged work is left alone and the
    phase blocks saying so (CommitError)."""
    state = ctx.store.read_state()
    base, branch, path = state["base_commit"], state["branch"], workspace.path
    head = git(path, "rev-parse", "HEAD").strip()
    if head == base:
        return
    on_branch = gitutil.run(path, "symbolic-ref", "-q", "--short", "HEAD").stdout.strip()
    if on_branch != branch:
        raise CommitError(f"workspace 0 is on {on_branch or 'a detached head'}, not on {branch}: left as it is; switch back "
                          f"to {branch} (or discard your work there), then resume")
    if staged := git(path, "diff", "--cached", "--name-only").split():
        raise CommitError(f"workspace 0 has staged changes ({', '.join(staged[:5])}): left as they are; commit or unstage "
                          "them, then resume")
    if head not in (state.get(ENGINE_HEADS_KEY) or []):
        raise CommitError(f"workspace 0 is at {head[:12]}, not a commit the engine made: left as it is; move {branch} back "
                          f"to {base[:12]} yourself (or discard that commit), then resume")
    git(path, "update-ref", f"refs/heads/{branch}", base, head)
    git(path, "reset", "-q")


def _remember_engine_head(ctx: FlowContext, path: Path) -> None:
    """Record the head the engine just left workspace 0 at (`ENGINE_HEADS_KEY`)."""
    head = git(path, "rev-parse", "HEAD").strip()
    ctx.store.update_state(lambda state: state.update(
        {ENGINE_HEADS_KEY: list(dict.fromkeys([*state.get(ENGINE_HEADS_KEY, []), head]))}))


def _settle(ctx: FlowContext, workspace: workspaces.Workspace) -> PhaseOutcome:
    """Plan (or take the rows waiting for approval), wait for the commands' approval, prove the rows on the base build,
    have the flows reviewed and save; flows wrong on the base build or not approved by the code reviewer go back to the
    planner once in all (`_send_back`)."""
    for _ in range(PLANNER_ROUNDS):
        rows = _pending_rows(ctx)
        if rows is None:
            rows, failure = _plan_rows(ctx, workspace)
            if failure:
                return failure
        failure = _await_command_approval(ctx, rows)
        if failure:
            return failure
        outcome = _prove_and_save(ctx, workspace, rows)
        if outcome is not None:
            return outcome
    return PhaseOutcome("blocked", "the contract went back to the planner more than once")


def _prove_and_save(ctx: FlowContext, workspace: workspaces.Workspace, rows: list[contract.Row]) -> PhaseOutcome | None:
    """Prove the rows on the base build, then commit, review and save the flows; None when they went back to the
    planner instead."""
    candidate = _record_flows(ctx, workspace.path)
    problems = _wrong_on_base(ctx, workspace, rows, candidate) if _checks_on(ctx) else []
    if problems:
        if _send_back(ctx, workspace, BASE_FINDING_KEY, _base_finding(ctx, rows, problems),
                      "flows wrong on the base build go back to the planner once: " + "; ".join(problems)):
            return None
        return PhaseOutcome("failed", "contract flows are wrong on the base build: " + "; ".join(problems))
    try:
        return _commit_and_save(ctx, workspace, rows)
    except FlowsNotApproved as refused:
        if _send_back(ctx, workspace, REVIEW_FINDING_KEY, str(refused),
                      "the code reviewer's findings on the contract flows go back to the planner once"):
            return None
        return PhaseOutcome("blocked", str(refused))


def _resume_saved_contract(ctx: FlowContext) -> PhaseOutcome:
    """A contract saved before a crash: the flows commit the base run ran on becomes its head's first step if it is
    not yet; nothing is taken from workspace 0's file list or HEAD."""
    state = ctx.store.read_state()
    workspace = workspaces.create(ctx, WORKSPACE_NAME, state["branch"], state["base_commit"])
    failure = _integrate_flows(ctx, workspace.path)
    if failure:
        return failure
    protected = _protected_paths(ctx, sorted(ctx.store.read_state().get(FLOWS_KEY) or {}))
    ctx.store.update_state(lambda current: current.update(protected_paths=protected))
    return PhaseOutcome("next")


def _flow_paths(ctx: FlowContext) -> list[str]:
    """Where a contract flow may live: the host's allowlist (`contract.flow_paths`, never a repository's)."""
    return list(ctx.config.values["contract"]["flow_paths"])


def _record_flows(ctx: FlowContext, path: Path) -> str:
    """Read the new flow files once, right before their base run, and commit exactly those bytes on the base in the
    engine's store (`final_head.flows_candidate`); returns that commit, which the base run runs on and which becomes
    the flows step once it passed. The names, digests and executable bits and the commit are recorded. Raises
    CommitError when the workspace holds anything but new files the host allowlist accepts (review 12, finding 1)."""
    allowed = _flow_paths(ctx)
    base = ctx.store.read_state()["base_commit"]
    _require_only_new_flow_files(path, base, allowed)
    files = _read_flow_files(path, sorted(flowfiles.added_flow_files(path, base, allowed)))
    _require_only_new_flow_files(path, base, allowed)  # again: nothing slipped in while the list was read
    try:
        candidate = final_head.flows_candidate(ctx, files, allowed, f"test(flow): contract flows for {ctx.store.task_id}")
    except final_head.MergeBlocked as error:
        raise CommitError(str(error)) from error
    recorded = {name: {"sha256": hashlib.sha256(data).hexdigest(), "executable": executable}
                for name, (data, executable) in files.items()}
    ctx.store.update_state(lambda state: state.update({FLOWS_KEY: recorded, FLOWS_COMMIT_KEY: candidate}))
    return candidate


def _read_flow_files(path: Path, names: list[str]) -> dict[str, tuple[bytes, bool]]:
    """Each flow file's bytes and executable bit, read below `path` within the limits of `flow/flow_paths.py`: each
    read is capped by what is left of both byte limits, so a larger file is refused by its size before it is read.
    Raises CommitError naming the file and the limit."""
    refused = flow_paths.count_refusal(len(names))
    if refused:
        raise CommitError(f"the contract flows add {refused}")
    files: dict[str, tuple[bytes, bool]] = {}
    left = flow_paths.MAX_FLOW_TOTAL_BYTES
    for name in names:
        cap = min(flow_paths.MAX_FLOW_FILE_BYTES, left)
        try:
            data = beneath.read_capped(path, name, cap)
        except beneath.FileTooLarge as error:
            limit = (f"the per-file limit of {flow_paths.MAX_FLOW_FILE_BYTES} bytes"
                     if cap == flow_paths.MAX_FLOW_FILE_BYTES else
                     f"what is left of the total limit of {flow_paths.MAX_FLOW_TOTAL_BYTES} bytes for all flow files")
            raise CommitError(f"contract flow {name} is over {limit} ({error})") from error
        files[name] = (data, beneath.is_executable(path, name))
        left -= len(data)
    return files


def _require_only_new_flow_files(path: Path, base: str, allowed: list[str]) -> None:
    problems = flowfiles.flow_problems(path, base, allowed)
    if problems:
        raise CommitError("the contract flows are not only new files in the contract flow folders: "
                          + "; ".join(problems))


def _integrate_flows(ctx: FlowContext, path: Path) -> PhaseOutcome | None:
    """Make the flows commit the base run ran on (`_record_flows`) the first step of the engine's head
    (`final_head.record_flows`), once the code reviewer approved exactly that commit (`_review_flows`); the builders
    start from it and it is tested and delivered. Nothing of workspace 0 is read. Workspace 0 is then moved to the
    engine's head, as a view. Returns the outcome of a review that did not approve; raises CommitError when the commit
    is refused."""
    state = ctx.store.read_state()
    candidate = state.get(FLOWS_COMMIT_KEY)
    if not candidate:
        raise CommitError("no contract flows commit was recorded before the base run; start the task again with --new")
    failure = _review_flows(ctx, state["base_commit"], candidate)
    if failure:
        return failure
    try:
        final_head.record_flows(ctx, candidate, _flow_paths(ctx))
    except final_head.MergeBlocked as error:
        raise CommitError(str(error)) from error
    try:
        final_head.show(ctx, path, state["branch"])
        _remember_engine_head(ctx, path)
    except GitError as error:
        # Never done over a failed checkout: the next step would find workspace 0 on another revision and block for good.
        ctx.store.append_ledger("contract", f"workspace 0 cannot be moved to the engine's head: {error}")
        raise CommitError(f"workspace 0 cannot be moved to the engine's head: {error}") from error
    return None


def flows_review_values(ctx: FlowContext, copy: Path, base: str, candidate: str) -> dict[str, str]:
    """The values of the flows review prompt (`review_flows`)."""
    return {"task_id": ctx.store.task_id, "repo_path": str(copy), "start": base, "branch": candidate,
            "spec_path": str(ctx.store.spec), "contract_path": str(ctx.store.contract)}


REVIEW_STEP = "plan_review"  # the review of the contract is part of the plan, and is named so in the run's record


def _review_flows(ctx: FlowContext, base: str, candidate: str) -> PhaseOutcome | None:
    """The code reviewer judges the flows commit `candidate` on the base, exactly those bytes, in a sealed copy of its
    own (`final_head.review_copy`), and its approval is recorded bound to both (`final_head.approve_flows`): the flows
    ship with the change and run in the repository's CI, whatever the build tasks do (review 13, finding 1). Nothing
    to review without a flow file, with the review disabled, or once that exact commit was approved. Raises
    FlowsNotApproved, with the findings, when the reviewer does not approve."""
    if candidate == base:
        return None
    engine.current_steps(ctx)
    state = ctx.store.read_state()
    if (not final_head.flows_review_required(ctx)
            or state.get(final_head.FLOWS_REVIEW_KEY) == final_head.flows_receipt(base, candidate)):
        return None
    with final_head.review_copy(ctx, "review-flows", base, candidate) as (repository, copy):
        request = runner.RunRequest("code_reviewer", "review", runner.load_template("review_flows"),
                                    flows_review_values(ctx, copy, base, candidate), copy, repository=repository,
                                    step=REVIEW_STEP)
        result = ctx.runner.run(request)
        failure = runs.failure_outcome(result, "code_reviewer", "review") or runs.workspace_changes(
            copy, "code_reviewer", "review", candidate)
    if failure:
        return failure
    verdict, findings = runs.parse_verdict(result.result)
    ctx.store.append_ledger("review", f"contract flows review: {verdict}", agent_values=findings)
    if verdict != runs.APPROVE:
        raise FlowsNotApproved("the code reviewer did not approve the contract flows: " + "; ".join(findings))
    final_head.approve_flows(ctx, candidate)
    return None


def _begin_replan(state: dict) -> None:
    """Replanning starts once: the old completion marker goes with the old contract, so a resume after the person
    approved the new candidate keeps that candidate instead of replanning it again, and the old approval goes too."""
    state["completed"].pop("contract", None)
    _drop_commands_approval(state)


def _drop_commands_approval(state: dict) -> None:
    """The person's approval of the commands of a contract that is being replaced (review 17, finding 5), and the
    candidate with it."""
    waits.advance_candidate_generation(state)
    state["approvals"].pop(checks.APPROVAL_GATE, None)


def _drop_unbound_approval(state: dict) -> None:
    """A discarded candidate: its approval goes unless it records the flows it was given for."""
    if "flows" in state["approvals"].get(checks.APPROVAL_GATE, {}):
        waits.advance_candidate_generation(state)
    else:
        _drop_commands_approval(state)


def _discard_stale_contract_files(ctx: FlowContext) -> None:
    """Remove files of a contract state no longer owns, left by a crash between the state write and the unlink.

    A candidate contract that waits for the person's approval of its commands is kept. A candidate that is discarded
    takes the person's approval of its commands with it (review 17, finding 5), except an approval that records the flow
    bytes it was given for: it holds across the resume while the commands and those bytes are unchanged, and
    `_await_command_approval` drops it the moment the new candidate differs."""
    if ctx.store.read_state().get("contract_sha256") or _pending_rows(ctx) is not None:
        return
    if ctx.store.contract.exists():
        ctx.store.update_state(_drop_unbound_approval)
    ctx.store.contract.unlink(missing_ok=True)
    shutil.rmtree(ctx.store.evidence_dir / baseline.EVIDENCE_DIR, ignore_errors=True)


def inputs_hash(ctx: FlowContext) -> str:
    """Digest of the approved spec and the base commit; the phase is skipped while it is unchanged."""
    state = ctx.store.read_state()
    spec = state["approvals"].get("spec", {}).get("sha256") or hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    return hashlib.sha256(f"{spec}\0{state['base_commit']}".encode()).hexdigest()


class CommitError(RuntimeError):
    """The flow files could not be tested or committed."""


class FlowsNotApproved(RuntimeError):
    """The code reviewer did not approve the contract flows commit; the message carries its findings."""


def _plan_rows(ctx: FlowContext, workspace: workspaces.Workspace) -> tuple[list[contract.Row], PhaseOutcome | None]:
    state = ctx.store.read_state()
    error = next((state[key] for key in FINDING_KEYS if key in state), "")
    said = ""
    for _ in range(MAX_CONTRACT_ATTEMPTS):
        ctx.store.contract.unlink(missing_ok=True)
        ctx.store.update_state(waits.advance_candidate_generation)
        failure, said = _run_planner(ctx, workspace, error)
        if failure:
            return [], failure
        rows, errors = _read_rows(ctx, workspace)
        if not errors:
            return rows, None
        error = _told(ctx, errors)
    if contract.NO_NEW_ROW_ERROR in errors:
        # Blocked, not failed: the project may already do what the lane asks, and the person may resume it with a note.
        ctx.store.append_ledger("contract", f"{contract.NO_NEW_ROW_ERROR}: nothing proves the new behavior")
        return [], PhaseOutcome("blocked", ask_words.nothing_new_to_prove(said))
    if not ctx.store.contract.exists():
        ctx.store.append_ledger("contract", f"contract invalid after {MAX_CONTRACT_ATTEMPTS} attempts: {error}")
        return [], PhaseOutcome("blocked", ask_words.nothing_written("contract", said))
    return [], PhaseOutcome("blocked", f"contract invalid after {MAX_CONTRACT_ATTEMPTS} attempts: {error}")


def _told(ctx: FlowContext, errors: list[str]) -> str:
    """What the writer is told about a rejected attempt: every problem, and for a file outside the flow folders what to do
    with it (the engine never moves an agent's file: the writer does, on its one retry)."""
    told = "; ".join(errors)
    if any(flow_paths.OUTSIDE_FOLDERS in error for error in errors):
        told += (f". Every new file must be inside {_flow_folders(ctx)}: move each file named above into one of those folders "
                 "(and change the flow that uses it to the new path), or delete it")
    return told


def _flow_folders(ctx: FlowContext) -> str:
    return ", ".join(_flow_paths(ctx))


def _read_rows(ctx: FlowContext, workspace: workspaces.Workspace) -> tuple[list[contract.Row], list[str]]:
    try:
        rows = contract.load(ctx.store, None)
    except contract.ContractError as error:
        return [], [str(error)]
    tree = flowfiles.flow_problems(workspace.path, ctx.store.read_state()["base_commit"], _flow_paths(ctx))
    return rows, [*contract.validate(rows, ctx.config, ctx.steps), *tree]


def _run_planner(ctx: FlowContext, workspace: workspaces.Workspace, error: str) -> tuple[PhaseOutcome | None, str]:
    """Run the planner; its failure, if any, and what it said at the end."""
    values = {
        "spec_path": str(ctx.store.spec),
        "contract_path": str(ctx.store.contract),
        "viewports": ", ".join(ctx.config.values["tests"]["viewports"]),
        "provers": ", ".join(f'"{prover}"' for prover in contract.PROVERS
                             if ctx.steps.evaluate or prover != contract.EVALUATOR),
        "flow_folders": _flow_folders(ctx),
        "previous_error": f"\nYour previous attempt was rejected: {error}. Fix that and write the files again." if error else "",
    }
    result = ctx.runner.run(runner.RunRequest("planner", "contract", runner.load_template("contract"), values, workspace.path, {}))
    return runs.failure_outcome(result, "planner", "contract"), result.final


def _pending_rows(ctx: FlowContext) -> list[contract.Row] | None:
    """The candidate rows that wait for command approval, while the contract file is still the one they came from."""
    pending = ctx.store.read_state().get(checks.PENDING_KEY)
    if not pending or not ctx.store.contract.is_file():
        return None
    if hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest() != pending["contract"]:
        return None
    return contract.load(ctx.store, None)


def _await_command_approval(ctx: FlowContext, rows: list[contract.Row], settle: bool = True) -> PhaseOutcome | None:
    """Wait at the `checks` gate while a command is neither on the host list nor approved by the person.

    The commands are the rows' and the ones a repository's own config supplies. `settle` also drops the
    waiting record once everything is approved; the early check before any candidate rows exist leaves it.

    With the checks step off the rows' commands are never run, so they are never asked about: only the commands a
    repository's own configuration supplies still wait for the person.
    """
    rows = rows if _checks_on(ctx) else []
    flows = _candidate_flows(ctx) if rows else None
    try:
        listed = ctx.config.values["checks"]
        checks.require_approved(ctx.store.read_state(), rows, ctx.steps, listed["approved"], listed["remembered"],
                                checks.repository_commands(ctx.config), flows)
    except checks.ChecksNeedApproval as waiting:
        summary = checks.wait_for_approval(ctx.store, waiting, rows, ctx.steps, flows)
        ctx.store.append_ledger("contract", f"{summary} wait for the person's approval")
        # The commands are the planner's text: the reason only counts them, `sakura flow status` lists them.
        return PhaseOutcome("await", f"{summary} to approve; see sakura flow status", gate=checks.APPROVAL_GATE)
    if settle:
        ctx.store.update_state(lambda state: state.pop(checks.PENDING_KEY, None))
    return None


def _checks_on(ctx: FlowContext) -> bool:
    """Whether the lane's preset runs the checks step (the config it runs with now, as `phases/evaluate.py` reads it)."""
    return bool(ctx.config.values["knobs"]["checks"])


def _candidate_flows(ctx: FlowContext) -> str:
    """The digest of the contract flows the planner left in workspace 0 (`flowfiles.flow_digest`); "" with none."""
    state = ctx.store.read_state()
    recorded = state.get("workspaces", {}).get(WORKSPACE_NAME)
    if not recorded or not Path(recorded["path"]).is_dir():
        return ""
    return flowfiles.flow_digest(Path(recorded["path"]), state["base_commit"], _flow_paths(ctx))


def _wrong_on_base(ctx: FlowContext, workspace: workspaces.Workspace, rows: list[contract.Row],
                   candidate: str) -> list[str]:
    """Run every row the engine or the evaluator proves on the base build (an owner row's flow is for the person);
    returns each row whose result contradicts its kind.

    The application and the rows run from an engine-owned checkout of `candidate` (the sealed base plus the flow
    bytes the engine read, `_record_flows`), made at a fresh path no cage was granted, with the source read-only and
    their writes in the checkout's own build folder, as completion does (review 12, finding 2): nothing a planner, a
    survivor or the app's own start command changes in workspace 0 reaches the result."""
    rows = contract.proven_by(rows, ctx.steps, *contract.ENGINE_RUN_PROVERS)
    store = final_head.sealed_candidate(ctx, candidate)
    root = task_repository.workspace_root(ctx.config.values, ctx.repo)
    try:
        with completion.at(store, candidate, root, f"{ctx.store.task_id}-base-{os.getpid()}") as checkout:
            workspaces.run_setup(ctx, BASE_SETUP_NAME, checkout.source, checkout.build)
            with stack.start(ctx, workspace, checkout) as app:
                codes = baseline.run_rows(ctx, checkout.source, app.base_url, app.port, rows,
                                          build_dir=checkout.build)
            completion.require_unchanged(checkout)
    except completion.CompletionRefused as error:
        raise CommitError(f"the base run is not a statement about the base: {error}") from error
    return baseline.wrong_on_base(rows, codes)


def _base_finding(ctx: FlowContext, rows: list[contract.Row], problems: list[str]) -> str:
    """What the base run showed: the problems and the end of each wrong row's log."""
    tails = [f"{row.id} ended with:\n{tail(log)}" for row in rows
             for log in sorted((ctx.store.evidence_dir / baseline.EVIDENCE_DIR).glob(f"{row.id}-*.log"))
             if any(problem.startswith(f"{row.id} ") for problem in problems)]
    return ("; ".join(problems) + ". Run the flows on the base build yourself before you write them again.\n"
            + "\n".join(tails))


def _send_back(ctx: FlowContext, workspace: workspaces.Workspace, key: str, finding: str, note: str) -> bool:
    """Record `finding` under `key` for the planner's next attempt, once for both kinds (`FINDING_KEYS`), and ledger
    `note`; False when the planner already had that attempt. The finding can hold an agent's or a log's text: it is
    kept without control characters and bounded. This contract, its base evidence and the person's approval of its
    commands are discarded, since new flow bytes need a new approval even for the same commands, and workspace 0 goes
    back to the base with the planner's files in place, without the commit `_test_and_commit` made of them."""
    state = ctx.store.read_state()
    if any(name in state for name in FINDING_KEYS):
        return False
    bounded = strip_controls(finding)[:FINDING_MAX_CHARS]

    def sent_back(current: dict) -> None:
        waits.advance_candidate_generation(current)
        current[key] = bounded
        current["approvals"].pop(checks.APPROVAL_GATE, None)

    ctx.store.update_state(sent_back)
    ctx.store.append_ledger("contract", note)
    ctx.store.contract.unlink(missing_ok=True)
    shutil.rmtree(ctx.store.evidence_dir / baseline.EVIDENCE_DIR, ignore_errors=True)
    _return_to_base(ctx, workspace)
    return True


def _commit_and_save(ctx: FlowContext, workspace: workspaces.Workspace, rows: list[contract.Row]) -> PhaseOutcome:
    allowed = _flow_paths(ctx)
    base = ctx.store.read_state()["base_commit"]
    changed = flowfiles.flow_problems(workspace.path, base, allowed)
    if changed:
        raise CommitError("worktree changed during the base run: " + "; ".join(changed))
    git(workspace.path, "reset", "-q")
    flow_files = flowfiles.stage(workspace.path, base, allowed)
    _test_and_commit(ctx, workspace.path, base, flow_files)
    failure = _integrate_flows(ctx, workspace.path)
    if failure:
        return failure
    digest = contract.save(ctx.store, rows)
    protected = _protected_paths(ctx, sorted(ctx.store.read_state().get(FLOWS_KEY) or {}))

    def saved(state: dict) -> None:
        state.update(contract_sha256=digest, protected_paths=protected)
        for key in FINDING_KEYS:
            state.pop(key, None)

    ctx.store.update_state(saved)
    return PhaseOutcome("next")


def check_staged(path: Path, base_commit: str, flow_files: list[str]) -> None:
    """Raise CommitError unless the index differs from `base_commit` only by adding `flow_files`."""
    lines = git(path, "diff", "--cached", "--name-status", "--no-renames", "-z", base_commit).split("\0")
    stray = [name for status, name in zip(lines[0::2], lines[1::2]) if name and (status != "A" or name not in flow_files)]
    if stray:
        raise CommitError(f"refusing to commit unvetted staged paths: {', '.join(stray)}")


def _test_and_commit(ctx: FlowContext, path: Path, base_commit: str, flow_files: list[str]) -> None:
    try:
        with process.heavy_slot(ctx.config, "tests.focused", timeout_seconds=ctx.config.values["timeouts"]["contract"],
                                stop_file=ctx.store.stop_file):
            timeouts = ctx.config.values["timeouts"]
            test_code = testing.run(path, "focused", command_runner=lambda command, cwd:
                                    process.run_test_command(ctx, command, cwd,
                                                             budget_seconds=timeouts["contract"],
                                                             idle_seconds=timeouts["idle_seconds"]))
    except slots.SlotTimeout as error:
        raise CommitError("contract phase stalled waiting for the focused-test slot") from error
    except slots.SlotStopped as error:
        raise CommitError("contract phase stopped waiting for the focused-test slot") from error
    if test_code != 0:
        raise CommitError("focused tests failed with the contract flows added; see the output above")
    check_staged(path, base_commit, flow_files)
    if git(path, "diff", "--cached", "--name-only").strip():
        git(path, "commit", "-q", "-m", f"test(flow): contract flows for {ctx.store.task_id}")
        _remember_engine_head(ctx, path)


def _protected_paths(ctx: FlowContext, flow_files: list[str]) -> list[str]:
    """The test files of the engine's head (read from its store, never from workspace 0's index), the flow files and
    the contract."""
    patterns = ctx.config.values["brakes"]["test_patterns"]
    store, head = final_head.proved(ctx)
    tracked = [name for name in git(store, "ls-tree", "-r", "--name-only", head).splitlines()
               if globs.matches_any(name, patterns)]
    return sorted({*tracked, *flow_files, str(ctx.store.contract)})
