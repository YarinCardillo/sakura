"""Contract rows: validation, integrity and the contract phase."""

import hashlib
import json
import sys

import pytest

from sakura.flow import config as flow_config
from sakura.flow import ask_words, baseline, checks, contract, controls, final_head, inbox, ports, sealed, store
from sakura.flow.phases import contract as contract_phase

from conftest import git
from flow_support import FakeRunner, make_context

STEPS = flow_config.PresetSteps(True, True, True)
NO_EVALUATE = flow_config.PresetSteps(True, True, False)


def _row(**changes):
    row = {"id": "R1", "journey": "J1", "step": "s", "kind": "web", "viewport": "1440x900", "flow": "true",
           "baseline": False, "passes": False}
    return contract.Row(**{**row, **changes})


def _cfg(repo):
    return flow_config.load(repo, home=repo / "nohome")


def _rows():
    return [_row(), _row(id="B1", baseline=True)]


def test_contract_validate_rejects_true_rows_and_missing_viewport(repo):
    cfg = _cfg(repo)
    assert contract.validate(_rows(), cfg, STEPS) == []
    errors = contract.validate([_row(passes=True), _row(id="R2", viewport=None), _row(id="R2", viewport="1x1", flow=""),
                                _row(id="B1", baseline=True)], cfg, STEPS)
    text = "\n".join(errors)
    assert "R1" in text and "passes" in text
    assert "R2" in text and "viewport" in text
    assert "duplicate" in text
    assert "flow" in text


def test_contract_validate_rejects_missing_required_row_ids(repo):
    cfg = flow_config.load(repo, home=repo / "nohome", overrides={"contract": {"required_ids": ["R15", "R16"]}})

    assert contract.validate(_rows(), cfg, STEPS) == ["contract is missing required row ids: R15, R16"]
    assert contract.validate([*_rows(), _row(id="R15"), _row(id="R16")], cfg, STEPS) == []


def test_contract_validate_requires_baseline_only_when_evaluating(repo):
    cfg = _cfg(repo)
    only_new = [_row()]
    assert any("baseline" in error for error in contract.validate(only_new, cfg, STEPS))
    assert contract.validate(only_new, cfg, NO_EVALUATE) == []
    assert any("non-baseline" in error for error in contract.validate([_row(baseline=True)], cfg, NO_EVALUATE))


def test_contract_load_detects_tampering(repo):
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo"})
    digest = contract.save(task_store, _rows())
    assert [row.id for row in contract.load(task_store, digest)] == ["R1", "B1"]
    task_store.contract.write_text(task_store.contract.read_text().replace('"passes": false', '"passes": true', 1))
    with pytest.raises(contract.ContractTampered):
        contract.load(task_store, digest)


def test_contract_save_updates_only_from_authenticated_old_digest(repo):
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo", "contract_sha256": ""})
    original = _rows()
    first_digest = contract.save(task_store, original)
    assert task_store.read_state()["contract_sha256"] == first_digest
    updated = [_row(step="updated"), _row(id="B1", baseline=True)]

    second_digest = contract.save(task_store, updated)

    assert second_digest == hashlib.sha256(task_store.contract.read_bytes()).hexdigest()
    assert task_store.read_state()["contract_sha256"] == second_digest
    assert contract.load(task_store, second_digest) == updated
    task_store.contract.write_text("{\"rows\": []}\n")
    with pytest.raises(contract.ContractTampered):
        contract.save(task_store, original)


def test_contract_load_reports_malformed_file(repo):
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo"})
    task_store.contract.write_text("{not json")
    with pytest.raises(contract.ContractError):
        contract.load(task_store, None)


def test_mark_passed_updates_row_and_hash(repo):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    digest = contract.save(ctx.store, _rows())
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest))
    evidence = repo / "proof.png"
    evidence.write_bytes(b"x")
    contract.mark_passed(ctx.store, ctx.store.read_state(), "R1", "1440x900", evidence)
    state = ctx.store.read_state()
    rows = {row.id: row for row in contract.load(ctx.store, state["contract_sha256"])}
    assert rows["R1"].passes is True and rows["B1"].passes is False


def test_mark_passed_refuses_empty_evidence(repo):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    digest = contract.save(ctx.store, _rows())
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest))
    empty = repo / "empty.png"
    empty.write_bytes(b"")
    with pytest.raises(contract.ContractError):
        contract.mark_passed(ctx.store, ctx.store.read_state(), "R1", "1440x900", empty)


def test_initial_contract_save_recovers_after_contract_replace(repo, monkeypatch):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    rows = _rows()
    candidate = contract._payload(rows) + b" "
    ctx.store.contract.write_bytes(candidate)
    old_digest = hashlib.sha256(candidate).hexdigest()
    original_update = ctx.store.update_state
    updates = 0

    def interrupt_final_bind(update):
        nonlocal updates
        updates += 1
        if updates == 2:
            raise RuntimeError("interrupted after contract replace")
        return original_update(update)

    monkeypatch.setattr(ctx.store, "update_state", interrupt_final_bind)
    with pytest.raises(RuntimeError, match="after contract replace"):
        contract.save(ctx.store, rows)

    recovered = store.TaskStore(repo, "demo")
    digest = contract.save(recovered, rows)
    loaded = contract.load(recovered, digest)
    state = recovered.read_state()
    assert [row.id for row in loaded] == ["R1", "B1"]
    assert state["contract_sha256"] == digest == hashlib.sha256(recovered.contract.read_bytes()).hexdigest()
    assert "contract_transition" not in state


def test_initial_contract_save_recovers_before_contract_replace(repo, monkeypatch):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    rows = _rows()
    candidate = contract._payload(rows) + b" "
    ctx.store.contract.write_bytes(candidate)
    old_digest = hashlib.sha256(candidate).hexdigest()

    def interrupt(_store, _payload):
        raise RuntimeError("interrupted before contract replace")

    monkeypatch.setattr(contract, "_replace_contract", interrupt)
    with pytest.raises(RuntimeError, match="before contract replace"):
        contract.save(ctx.store, rows)

    recovered = store.TaskStore(repo, "demo")
    pending = recovered.read_state()["contract_transition"]
    assert pending["old_sha256"] == old_digest
    assert pending["new_sha256"] == hashlib.sha256(contract._payload(rows)).hexdigest()
    assert pending["initial"] is True
    assert contract._recover_transition(recovered) is None
    state = recovered.read_state()
    assert state["contract_sha256"] == ""
    assert "contract_transition" not in state
    assert contract._parse(recovered.contract.read_bytes()) == rows


@pytest.mark.parametrize("operation,initial_passes,expected_passes", [
    ("mark", False, True), ("reset", True, False),
])
def test_contract_verdict_transition_recovers_after_contract_replace(repo, operation, initial_passes,
                                                                     expected_passes, monkeypatch):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    rows = [_row(passes=initial_passes), _row(id="B1", baseline=True)]
    old_digest = contract.save(ctx.store, rows)
    ctx.store.update_state(lambda state: state.update(contract_sha256=old_digest))
    state = ctx.store.read_state()
    evidence = repo / "proof.png"
    evidence.write_bytes(b"proof")

    original_update = ctx.store.update_state
    updates = 0

    def interrupted(update):
        nonlocal updates
        updates += 1
        if updates == 2:
            raise RuntimeError("simulated process interruption")
        return original_update(update)

    monkeypatch.setattr(ctx.store, "update_state", interrupted)
    with pytest.raises(RuntimeError, match="simulated process interruption"):
        if operation == "mark":
            contract.mark_passed(ctx.store, state, "R1", "1440x900", evidence)
        else:
            contract.reset_passes(ctx.store, state)

    recovered = store.TaskStore(repo, "demo")
    rows = contract.load(recovered, old_digest)

    assert rows[0].passes is expected_passes
    assert recovered.read_state()["contract_sha256"] == __import__("hashlib").sha256(
        recovered.contract.read_bytes()).hexdigest()


@pytest.mark.parametrize("operation,initial_passes", [("mark", False), ("reset", True)])
def test_contract_verdict_transition_recovers_before_replace(repo, operation, initial_passes, monkeypatch):
    ctx = make_context(repo, own_repository=True, state={"contract_sha256": ""})
    digest = contract.save(ctx.store, [_row(passes=initial_passes), _row(id="B1", baseline=True)])
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest))
    state = ctx.store.read_state()
    evidence = repo / "proof.png"
    evidence.write_bytes(b"proof")

    def interrupt(_store, _payload):
        raise RuntimeError("interrupted before contract replace")

    monkeypatch.setattr(contract, "_replace_contract", interrupt)
    with pytest.raises(RuntimeError, match="before contract replace"):
        if operation == "mark":
            contract.mark_passed(ctx.store, state, "R1", "1440x900", evidence)
        else:
            contract.reset_passes(ctx.store, state)

    recovered = store.TaskStore(repo, "demo")
    rows = contract.load(recovered, digest)

    assert rows[0].passes is initial_passes
    assert recovered.read_state()["contract_sha256"] == digest


def _write_approved(ctx, rows):
    """The planner writes these rows and the person approves their commands (no command runs unapproved)."""
    flows = sorted({row["flow"] for row in rows})
    ctx.store.update_state(lambda state: state["approvals"].update(checks={"commands": flows}))
    ctx.store.contract.write_text(json.dumps({"rows": rows}))


FLOW_NEW = f"{sys.executable} -c \"import sys; sys.exit(1)\""
FLOW_OLD = f"{sys.executable} -c \"pass\""


APPROVED = {"verdict": "APPROVE", "findings": []}


def _approve_pending(ctx):
    """The person approves the commands that wait at the checks gate, as `sakura flow approve <task> checks` does."""
    pending = ctx.store.read_state()[checks.PENDING_KEY]
    ctx.store.update_state(lambda state: state["approvals"].update(
        checks={"sha256": pending["sha256"], "commands": pending["commands"], "flows": pending.get("flows", "")}))


def _phase_context(repo, tmp_path, new_flow, baseline_flow=FLOW_OLD, runner_rows=None, files=None, extra=None,
                   review=None):
    base = git(repo, "rev-parse", "HEAD")
    overrides = {
        "workspace": {"root": str(tmp_path / "trees" / "{repo}")},
        "tests": {"focused": ["true"]},
        "app": {"start": "", "ready_url": ""},
        **(extra or {}),
    }
    # The person approved these contract commands (S1 review, finding 1: no command runs unapproved).
    flows = [row["flow"] for row in runner_rows] if runner_rows else [new_flow, baseline_flow]
    state = {"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}, "contract_sha256": "",
             "protected_paths": [], "approvals": {"spec": {"sha256": "abc"}, "checks": {"commands": flows}}}
    holder = {}

    def write_contract(request):
        if request.role != "planner":
            return
        flows = request.cwd / "e2e"
        flows.mkdir(exist_ok=True)
        (flows / "new_flow.py").write_text("# flow\n")
        for name, content in (files or {}).items():
            (request.cwd / name).parent.mkdir(parents=True, exist_ok=True)
            (request.cwd / name).write_text(content)
        rows = runner_rows or [
            {"id": "R1", "journey": "J1", "step": "new", "kind": "cli", "viewport": None, "flow": new_flow,
             "baseline": False, "passes": False},
            {"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None, "flow": baseline_flow,
             "baseline": True, "passes": False},
        ]
        holder["ctx"].store.contract.write_text(json.dumps({"rows": rows}))

    runner = FakeRunner(write_contract, by_role={"code_reviewer": review or APPROVED})
    ctx = make_context(repo, own_repository=True, runner=runner, overrides=overrides, state=state)
    holder["ctx"] = ctx
    ctx.store.spec.write_text("## Journeys\n- J1\n")
    return ctx


def test_ContractPhase_TheFlowsCommitIsTheFirstStepOfTheEnginesHead(repo, tmp_path):
    # Review 11: what is built, tested and delivered starts from the engine's own head; the contract flows the engine
    # committed after the person's approval are its first step, checked against the store, never workspace 0's HEAD.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    assert contract_phase.run(ctx).kind == "next"
    state = ctx.store.read_state()
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    store, head = final_head.proved(ctx)
    assert head == git(workspace, "rev-parse", "HEAD") != state["base_commit"]
    assert git(store, "rev-parse", f"{head}^") == state["base_commit"]
    assert "e2e/new_flow.py" in git(store, "ls-tree", "-r", "--name-only", head).splitlines()


def test_ContractPhase_TheFlowsCommitIsCodeReviewedOnExactlyItsBytes(repo, tmp_path):
    # Review 13, finding 1: the flows commit gets its own code review, bound to the sealed base and that commit,
    # before it enters the engine's head; it does not wait for a build task to change something.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    assert contract_phase.run(ctx).kind == "next"
    state = ctx.store.read_state()
    head = final_head.proved(ctx)[1]
    receipt = {"base": state["base_commit"], "head": head, "verdict": "APPROVE"}
    reviews = [request for request in ctx.runner.requests if request.role == "code_reviewer"]
    assert len(reviews) == 1
    assert reviews[0].values["start"] == state["base_commit"] and reviews[0].values["branch"] == head
    assert state[final_head.FLOWS_REVIEW_KEY] == receipt
    assert state[final_head.STATE_KEY]["steps"][0]["review"] == receipt


def test_ContractPhase_AFlowsReviewThatAsksForChanges_IntegratesNothing(repo, tmp_path):
    # The first REVISE goes back to the planner (real run r17); the second blocks with its findings.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW,
                         review={"verdict": "REVISE", "findings": ["the flow posts the environment when CI is set"]})
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    state = ctx.store.read_state()
    assert outcome.kind == "blocked" and "the flow posts the environment when CI is set" in outcome.reason
    assert final_head.recorded(state) == state["base_commit"]
    assert final_head.FLOWS_REVIEW_KEY not in state
    assert len(_planner_requests(ctx)) == 2 and not state["contract_sha256"]


def _engine_flow(ctx, name="e2e/new_flow.py"):
    from sakura.flow import final_head

    store, head = final_head.proved(ctx)
    return git(store, "show", f"{head}:{name}")


REVISED = {"verdict": "REVISE", "findings": ["e2e/new_flow.py matches the function names as bare substrings"]}


def _planner_requests(ctx):
    return [request for request in ctx.runner.requests if request.role == "planner"]


def _reviews_in_turn(ctx, *answers):
    """The code reviewer gives `answers` one flows review after the other, and each planner run writes new bytes of
    the same flow file."""
    planner, answers = ctx.runner.behaviour, iter(answers)

    def behaviour(request):
        if request.role == "code_reviewer":
            ctx.runner.by_role["code_reviewer"] = next(answers)
            return
        planner(request)
        if request.role == "planner":
            (request.cwd / "e2e" / "new_flow.py").write_text(f"# flow, attempt {len(_planner_requests(ctx))}\n")

    ctx.runner.behaviour = behaviour


def test_ContractPhase_AFlowsReviewThatAsksForChanges_GoesBackToThePlannerOnce_AndItsNewFlowsAreSaved(repo, tmp_path):
    # Real run r17: a valid REVISE of the flows commit blocked the task, and nothing told the planner. Now its findings
    # go back to the planner once; the new flows wait for the person, then the base run and the review run again.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, APPROVED)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    sent_back = contract_phase.run(ctx)
    assert (sent_back.kind, sent_back.gate) == ("await", checks.APPROVAL_GATE)
    planners = _planner_requests(ctx)
    assert len(planners) == 2 and REVISED["findings"][0] in planners[1].values["previous_error"]
    assert git(workspace, "rev-parse", "HEAD") == ctx.store.read_state()["base_commit"]
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    state = ctx.store.read_state()
    reviews = [request for request in ctx.runner.requests if request.role == "code_reviewer"]
    assert len(reviews) == 2
    assert reviews[1].values["branch"] == state[contract_phase.FLOWS_COMMIT_KEY] != reviews[0].values["branch"]
    assert _engine_flow(ctx) == "# flow, attempt 2"
    assert git(workspace, "rev-parse", "HEAD") == final_head.proved(ctx)[1]
    assert state["contract_sha256"] and contract_phase.REVIEW_FINDING_KEY not in state
    assert len(_planner_requests(ctx)) == 2


def _blocked_after_the_second_review(ctx, workspace):
    """Two reviews that ask for changes: the first goes back to the planner, the second blocks the contract with the
    planner's second flows committed in workspace 0 (real proof 2, lanes A and D)."""
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    blocked = contract_phase.run(ctx)
    assert blocked.kind == "blocked" and REVISED["findings"][0] in blocked.reason
    return blocked


def test_ContractPhase_ResumedAfterABlockingReview_WorkspaceZeroIsCleanAndFollowsTheEnginesHead(repo, tmp_path):
    # D6: the blocked contract left the flows committed in workspace 0; the next planner edited that tracked file, the
    # checkout to the engine's head then failed over the edit, yet the phase said done and plan blocked for good.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, REVISED, APPROVED)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    _blocked_after_the_second_review(ctx, workspace)
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert [line for line in git(workspace, "status", "--porcelain").splitlines() if "e2e/" in line] == []
    assert git(workspace, "rev-parse", "HEAD") == final_head.proved(ctx)[1]
    assert "still shows another revision" not in ctx.store.ledger.read_text()


def test_ContractPhase_WorkspaceZeroCannotFollowTheEnginesHead_FailsVisiblyNeverDone(repo, tmp_path, monkeypatch):
    # The step may not say done over a failed checkout: it blocks, naming why.
    from sakura.flow import final_head
    from sakura.flow.gitutil import GitError

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)

    def refuse(*_args):
        raise GitError("error: your local changes would be overwritten by checkout")

    monkeypatch.setattr(final_head, "show", refuse)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "workspace 0 cannot be moved to the engine's head" in outcome.reason
    assert "local changes would be overwritten" in outcome.reason
    assert "contract" not in ctx.store.read_state().get("completed", {}) and not ctx.store.read_state()["contract_sha256"]


def test_ContractPhase_ResumedWorkspaceNotOnTheBase_StartsThePlannerFromTheBase(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, REVISED, APPROVED)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    _blocked_after_the_second_review(ctx, workspace)
    assert git(workspace, "rev-parse", "HEAD") != ctx.store.read_state()["base_commit"]
    contract_phase.run(ctx)
    assert git(workspace, "rev-parse", "HEAD") == ctx.store.read_state()["base_commit"]


def test_ContractPhase_AfterAFlowsReviewSendBack_TheOldCommandApprovalDoesNotApplyToTheNewFlows(repo, tmp_path):
    # The new flows run the very same commands, but their bytes are new: nothing the person approved for the old
    # flows runs them, and nothing of them runs before the person approved again.
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, APPROVED)
    approved = ctx.store.read_state()["approvals"][checks.APPROVAL_GATE]["commands"]
    outcome = contract_phase.run(ctx)
    state = ctx.store.read_state()
    assert (outcome.kind, outcome.gate) == ("await", checks.APPROVAL_GATE)
    assert checks.APPROVAL_GATE not in state["approvals"]
    assert sorted(state[checks.PENDING_KEY]["commands"]) == sorted(approved)
    reviewed = [request for request in ctx.runner.requests if request.role == "code_reviewer"]
    assert state[contract_phase.FLOWS_COMMIT_KEY] == reviewed[0].values["branch"]


def test_ContractPhase_TheReviewersFindingReachesThePlannerBoundedAndWithoutControlCharacters(repo, tmp_path):
    finding = ("\x1b]0;retitled\x07bare \x1b[31msubstrings\x1b[0m\x00\u202e"
               + "x" * (2 * contract_phase.FINDING_MAX_CHARS))
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, {"verdict": "REVISE", "findings": [finding]}, APPROVED)
    contract_phase.run(ctx)
    kept = ctx.store.read_state()[contract_phase.REVIEW_FINDING_KEY]
    told = _planner_requests(ctx)[1].values["previous_error"]
    assert len(kept) == contract_phase.FINDING_MAX_CHARS and kept in told
    assert "bare substrings" in kept and "retitled" not in told and "\u202e" not in told
    assert not [character for character in told if controls.is_control(character) and character not in "\n\t"]


def test_ContractPhase_AFlowsReviewThatAsksForChangesAfterTheBaseRunSentTheFlowsBack_Blocks(repo, tmp_path):
    # One attempt for both kinds of finding: the planner writes the contract at most twice.
    broken = f"{sys.executable} -c \"import sys; sys.exit(3)\""
    first = [{"id": "R1", "journey": "J1", "step": "new", "kind": "cli", "viewport": None, "flow": FLOW_NEW,
              "baseline": False, "passes": False},
             {"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None, "flow": broken,
              "baseline": True, "passes": False}]
    second = [dict(first[0]), {**first[1], "flow": FLOW_OLD}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=first, review=REVISED)
    attempts = iter([first, second])

    def write_contract(request):
        if request.role == "planner":
            (request.cwd / "e2e").mkdir(exist_ok=True)
            (request.cwd / "e2e" / "new_flow.py").write_text("# flow\n")
            ctx.store.contract.write_text(json.dumps({"rows": next(attempts)}))

    ctx.runner.behaviour = write_contract
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and REVISED["findings"][0] in outcome.reason
    assert len(_planner_requests(ctx)) == 2
    assert not ctx.store.read_state()["contract_sha256"]


def test_ContractPhase_ASurvivorMovingWorkspaceZeroAfterTheFlowsCommit_DoesNotChangeTheEnginesFlows(repo, tmp_path,
                                                                                                    monkeypatch):
    # Security review of review 11 (high): the engine's flows commit is not read from workspace 0's HEAD.
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    real = contract_phase._test_and_commit

    def then_a_survivor_moves_head(context, path, base, flow_files):
        real(context, path, base, flow_files)
        (path / "e2e" / "new_flow.py").write_text("# swapped in after the base run\n")
        git(path, "add", "e2e/new_flow.py")
        git(path, "commit", "-q", "--amend", "--no-verify", "-m", "test(flow): contract flows for demo")

    monkeypatch.setattr(contract_phase, "_test_and_commit", then_a_survivor_moves_head)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" or _engine_flow(ctx) == "# flow"
    if outcome.kind == "blocked":
        assert "changed" in outcome.reason


def test_ContractPhase_AFlowFileChangedAfterTheBaseRun_IsNeverCommittedToTheEnginesHead(repo, tmp_path, monkeypatch):
    # Review 12: the flows step is the commit the base run ran on, made from the bytes read before it; a flow
    # rewritten in workspace 0 afterwards is not what is committed (before, the rewrite was refused).
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    real = contract_phase._wrong_on_base

    def then_a_survivor_rewrites_the_flow(context, workspace, *args):
        problems = real(context, workspace, *args)
        (workspace.path / "e2e" / "new_flow.py").write_text("# rewritten after the base run\n")
        return problems

    monkeypatch.setattr(contract_phase, "_wrong_on_base", then_a_survivor_rewrites_the_flow)
    assert contract_phase.run(ctx).kind == "next"
    assert _engine_flow(ctx) == "# flow"


def test_ContractPhase_OnResumeTheFlowsAreTheOnesRecordedBeforeTheBaseRun_NotTheWorkspaceList(repo, tmp_path):
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    assert contract_phase.run(ctx).kind == "next"
    state = ctx.store.read_state()
    recorded = state["contract_flows"]
    assert set(recorded) == {"e2e/new_flow.py"}
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    (workspace / "app.py").write_text("planted\n")  # not a flow file: it must never become one
    ctx.store.update_state(lambda current: current.pop(final_head.STATE_KEY))
    ctx.store.update_state(lambda current: current.update(protected_paths=[]))
    contract_phase._integrate_flows(ctx, workspace)
    store, head = final_head.proved(ctx)
    names = git(store, "diff", "--name-only", state["base_commit"], head).splitlines()
    assert names == ["e2e/new_flow.py"]


def test_ContractPhase_AFlowsCommitWithAFileOutsideTheFlowFolders_IsNeverTheEnginesHead(repo, tmp_path):
    # Security review of review 11, and review 12: a source file must not reach the engine's head as a flow, also
    # when a commit holding one is recorded as the flows candidate.
    from sakura.flow import final_head

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    assert contract_phase.run(ctx).kind == "next"
    with pytest.raises(final_head.MergeBlocked, match="app.py"):
        final_head.flows_candidate(ctx, {"app.py": (b"print('x')\n", False)}, ["e2e/**"], "x")
    ctx.store.update_state(lambda state: state.pop(final_head.STATE_KEY))
    loose = final_head.flows_candidate(ctx, {"src/app.py": (b"print('x')\n", False)}, ["src/**"], "x")
    ctx.store.update_state(lambda state: state.update(contract_flows_commit=loose))
    with pytest.raises(contract_phase.CommitError, match="src/app.py"):
        contract_phase._integrate_flows(ctx, workspace)
    state = ctx.store.read_state()
    assert final_head.recorded(state) == state["base_commit"]


# -- review 12 --------------------------------------------------------------------------------------------------------

def _engine_names(ctx):
    from sakura.flow import final_head

    state = ctx.store.read_state()
    store, head = final_head.proved(ctx)
    return git(store, "diff", "--name-only", state["base_commit"], head).splitlines()


@pytest.mark.parametrize("name", [".github/workflows/x.spec.yml", "e2e/.github/workflows/ci.yml",
                                  "tests/conftest.py", "e2e/package.json"])
def test_ContractPhase_AFlowAtACiGitOrBuildPath_IsRefusedAndNeverCommitted(repo, tmp_path, name):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={name: "on: push\n"})
    outcome = contract_phase.run(ctx)
    assert outcome.kind in ("blocked", "failed") and name in outcome.reason
    assert name not in _engine_names(ctx)


def test_ContractPhase_ARepositoryWideningTestPatterns_DoesNotWidenWhereAFlowMayLive(repo, tmp_path):
    widened = [*flow_config.load(repo).values["brakes"]["test_patterns"], "**"]  # a superset: a tightening
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text(f"[brakes]\ntest_patterns = {widened!r}\n".replace("'", '"'))
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={"src/helper.py": "VALUE = 2\n"})
    outcome = contract_phase.run(ctx)
    assert outcome.kind in ("blocked", "failed") and "src/helper.py" in outcome.reason
    assert "src/helper.py" not in _engine_names(ctx)


def test_ContractPhase_AFlowCollidingIgnoringCaseWithATrackedPath_IsNeverCommitted(repo, tmp_path):
    (repo / "e2e").mkdir()
    (repo / "e2e" / "readme.md").write_text("tracked\n")
    git(repo, "add", "e2e/readme.md")
    git(repo, "commit", "-qm", "tracked e2e readme")
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={"e2e/README.md": "the planner's\n"})
    outcome = contract_phase.run(ctx)
    # On a case-insensitive volume the planner's file is the tracked one, changed; on a case-sensitive one a new file:
    # refused either way, and the engine's own check refuses the name whatever the volume.
    assert outcome.kind in ("blocked", "failed") and "e2e/readme.md" in outcome.reason.casefold()
    assert not {"e2e/README.md", "e2e/readme.md"} & set(_engine_names(ctx))
    from sakura.flow import final_head
    with pytest.raises(final_head.MergeBlocked, match="collides, ignoring case"):
        final_head.flows_candidate(ctx, {"e2e/README.md": (b"x\n", False)}, ["e2e/**"], "x")
    with pytest.raises(final_head.MergeBlocked, match="collides, ignoring case"):
        final_head.flows_candidate(ctx, {"e2e/a.py": (b"x\n", False), "e2e/A.py": (b"y\n", False)}, ["e2e/**"], "x")


def _with_app_source(repo):
    (repo / "app.py").write_text("VALUE = 1\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "app")


BASE_ROW = "grep -q 'VALUE = 1' app.py"


def test_ContractPhase_AnAppStartThatRewritesTrackedSource_HasNoEffectOnTheBaseline(repo, tmp_path):
    _with_app_source(repo)
    rewrite = "sh -c 'echo VALUE = 2 > app.py'"
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, baseline_flow=BASE_ROW,
                         extra={"app": {"start": rewrite, "ready_url": ""}})
    ctx.store.update_state(lambda state: state["approvals"]["checks"]["commands"].append(rewrite))
    outcome = contract_phase.run(ctx)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    assert (workspace / "app.py").read_text() == "VALUE = 1\n", "the app wrote the live workspace"
    if outcome.kind == "next":
        log = (ctx.store.evidence_dir / "contract-base" / "B1-api.log").read_text()
        assert baseline.EXIT_LINE.search(log).group(1) == "0"


def test_ContractPhase_ASurvivorChangingWorkspaceSourceDuringTheBaseRun_DoesNotChangeWhatIsProved(repo, tmp_path,
                                                                                                  monkeypatch):
    _with_app_source(repo)
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, baseline_flow=BASE_ROW)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    real = contract_phase.baseline.run_rows

    def survivor_then_rows(context, cwd, *args, **kwargs):
        (workspace / "app.py").write_text("VALUE = 2\n")
        return real(context, cwd, *args, **kwargs)

    monkeypatch.setattr(contract_phase.baseline, "run_rows", survivor_then_rows)
    outcome = contract_phase.run(ctx)
    log = (ctx.store.evidence_dir / "contract-base" / "B1-api.log").read_text()
    assert baseline.EXIT_LINE.search(log).group(1) == "0", "the base run read the survivor's bytes"
    # The changed workspace is then refused rather than committed; what was proved is the base's.
    assert outcome.kind == "next" or "changed during the base run" in outcome.reason
    assert "app.py" not in _engine_names(ctx)


def test_contract_phase_commits_flows_saves_contract_and_protects_paths(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    git(repo, "config", "user.name", "T")
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    state = ctx.store.read_state()
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    assert git(workspace, "log", "-1", "--format=%s") == "test(flow): contract flows for demo"
    assert "e2e/new_flow.py" in git(workspace, "show", "--name-only", "--format=", "HEAD").splitlines()
    assert state["contract_sha256"]
    assert "e2e/new_flow.py" in state["protected_paths"]
    assert str(ctx.store.contract) in state["protected_paths"]
    assert (ctx.store.runs_dir / "operations" / "contract-flow-R1.raw").is_file()
    assert not list(ctx.store.runs_dir.glob("*.log"))
    log = (ctx.store.evidence_dir / "contract-base" / "R1-api.log").read_text()
    assert baseline.EXIT_LINE.search(log).group(1) == "1"


@pytest.mark.parametrize("interrupt_at", ["before_replace", "after_replace"])
def test_contract_phase_recovers_initial_save_before_replanning(repo, tmp_path, monkeypatch, interrupt_at):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    git(repo, "config", "user.name", "T")
    original_replace = contract._replace_contract
    if interrupt_at == "before_replace":
        def interrupt_replace(*_args):
            raise RuntimeError("before replace")

        monkeypatch.setattr(contract, "_replace_contract", interrupt_replace)
    else:
        def interrupt_after_replace(task_store, payload):
            original_replace(task_store, payload)
            raise RuntimeError("after replace")

        monkeypatch.setattr(contract, "_replace_contract", interrupt_after_replace)

    with pytest.raises(RuntimeError, match="replace"):
        contract_phase.run(ctx)
    planner_calls = len(ctx.runner.requests)
    monkeypatch.setattr(contract, "_replace_contract", original_replace)
    object.__setattr__(ctx, "store", store.TaskStore(repo, "demo"))

    outcome = contract_phase.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert len(ctx.runner.requests) == planner_calls
    state = ctx.store.read_state()
    assert contract.load(ctx.store, state["contract_sha256"])
    assert "e2e/new_flow.py" in state["protected_paths"]


def test_crash_between_state_and_unlink_recovers(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    git(repo, "config", "user.name", "T")
    stale_base = ctx.store.evidence_dir / "contract-base" / "STALE-api.log"
    stale_base.parent.mkdir(parents=True)
    stale_base.write_text("evidence of the archived contract\n")
    ctx.store.contract.write_text('{"rows": []}')
    ctx.store.update_state(lambda state: state.update(completed={"contract": "recontract-authorized"}))

    first = contract_phase.run(ctx)
    # Requirement change (review 17, finding 5): the replaced contract takes the approval of its commands with it.
    assert (first.kind, first.gate) == ("await", checks.APPROVAL_GATE)
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert not stale_base.exists()
    state = ctx.store.read_state()
    assert [row.id for row in contract.load(ctx.store, state["contract_sha256"])] == ["R1", "B1"]


def test_contract_phase_fails_when_new_flow_passes_on_base(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_OLD)
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "failed"
    assert "R1" in outcome.reason
    assert ctx.store.read_state()["contract_sha256"] == ""


def test_contract_phase_fails_when_baseline_flow_fails_on_base(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, baseline_flow=FLOW_NEW)
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "failed" and "B1" in outcome.reason


def test_ContractPhase_FlowsWrongOnTheBaseBuild_GoBackToThePlannerOnceWithWhatTheBaseRunShowed(repo, tmp_path):
    broken = f"{sys.executable} -c \"print('fake driver broke'); import sys; sys.exit(3)\""
    first = [{"id": "R1", "journey": "J1", "step": "new", "kind": "cli", "viewport": None, "flow": FLOW_NEW,
              "baseline": False, "passes": False},
             {"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None, "flow": broken,
              "baseline": True, "passes": False}]
    second = [dict(first[0]), {**first[1], "flow": FLOW_OLD}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=first)
    ctx.store.update_state(lambda state: state["approvals"].update(checks={"commands": [FLOW_NEW, FLOW_OLD, broken]}))
    attempts = iter([first, second])

    def write_contract(_request):
        ctx.store.contract.write_text(json.dumps({"rows": next(attempts)}))

    ctx.runner.behaviour = write_contract
    sent_back = contract_phase.run(ctx)
    # New flow bytes need a new approval, even for the very commands the person approved for the old ones.
    assert (sent_back.kind, sent_back.gate) == ("await", checks.APPROVAL_GATE)
    assert checks.APPROVAL_GATE not in ctx.store.read_state()["approvals"]
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert len(ctx.runner.requests) == 2
    told = ctx.runner.requests[1].values["previous_error"]
    assert "B1 (baseline) fails on the base build but must pass" in told and "fake driver broke" in told
    assert [row.flow for row in contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])] == \
        [FLOW_NEW, FLOW_OLD]
    assert contract_phase.BASE_FINDING_KEY not in ctx.store.read_state()


def test_ContractPhase_FlowsWrongOnTheBaseBuildTwice_Fail(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, baseline_flow=FLOW_NEW)
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "failed" and "B1 (baseline) fails on the base build" in outcome.reason
    assert len(ctx.runner.requests) == 2


def test_contract_phase_fails_without_any_new_row(repo, tmp_path):
    only_baseline = [{"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None,
                      "flow": FLOW_OLD, "baseline": True, "passes": False}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=only_baseline)
    outcome = contract_phase.run(ctx)
    # blocked, not failed: the person may resume it; the words are plain (round 6, N13) and the raw line is in the ledger
    assert outcome.kind == "blocked" and outcome.reason.startswith("The contract found nothing new to prove")
    assert "non-baseline" in ctx.store.ledger.read_text()


def test_contract_phase_reruns_planner_once_then_blocks(repo, tmp_path):
    bad = [{"id": "R1", "journey": "J1", "step": "x", "kind": "web", "viewport": None, "flow": "true",
            "baseline": False, "passes": False}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=bad)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "viewport" in outcome.reason
    assert len(ctx.runner.requests) == contract_phase.MAX_CONTRACT_ATTEMPTS
    assert "viewport" in ctx.runner.requests[1].values["previous_error"]


def test_contract_phase_blocks_missing_required_rows_before_base_proof(repo, tmp_path, monkeypatch):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.config.values["contract"]["required_ids"] = ["R15", "R16"]
    app_starts = []
    monkeypatch.setattr(contract_phase.stack, "start", lambda *_args: app_starts.append(True))

    outcome = contract_phase.run(ctx)

    assert outcome.kind == "blocked"
    assert "R15, R16" in outcome.reason
    assert len(ctx.runner.requests) == contract_phase.MAX_CONTRACT_ATTEMPTS
    assert app_starts == []


def test_contract_phase_rejects_helper_outside_test_patterns(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={"helper.py": "x = 1\n"})
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "helper.py" in outcome.reason
    assert "helper.py" in ctx.runner.requests[1].values["previous_error"]


def test_contract_phase_rejects_edit_of_existing_tracked_file(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={"README": "changed\n"})
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "README" in outcome.reason and "existing file" in outcome.reason


def test_contract_phase_commits_every_new_flow_file_and_protects_them(repo, tmp_path):
    files = {"e2e/sub/second.py": "# 2\n", "tests/test_extra.py": "# 3\n"}
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files=files)
    assert contract_phase.run(ctx).kind == "next"
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    committed = git(workspace, "show", "--name-only", "--format=", "HEAD").splitlines()
    expected = {"e2e/new_flow.py", "e2e/sub/second.py", "tests/test_extra.py"}
    assert expected <= set(committed)
    assert git(workspace, "status", "--porcelain", "--untracked-files=all").count("e2e/") == 0
    assert expected <= set(ctx.store.read_state()["protected_paths"])


def test_contract_phase_runs_focused_tests_even_without_new_flow_files(repo, tmp_path):
    # The focused tests run in the cage, which writes only in the workspace, the task repository's Git folder and its tmp.
    marker = tmp_path / "trees" / "repo" / "demo.git" / "focused.ran"
    (repo / ".sakura").mkdir()
    (repo / ".sakura/flow.toml").write_text(f'[tests]\nfocused = ["touch {marker}"]\n')
    git(repo, "add", ".sakura/flow.toml")
    git(repo, "commit", "-qm", "configure focused tests")
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, extra={"tests": {"focused": [f"touch {marker}"]}})
    rows = [{"id": "R1", "journey": "J", "step": "s", "kind": "cli", "viewport": None, "flow": FLOW_NEW,
             "baseline": False, "passes": False}, {"id": "B1", "journey": "J", "step": "s", "kind": "cli",
             "viewport": None, "flow": FLOW_OLD, "baseline": True, "passes": False}]
    ctx.runner.behaviour = lambda request: _write_approved(ctx, rows)
    assert contract_phase.run(ctx).kind == "next"
    assert marker.exists()
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    assert git(workspace, "log", "-1", "--format=%s") == "configure focused tests"


def test_contract_phase_zero_new_rows_fails_even_with_other_errors(repo, tmp_path):
    rows = [{"id": "B1", "journey": "J", "step": "s", "kind": "web", "viewport": None, "flow": FLOW_OLD,
             "baseline": True, "passes": False}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=rows)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and outcome.reason.startswith("The contract found nothing new to prove")


def _interrupt_rows(flow):
    return [{"id": "R1", "journey": "J", "step": "s", "kind": "cli", "viewport": None, "flow": flow,
             "baseline": False, "passes": False}, {"id": "B1", "journey": "J", "step": "s", "kind": "cli",
             "viewport": None, "flow": FLOW_OLD, "baseline": True, "passes": False}]


def test_contract_phase_stop_during_flow_returns_stopped(repo, tmp_path):
    # Flows run caged and cannot write the task folder, so the operator's STOP arrives from outside.
    import threading

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.runner.behaviour = lambda request: _write_approved(ctx, _interrupt_rows("sleep 30"))
    stopper = threading.Timer(2.0, ctx.store.stop_file.touch)
    stopper.start()
    outcome = contract_phase.run(ctx)
    stopper.join()
    assert outcome.kind == "stopped" and "R1" in outcome.reason
    assert ctx.store.read_state()["contract_sha256"] == ""


def test_contract_phase_stalled_flow_blocks_naming_the_row(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, extra={"timeouts": {"idle_seconds": 1}})
    ctx.runner.behaviour = lambda request: _write_approved(ctx, _interrupt_rows("sleep 30"))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "R1" in outcome.reason and "stalled" in outcome.reason


def test_contract_phase_timed_out_flow_blocks_naming_the_row(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, extra={"timeouts": {"contract": 3}})
    chatty = f"{sys.executable} -c \"import time; [(print('x', flush=True), time.sleep(0.2)) for _ in range(60)]\""
    ctx.runner.behaviour = lambda request: _write_approved(ctx, _interrupt_rows(chatty))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "R1" in outcome.reason and "timed out" in outcome.reason


@pytest.mark.parametrize("changes, fragment", [
    ({"id": "../evil"}, "row id"),
    ({"id": "a" * 41}, "row id"),
    ({"viewport": "1440x900/../x"}, "viewport"),
    ({"viewport": "huge"}, "viewport"),
    ({"kind": "api", "viewport": "1440x900"}, "null"),
    ({"baseline": "false"}, "baseline"),
])
def test_contract_validate_rejects_unsafe_or_mistyped_fields(repo, changes, fragment):
    errors = contract.validate([_row(**changes), _row(id="B1", baseline=True)], _cfg(repo), STEPS)
    assert any(fragment in error for error in errors), errors


def test_contract_load_treats_empty_expected_hash_as_a_mismatch(repo):
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo"})
    contract.save(task_store, _rows())
    with pytest.raises(contract.ContractTampered):
        contract.load(task_store, "")


def test_baseline_refuses_paths_that_escape_their_directory(tmp_path):
    from sakura.flow import baseline

    with pytest.raises(contract.ContractError):
        baseline._inside(tmp_path, "../escape.log")
    assert baseline._inside(tmp_path, "R1-api.log") == tmp_path / "R1-api.log"


def _rows_with_new_flow(flow):
    return [{"id": "R1", "journey": "J", "step": "s", "kind": "cli", "viewport": None, "flow": flow,
             "baseline": False, "passes": False}, {"id": "B1", "journey": "J", "step": "s", "kind": "cli",
             "viewport": None, "flow": FLOW_OLD, "baseline": True, "passes": False}]


def _tracked_app(repo):
    (repo / "app.py").write_text("print('app')\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")


def test_contract_phase_blocks_when_a_flow_modifies_app_code_during_base_run(repo, tmp_path):
    _tracked_app(repo)
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    flow = f"echo tampered >> app.py; {FLOW_NEW}"
    ctx.runner.behaviour = lambda request: _write_approved(ctx, _rows_with_new_flow(flow))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "app.py" in outcome.reason
    assert ctx.store.read_state()["contract_sha256"] == ""


def test_contract_phase_does_not_commit_a_file_a_flow_staged(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    flow = f"echo x > extra.txt; git add extra.txt; {FLOW_NEW}"
    ctx.runner.behaviour = lambda request: _write_approved(ctx, _rows_with_new_flow(flow))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "extra.txt" in outcome.reason
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    assert "extra.txt" not in git(workspace, "log", "--name-only", "--format=", "feat/flow-demo")


def test_contract_commit_guard_rejects_unvetted_staged_file(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next"
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    (workspace / "sneak.txt").write_text("x")
    git(workspace, "add", "sneak.txt")
    with pytest.raises(contract_phase.CommitError, match="sneak.txt"):
        contract_phase.check_staged(workspace, git(repo, "rev-parse", "HEAD"), ["e2e/new_flow.py"])


# -- a check runs in a read-only copy of the committed files: a command that plainly writes there is refused up front --


@pytest.mark.parametrize("flow", [
    "python3 e2e/check.py > e2e/result.txt",
    "sh e2e/check.sh >> result.txt",
    "python3 e2e/check.py 2> ./errors.log",
    "python3 e2e/check.py | tee e2e/result.txt",
    "python3 e2e/check.py | tee -a result.txt",
    "touch e2e/ready && python3 e2e/check.py",
    "python3 e2e/check.py >'out dir/result.txt'",
])
def test_contract_validate_rejects_a_command_that_writes_inside_the_checkout(repo, flow):
    errors = contract.validate([_row(flow=flow, kind="cli", viewport=None), _row(id="B1", baseline=True)],
                               _cfg(repo), STEPS)
    assert len(errors) == 1
    assert "R1" in errors[0] and "read-only" in errors[0] and "$TMPDIR" in errors[0]


@pytest.mark.parametrize("flow", [
    "python3 e2e/check.py > /dev/null",
    "python3 e2e/check.py 2>&1",
    "python3 e2e/check.py > \"$TMPDIR/result.txt\"",
    "python3 e2e/check.py > ${SAKURA_FLOW_BUILD_DIR}/result.txt",
    "python3 e2e/check.py | tee \"$TMPDIR/result.txt\"",
    "python3 e2e/check.py > /tmp/result.txt",
    "test 3 -gt 2 && python3 e2e/check.py",
    "python3 -c 'print(1 > 0)'",
])
def test_contract_validate_accepts_a_command_that_writes_only_to_the_run_temp_folder(repo, flow):
    assert contract.validate([_row(flow=flow, kind="cli", viewport=None), _row(id="B1", baseline=True)],
                             _cfg(repo), STEPS) == []


def test_contract_validate_leaves_an_owner_rows_sentence_alone(repo):
    rows = [_row(prover="owner", kind="cli", viewport=None, flow="Try it > then write down what you see"),
            _row(id="B1", baseline=True)]
    assert contract.validate(rows, _cfg(repo), STEPS) == []


def _flows_workspace(repo, tmp_path):
    """Workspace 0 of a contract phase, at the base, for the planner's flow files."""
    from sakura.flow import workspaces

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    state = ctx.store.read_state()
    return ctx, workspaces.create(ctx, "0", state["branch"], state["base_commit"]).path


def _spy_reads(monkeypatch):
    """Every flow read the engine makes, with the limit it read under; an unbounded read fails the test."""
    from sakura import beneath

    reads = []
    real, real_unbounded = beneath.read_capped, beneath.read_bytes

    def capped(root, relative, limit):
        if relative.startswith("e2e/"):
            reads.append((relative, limit))
        return real(root, relative, limit)

    def unbounded(root, relative):
        if relative.startswith("e2e/"):
            raise AssertionError("a flow file was read without a bound")
        return real_unbounded(root, relative)

    monkeypatch.setattr(contract_phase.beneath, "read_capped", capped)
    monkeypatch.setattr(contract_phase.beneath, "read_bytes", unbounded)
    return reads


def test_RecordFlows_AFlowFileOverThePerFileLimit_IsRefusedBeforeItIsRead(repo, tmp_path, monkeypatch):
    # Review 13, finding 3: a huge or sparse flow file never reaches the engine's memory.
    ctx, path = _flows_workspace(repo, tmp_path)
    reads = _spy_reads(monkeypatch)
    (path / "e2e").mkdir()
    with open(path / "e2e/huge.py", "wb") as sparse:
        sparse.truncate(contract_phase.flow_paths.MAX_FLOW_FILE_BYTES + 1)

    with pytest.raises(contract_phase.CommitError, match=r"e2e/huge\.py .*per-file limit of 1048576 bytes"):
        contract_phase._record_flows(ctx, path)
    assert all(limit <= contract_phase.flow_paths.MAX_FLOW_FILE_BYTES for _name, limit in reads)
    assert contract_phase.FLOWS_COMMIT_KEY not in ctx.store.read_state()


def test_RecordFlows_TooManyFlowFiles_AreRefusedBeforeAnyIsRead(repo, tmp_path, monkeypatch):
    ctx, path = _flows_workspace(repo, tmp_path)
    reads = _spy_reads(monkeypatch)
    monkeypatch.setattr(contract_phase.flow_paths, "MAX_FLOW_FILES", 3)
    (path / "e2e").mkdir()
    for number in range(4):
        (path / f"e2e/flow_{number}.py").write_text("# flow\n")

    with pytest.raises(contract_phase.CommitError, match="4 files, more than the limit of 3"):
        contract_phase._record_flows(ctx, path)
    assert reads == []


def test_RecordFlows_FlowsOverTheTotalLimit_AreRefusedNamingTheFileThatCrossedIt(repo, tmp_path, monkeypatch):
    ctx, path = _flows_workspace(repo, tmp_path)
    reads = _spy_reads(monkeypatch)
    monkeypatch.setattr(contract_phase.flow_paths, "MAX_FLOW_TOTAL_BYTES", 10)
    (path / "e2e").mkdir()
    (path / "e2e/a.py").write_text("123456\n")
    (path / "e2e/b.py").write_text("123456\n")

    with pytest.raises(contract_phase.CommitError, match=r"e2e/b\.py .*total limit of 10 bytes"):
        contract_phase._record_flows(ctx, path)
    assert reads == [("e2e/a.py", 10), ("e2e/b.py", 3)]


def test_ContractPhase_ResumeAfterACrashThatLeftADiscardedCandidate_DoesNotKeepTheApprovalForNewBytes(repo, tmp_path):
    # Review 17, finding 5: the person approved the commands of a candidate, the pending record was settled, and the
    # engine died before the contract was saved. The resume replans, so the approval of the old candidate is gone and
    # the new flow bytes wait for the person again, however identical their commands are.
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    assert checks.APPROVAL_GATE in ctx.store.read_state()["approvals"]
    ctx.store.contract.write_text(json.dumps({"rows": []}))

    outcome = contract_phase.run(ctx)

    state = ctx.store.read_state()
    assert (outcome.kind, outcome.gate) == ("await", checks.APPROVAL_GATE)
    assert checks.APPROVAL_GATE not in state["approvals"]
    assert state["approvals"]["spec"], "only the commands' approval is dropped"
    assert state["contract_sha256"] == ""


def _approved_then_resumed(ctx, planner_writes=None):
    """The person approved the commands of a candidate (the approval records the flow bytes it was given for), the pending
    record was settled and the engine died before the contract was saved; the lane is resumed and plans again, the
    planner writing `planner_writes` into the flow file when given."""
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    ctx.store.update_state(lambda state: state.pop(checks.PENDING_KEY))
    if planner_writes:
        writes_contract = ctx.runner.behaviour

        def replans(request):
            writes_contract(request)
            if request.role == "planner":
                (request.cwd / "e2e" / "new_flow.py").write_text(planner_writes)

        ctx.runner.behaviour = replans
    return contract_phase.run(ctx)


def test_ContractPhase_ResumeWithTheSameCommandsAndTheSameFlowBytes_KeepsTheApproval(repo, tmp_path):
    # Requirement change (round 6, N19, partly reverting the first try): an approval holds across a resume while both the
    # commands and the flow bytes they run are the ones it was given for.
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.store.update_state(lambda state: state["approvals"].pop(checks.APPROVAL_GATE))

    outcome = _approved_then_resumed(ctx)

    assert outcome.kind == "next", outcome.reason
    assert checks.APPROVAL_GATE in ctx.store.read_state()["approvals"]


def test_ContractPhase_ResumeWithTheSameCommandsBehindNewFlowBytes_AsksAgain(repo, tmp_path):
    # Review 17, finding 5 stands: new bytes behind the very same command lines are code the person never read.
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.store.update_state(lambda state: state["approvals"].pop(checks.APPROVAL_GATE))

    outcome = _approved_then_resumed(ctx, planner_writes="# a different flow\n")

    state = ctx.store.read_state()
    assert (outcome.kind, outcome.gate) == ("await", checks.APPROVAL_GATE)
    assert checks.APPROVAL_GATE not in state["approvals"] and state["approvals"]["spec"]


def test_ContractPhase_ResumeOfALaneWhoseNewCandidateRunsAnotherCommand_AsksAgainThoughTheFlowBytesAreTheSame(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.store.update_state(lambda state: state["approvals"].pop(checks.APPROVAL_GATE))
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)
    ctx.store.update_state(lambda state: state["approvals"][checks.APPROVAL_GATE].update(commands=[FLOW_OLD]))
    ctx.store.update_state(lambda state: state.pop(checks.PENDING_KEY))

    outcome = contract_phase.run(ctx)

    state = ctx.store.read_state()
    assert (outcome.kind, outcome.gate) == ("await", checks.APPROVAL_GATE)
    assert FLOW_NEW in state[checks.PENDING_KEY]["commands"]


def test_ContractPhase_APlannerRetryWithIdenticalContractAndCommands_IsAnotherRevisionOfTheGate(repo, tmp_path):
    # Review 18, finding 2: a confirmation issued for candidate A must not approve candidate B, even when B's
    # contract.json and commands are the same bytes and only the flow file differs.
    from sakura.flow import waits

    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, APPROVED)
    ctx.store.update_state(lambda state: state["approvals"].pop(checks.APPROVAL_GATE))
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    first = ctx.store.read_state()
    seen = waits.approval_revision(ctx.store, first, checks.APPROVAL_GATE)
    first_contract = ctx.store.contract.read_bytes()
    _approve_pending(ctx)

    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE  # the flows review sent it back; the planner wrote B

    second = ctx.store.read_state()
    assert ctx.store.contract.read_bytes() == first_contract
    assert second[checks.PENDING_KEY]["commands"] == first[checks.PENDING_KEY]["commands"]
    assert waits.approval_revision(ctx.store, second, checks.APPROVAL_GATE) != seen


# -- the recovery resets only a head the engine made (review 25, finding 4) ----------------------------------------------------


def _blocked_workspace(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, REVISED, REVISED, APPROVED)
    workspace = tmp_path / "trees" / "repo" / "demo-0"
    _blocked_after_the_second_review(ctx, workspace)
    return ctx, workspace


def _resumes_blocked(ctx):
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked", outcome
    return outcome


def test_ContractPhase_ResumeOnAPersonalCommit_ResetsNothingAndSaysSo(repo, tmp_path):
    ctx, workspace = _blocked_workspace(repo, tmp_path)
    (workspace / "mine.txt").write_text("my own work\n")
    git(workspace, "add", "mine.txt")
    git(workspace, "commit", "-q", "-m", "my commit")
    mine = git(workspace, "rev-parse", "HEAD").strip()

    outcome = _resumes_blocked(ctx)

    assert git(workspace, "rev-parse", "HEAD").strip() == mine and (workspace / "mine.txt").read_text() == "my own work\n"
    assert "not a commit the engine made" in outcome.reason and mine[:12] in outcome.reason


def test_ContractPhase_ResumeOnAnotherBranch_ResetsNoBranch(repo, tmp_path):
    ctx, workspace = _blocked_workspace(repo, tmp_path)
    branch = ctx.store.read_state()["branch"]
    engine_head = git(workspace, "rev-parse", "HEAD").strip()
    git(workspace, "checkout", "-q", "-b", "personal")
    (workspace / "mine.txt").write_text("x\n")
    git(workspace, "add", "mine.txt")
    git(workspace, "commit", "-q", "-m", "mine")
    personal = git(workspace, "rev-parse", "HEAD").strip()

    outcome = _resumes_blocked(ctx)

    assert git(workspace, "rev-parse", "personal").strip() == personal
    assert git(workspace, "rev-parse", branch).strip() == engine_head
    assert "personal" in outcome.reason and branch in outcome.reason


def test_ContractPhase_ResumeWithStagedChanges_KeepsTheIndexAndSaysSo(repo, tmp_path):
    ctx, workspace = _blocked_workspace(repo, tmp_path)
    (workspace / "staged.txt").write_text("keep me\n")
    git(workspace, "add", "staged.txt")
    head = git(workspace, "rev-parse", "HEAD").strip()

    outcome = _resumes_blocked(ctx)

    assert git(workspace, "rev-parse", "HEAD").strip() == head
    assert "staged.txt" in git(workspace, "diff", "--cached", "--name-only") and "staged" in outcome.reason


def test_ContractPhase_ResetMovesTheBranchOnlyFromTheHeadTheEngineLeft_NotFromOneThatMovedMeanwhile(repo, tmp_path, monkeypatch):
    ctx, workspace = _blocked_workspace(repo, tmp_path)
    branch = ctx.store.read_state()["branch"]
    mine = None

    real_git = contract_phase.git

    def moved_after_the_checks(path, *args, **kwargs):
        nonlocal mine
        if args[:1] == ("update-ref",) and mine is None:
            (workspace / "mine.txt").write_text("x\n")
            real_git(workspace, "add", "mine.txt")
            real_git(workspace, "commit", "-q", "-m", "mine, a moment before the reset")
            mine = real_git(workspace, "rev-parse", "HEAD").strip()
        return real_git(path, *args, **kwargs)

    monkeypatch.setattr(contract_phase, "git", moved_after_the_checks)
    outcome = contract_phase.run(ctx)

    assert outcome.kind == "blocked" and mine and git(workspace, "rev-parse", branch).strip() == mine


def test_ContractPhase_TheReviewOfTheContractIsTheStepPlanReview(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    _reviews_in_turn(ctx, APPROVED)

    contract_phase.run(ctx)

    reviews = [request for request in ctx.runner.requests if request.role == "code_reviewer"]
    assert reviews and all(request.step == "plan_review" for request in reviews), "it says which review it is"


@pytest.mark.parametrize("is_consumed", [False, True])
def test_contract_skips_next_flows_review_after_plan_review_is_turned_off(repo, tmp_path, monkeypatch, is_consumed):
    monkeypatch.setattr(ports, "is_free", lambda _port: True)
    monkeypatch.setattr(sealed, "_upload_pack", lambda _source, _common: "git upload-pack --strict")
    monkeypatch.setattr(contract_phase.workspaces, "_prepare", lambda *_args: None)
    monkeypatch.setattr(contract_phase, "_wrong_on_base", lambda *_args: [])
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.store.update_state(lambda state: state.update(phase="contract"))
    _reviews_in_turn(ctx, REVISED, REVISED)
    write = ctx.runner.behaviour

    def turn_off_in_second_round(request):
        write(request)
        if request.role == "planner" and len(_planner_requests(ctx)) == 2:
            inbox.enqueue(ctx.store, "step", "off", role="plan_review", actor="app:owner")
            if is_consumed:
                assert inbox.consume(ctx.store, ctx.repo, "contract") == 1

    ctx.runner.behaviour = turn_off_in_second_round
    assert contract_phase.run(ctx).gate == checks.APPROVAL_GATE
    _approve_pending(ctx)

    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert len([request for request in ctx.runner.requests if request.role == "code_reviewer"]) == 1
    assert "step plan_review turned off" in ctx.store.ledger.read_text()
    assert ctx.steps.code_review and not ctx.steps.plan_review
    assert final_head.proved(ctx)[1] == ctx.store.read_state()[contract_phase.FLOWS_COMMIT_KEY]


# -- a writer that ends without writing its file says so in plain words, with its own last message

SAID = "The --version option already exists in measure.py, so there is nothing new for me to prove."


def _writer_writes_nothing(repo, tmp_path, said=SAID):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    ctx.runner.behaviour = None
    ctx.runner.final = said
    return ctx


def test_ContractPhase_TheWriterEndsWithoutWritingTheContract_SaysSoInPlainWordsWithItsOwnLastMessage(repo, tmp_path):
    ctx = _writer_writes_nothing(repo, tmp_path)

    outcome = contract_phase.run(ctx)

    assert outcome.kind == "blocked"
    assert outcome.reason.startswith("The planner ended without writing the contract. It said: ")
    assert SAID in outcome.reason and outcome.reason.endswith("Resume the lane to ask it again, or stop it.")
    for engine_word in ("contract.json", "Errno", "No such file", "attempts"):
        assert engine_word not in outcome.reason
    assert "contract invalid after 2 attempts: contract.json cannot be read" in ctx.store.ledger.read_text(), "the raw error stays in the ledger"


def test_ContractPhase_TheWriterEndsWithoutWritingAndWithoutSayingAnything_SaysThatToo(repo, tmp_path):
    ctx = _writer_writes_nothing(repo, tmp_path, said="")

    outcome = contract_phase.run(ctx)

    assert outcome.reason == ("The planner ended without writing the contract and gave no reason. "
                              "Resume the lane to ask it again, or stop it.")


def test_ContractPhase_NothingNewToProve_QuotesWhatThePlannerSaidAtTheEnd(repo, tmp_path):
    only_baseline = [{"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None,
                      "flow": FLOW_OLD, "baseline": True, "passes": False}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=only_baseline)
    ctx.runner.final = "The --version option already exists in measure.py, so there is nothing new to prove."

    outcome = contract_phase.run(ctx)

    assert outcome.reason == ("The contract found nothing new to prove: the project may already do what the lane asks. The "
                              'planner said: "The --version option already exists in measure.py, so there is nothing new to '
                              'prove." Resume the lane with a note on what to prove, or stop it.')


def test_ContractPhase_NothingNewToProveAndTheWriterSaidNothing_KeepsTheSentenceWithoutAQuote(repo, tmp_path):
    only_baseline = [{"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None,
                      "flow": FLOW_OLD, "baseline": True, "passes": False}]
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, runner_rows=only_baseline)

    assert contract_phase.run(ctx).reason == ask_words.NOTHING_NEW_WORDS


def test_AwaitCommandApproval_WithTheChecksStepOff_AsksNoQuestionAndKeepsNoWaitingRecord(repo):
    ctx = make_context(repo, overrides={"preset": "lean", "relax": ["preset"]})
    assert ctx.config.values["knobs"]["checks"] is False
    ctx.store.contract.write_text('{"rows": []}')
    assert contract_phase._await_command_approval(ctx, [_row(flow="make check")]) is None
    assert checks.PENDING_KEY not in ctx.store.read_state()


def test_AwaitCommandApproval_WithTheChecksStepOn_StillAsks(repo):
    ctx = make_context(repo, overrides={"preset": "verified", "relax": ["preset"]})
    ctx.store.contract.write_text('{"rows": []}')
    waiting = contract_phase._await_command_approval(ctx, [_row(flow="make check")])
    assert waiting is not None and waiting.kind == "await" and "make check" in ctx.store.read_state()[checks.PENDING_KEY]["commands"]


def test_ContractPhase_WithTheChecksStepOff_RunsNoCheckOnTheBaseBuildAndAsksNothing(repo, tmp_path, monkeypatch):
    ran = []
    monkeypatch.setattr(contract_phase, "_wrong_on_base", lambda *args, **kwargs: ran.append(1) or [])
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, extra={"preset": "lean", "relax": ["preset"]})
    ctx.store.update_state(lambda state: state["approvals"].pop("checks"))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next" and ran == []
    assert "wait for the person's approval" not in ctx.store.ledger.read_text()


def _writer_that_first_writes_outside_then_inside(ctx, new_flow=FLOW_NEW):
    """A stubbed planner: its first attempt leaves a fixture outside the flow folders, its second moves it inside."""
    rows = [{"id": "R1", "journey": "J1", "step": "new", "kind": "cli", "viewport": None, "flow": new_flow,
             "baseline": False, "passes": False},
            {"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None, "flow": FLOW_OLD,
             "baseline": True, "passes": False}]
    attempts = []

    def write(request):
        if request.role != "planner":
            return
        attempts.append(request)
        (request.cwd / "e2e").mkdir(exist_ok=True)
        (request.cwd / "e2e" / "new_flow.py").write_text("# flow\n")
        if len(attempts) == 1:
            (request.cwd / "fixtures").mkdir(exist_ok=True)
            (request.cwd / "fixtures" / "data.txt").write_text("sample\n")
        else:
            (request.cwd / "fixtures" / "data.txt").unlink()
            (request.cwd / "fixtures").rmdir()
            (request.cwd / "e2e" / "data.txt").write_text("sample\n")
        ctx.store.contract.write_text(json.dumps({"rows": rows}))

    ctx.runner.behaviour = write
    return attempts


def test_ContractPhase_AFixtureOutsideTheFlowFolders_IsFixedByTheWriterOnTheAutomaticRetryAndTheLaneGoesOn(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    attempts = _writer_that_first_writes_outside_then_inside(ctx)

    outcome = contract_phase.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert len(attempts) == 2
    told = attempts[1].values["previous_error"]
    assert "fixtures/data.txt" in told and "tests/**, test/**, e2e/**" in told
    assert "move" in told and "delete" in told  # in words: what to do with each file
    assert "contract invalid" not in ctx.store.ledger.read_text()


def test_ContractPhase_TheWriterIsToldTheExactAllowedFoldersInItsPromptFromTheFirstAttempt(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW)
    attempts = _writer_that_first_writes_outside_then_inside(ctx)
    contract_phase.run(ctx)

    first = attempts[0]
    assert first.values["flow_folders"] == "tests/**, test/**, e2e/**"
    assert "$flow_folders" in first.template and "fixture" in first.template


def test_ContractPhase_AWriterThatKeepsWritingOutside_StopsForThePersonAfterTheRetry(repo, tmp_path):
    ctx = _phase_context(repo, tmp_path, FLOW_NEW, files={"fixtures/data.txt": "sample\n"})
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and len(ctx.runner.requests) == contract_phase.MAX_CONTRACT_ATTEMPTS
