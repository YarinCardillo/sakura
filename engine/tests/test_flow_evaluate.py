import os
import json
import shutil
import subprocess
from contextlib import nullcontext
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import calibration, checks, completion, contract, config, final_head, gitutil, stack, workspaces
from sakura.flow.phases import evaluate
from sakura.flow.phases import pull_request
from conftest import git
from flow_support import engine_flows, engine_head, make_context


class Runner:
    """An evaluator double; a PASS it writes evidence for was run through the tool, so the engine holds a receipt."""

    def __init__(self, results):
        self.results = iter(results)
        self.requests = []
        self.ctx = None

    def run(self, request):
        self.requests.append(request)
        result = next(self.results)
        for entry in result.get("rows", []) if isinstance(result, dict) else []:
            if entry.pop("_write_evidence", False):
                task_root = next(folder.parent for folder in Path(request.values["evidence_dir"]).parents
                                 if folder.name == "evidence")
                target = task_root / entry["evidence"]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("evidence")
                if request.phase == "evaluate" and self.ctx is not None:
                    self._engine_receipt(request, entry)
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, final="", result=result,
                               log_path=Path("run.log"))

    def _engine_receipt(self, request, entry):
        _record_run_row(self.ctx, request.cwd, entry["id"], entry["viewport"])


def _record_run_row(ctx, workspace, row_id, viewport):
    """What the engine's run_row tool leaves behind for a passing run: a log under evidence/ and a receipt."""
    store = ctx.store
    rows = contract.load(store, store.read_state()["contract_sha256"])
    row = next(row for row in rows if row.id == row_id and row.viewport == viewport)
    log = store.evidence_dir / "evaluator" / f"{row.id}-api.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text("exit 0\n$ true\n")
    revision = git(workspace, "rev-parse", "HEAD").strip()
    checks.record_receipt(store, row, contract.rows_digest(rows), revision, 0, log, checks.EVALUATOR_RECEIPTS_KEY)


class RunningStack:
    def __init__(self, base_url):
        self.base_url = base_url
        self.port = int(base_url.rsplit(":", 1)[1])

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return None


def _row(row_id="R1", baseline=False):
    return contract.Row(row_id, "journey", "step", "api", None, "true", baseline, False)


def _new_row():
    """A non-baseline row as a valid contract has it: it fails on the base build."""
    return contract.Row("R1", "journey", "step", "api", None, "false", False, False)


def _context(repo, tmp_path, rows, results, evaluate_on=True, familiarity="familiar", dial="save"):
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []},
                 "brakes": {"test_only_markers": ["SAKURA_TEST_ONLY"]}, "dial": dial}
    state = {"base_commit": git(repo, "rev-parse", "HEAD"), "branch": "feature/demo", "workspaces": {"0": {"path": str(repo), "branch": "feature/demo", "ports": [31000, 31009]}},
             "contract_sha256": "", "dial": dial, "familiarity": familiarity}
    # The person approved the rows' commands (S1 review, finding 1: no contract command runs unapproved).
    state["approvals"] = {"checks": {"commands": sorted({row.flow for row in rows})}}
    ctx = make_context(repo, own_repository=True, overrides=overrides, state=state)
    object.__setattr__(ctx, "steps", config.PresetSteps(True, True, evaluate_on))
    contract.save(ctx.store, rows)
    digest = __import__("hashlib").sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda current: current.update(contract_sha256=digest))
    engine_head(ctx)
    runner = Runner(results)
    runner.ctx = ctx
    object.__setattr__(ctx, "runner", runner)
    return ctx, runner


def _verdict(ctx, row_id="R1", verdict="PASS", evidence="evidence/R1.txt", viewport=None):
    return {"rows": [{"id": row_id, "viewport": viewport, "verdict": verdict,
                      "evidence": evidence, "notes": "checked", "_write_evidence": verdict == "PASS"}]}


def _install_stack(monkeypatch, repo):
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspaces.Workspace("0", repo, "feature/demo", range(31000, 31010)))
    monkeypatch.setattr(stack, "start", lambda *_args: RunningStack("http://127.0.0.1:31000"))


def _calibration_fixture(repo, tmp_path, monkeypatch, results):
    test_file = repo / "tests/test_app.py"
    test_file.parent.mkdir()
    test_file.write_text("baseline test\n")
    git(repo, "add", "tests/test_app.py")
    git(repo, "commit", "-qm", "add application test")
    baseline = git(repo, "rev-parse", "HEAD").strip()
    test_file.write_text("arbiter-approved test\n")
    git(repo, "add", "tests/test_app.py")
    git(repo, "commit", "-qm", "approve application test")
    ctx, runner = _context(repo, tmp_path, [_new_row()], results)
    ctx.store.update_state(lambda state: state.update(base_commit=baseline))
    engine_head(ctx)
    base = tmp_path / "base"
    git(repo, "clone", "-q", str(repo), str(base))
    git(base, "checkout", "--detach", baseline)
    gitutil.trust_repository(base)  # a clone stands in for the worktree the engine makes
    workspace = workspaces.Workspace("base", base, "base", range(31000, 31010))
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspace)
    monkeypatch.setattr(stack, "start", lambda *_args: RunningStack("http://127.0.0.1:31000"))
    return ctx, runner, base


def _calibration_needs_work():
    return {"rows": [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK",
                      "evidence": "", "notes": "check"}]}


def test_pass_without_evidence_file_is_not_flipped(repo, tmp_path, monkeypatch):
    ctx, _ = _context(repo, tmp_path, [_row()], [{"rows": [{"id": "R1", "viewport": None,
        "verdict": "PASS", "evidence": "evidence/missing.txt", "notes": ""}]}])
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: evaluate.PhaseOutcome("blocked", "stop"))
    assert evaluate.run(ctx).kind == "blocked"
    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_pass_without_an_engine_receipt_is_not_flipped(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row()], [_verdict(ctx=None)])
    runner.ctx = None  # the evaluator never ran the row through the tool
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: evaluate.PhaseOutcome("blocked", "stop"))

    assert evaluate.run(ctx).kind == "blocked"

    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes
    assert "no engine receipt of a passing run_row" in ctx.store.ledger.read_text()


def test_pass_on_a_failing_engine_receipt_is_not_flipped(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row()], [_verdict(ctx=None)])
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: evaluate.PhaseOutcome("blocked", "stop"))
    receipt = checks.record_receipt

    def failing(store, row, digest, revision, exit_code, log, space):
        return receipt(store, row, digest, revision, 3, log, space)

    monkeypatch.setattr(checks, "record_receipt", failing)

    assert evaluate.run(ctx).kind == "blocked"

    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_stale_evidence_is_not_flipped(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row()], [{}])
    evidence = ctx.store.root / "evidence/R1.txt"
    evidence.parent.mkdir(parents=True)
    evidence.write_text("stale")
    os.utime(evidence, ns=(1, 1))
    runner.results = iter([{"rows": [{"id": "R1", "viewport": None, "verdict": "PASS",
        "evidence": "evidence/R1.txt", "notes": ""}]}])
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: evaluate.PhaseOutcome("blocked", "stop"))
    assert evaluate.run(ctx).kind == "blocked"
    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_needs_work_evidence_must_remain_inside_task_evidence(repo, tmp_path):
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    verdicts = [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK", "evidence": "../app.py"}]

    errors = evaluate._validate_evidence(ctx, [_row()], verdicts, 0, require_pass=False)

    assert errors == ["R1: evidence path is outside evidence/"]


def test_evaluator_workspace_edit_cannot_pass_row_or_open_pr(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row()], [_verdict(ctx=None)])
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    original_run = runner.run

    def edit_then_pass(request):
        (repo / "app.py").write_text("changed by evaluator\n")
        return original_run(request)

    monkeypatch.setattr(runner, "run", edit_then_pass)
    pushed = []
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pushed.append("push"))
    monkeypatch.setattr(pull_request, "_create", lambda *_args: pushed.append("create"))

    outcome = evaluate.run(ctx)

    assert outcome.kind == "blocked"
    assert "modified the evaluated workspace" in outcome.reason
    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes
    assert pull_request._precondition(ctx.store.read_state(), contract.load(
        ctx.store, ctx.store.read_state()["contract_sha256"]), ctx.steps, "head") == "contract rows are still open"
    assert pushed == []


def test_calibration_workspace_edit_blocks_valid_base_pass(repo, tmp_path, monkeypatch):
    row = _row("BASE", baseline=True)
    ctx, runner = _context(repo, tmp_path, [row], [_verdict(ctx=None, row_id="BASE")])
    original_run = runner.run

    def edit_then_calibrate(request):
        (request.cwd / "app.py").write_text("changed during calibration\n")
        return original_run(request)

    runner.run = edit_then_calibrate
    workspace = workspaces.Workspace("base", repo, "base", range(31000, 31010))
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspace)

    try:
        evaluate._calibrate(ctx, [row], "evaluator", "claude:sonnet")
    except evaluate.EvaluationFailure as error:
        assert error.outcome.kind == "blocked"
        assert "evaluator modified the evaluated workspace" in error.outcome.reason
    else:
        raise AssertionError("calibration accepted an evaluator workspace edit")

    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_agent_blocks_preexisting_tracked_application_dirt(repo, tmp_path):
    (repo / "app.py").write_text("committed\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")
    (repo / "app.py").write_text("dirty before evaluator\n")
    ctx, runner = _context(repo, tmp_path, [_row()], [{}])
    workspace = workspaces.Workspace("0", repo, "feature/demo", range(31000, 31010))

    with pytest.raises(evaluate.EvaluationFailure, match="preexisting tracked changes"):
        evaluate._agent(ctx, "evaluator", "evaluate", workspace, [_row()], "http://localhost", "evidence")

    assert runner.requests == []


def test_calibration_accepts_only_the_engine_copied_committed_test(repo, tmp_path, monkeypatch):
    ctx, runner, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])

    outcome = evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")

    assert outcome is None
    assert (base / "tests/test_app.py").read_text() == "arbiter-approved test\n"
    assert len(runner.requests) == 1


def test_calibration_rejects_changed_engine_copied_test(repo, tmp_path, monkeypatch):
    ctx, runner, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])

    class ChangedPreparedInput(RunningStack):
        def __enter__(self):
            (base / "tests/test_app.py").write_text("changed after preparation\n")
            return self

    monkeypatch.setattr(stack, "start", lambda *_args: ChangedPreparedInput("http://127.0.0.1:31000"))

    with pytest.raises(evaluate.EvaluationFailure, match="preexisting workspace changes"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")

    assert runner.requests == []


def test_calibration_rejects_unrelated_preexisting_dirt(repo, tmp_path, monkeypatch):
    ctx, runner, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])
    (base / "README").write_text("unrelated dirt\n")

    with pytest.raises(evaluate.EvaluationFailure, match="preexisting workspace changes: README"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")

    assert runner.requests == []


def test_calibration_rejects_evaluator_mutation_of_prepared_test(repo, tmp_path, monkeypatch):
    ctx, runner, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])
    original_run = runner.run

    def mutate_test(request):
        (request.cwd / "tests/test_app.py").write_text("evaluator mutation\n")
        return original_run(request)

    runner.run = mutate_test

    with pytest.raises(evaluate.EvaluationFailure, match="evaluator modified the evaluated workspace"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")


def test_calibration_copy_preserves_committed_executable_flow_mode(repo, tmp_path, monkeypatch):
    ctx, _, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])
    flow = repo / "e2e/flow.sh"
    flow.parent.mkdir()
    flow.write_text("#!/bin/sh\nprintf flow-ok\n")
    flow.chmod(0o755)
    git(repo, "add", "e2e/flow.sh")
    git(repo, "commit", "-qm", "add executable flow")
    flow.chmod(0o644)
    engine_head(ctx)

    evaluate._sync_contract_flows(ctx, base)
    copied = base / "e2e/flow.sh"
    result = subprocess.run([str(copied)], capture_output=True, text=True, check=True)

    assert result.stdout == "flow-ok"
    assert copied.stat().st_mode & 0o111


def test_calibration_rejects_changed_executable_bit_on_prepared_flow(repo, tmp_path, monkeypatch):
    ctx, runner, base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])
    flow = repo / "e2e/flow.sh"
    flow.parent.mkdir()
    flow.write_text("#!/bin/sh\nprintf flow-ok\n")
    flow.chmod(0o755)
    git(repo, "add", "e2e/flow.sh")
    git(repo, "commit", "-qm", "add executable flow")
    engine_head(ctx)

    class ChangedPreparedMode(RunningStack):
        def __enter__(self):
            (base / "e2e/flow.sh").chmod(0o644)
            return self

    monkeypatch.setattr(stack, "start", lambda *_args: ChangedPreparedMode("http://127.0.0.1:31000"))

    with pytest.raises(evaluate.EvaluationFailure, match="preexisting workspace changes: e2e/flow.sh"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")

    assert runner.requests == []


def test_calibration_rejects_evaluator_chmod_of_prepared_flow(repo, tmp_path, monkeypatch):
    ctx, runner, _base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])
    flow = repo / "e2e/flow.sh"
    flow.parent.mkdir()
    flow.write_text("#!/bin/sh\nprintf flow-ok\n")
    flow.chmod(0o755)
    git(repo, "add", "e2e/flow.sh")
    git(repo, "commit", "-qm", "add executable flow")
    engine_head(ctx)
    original_run = runner.run

    def remove_executable_bit(request):
        (request.cwd / "e2e/flow.sh").chmod(0o644)
        return original_run(request)

    runner.run = remove_executable_bit

    with pytest.raises(evaluate.EvaluationFailure, match="evaluator modified the evaluated workspace"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")


def _calibration_row(repo, tmp_path, monkeypatch, flow):
    """A calibration row run from the base workspace (the repository here), as the evaluator's row tool runs it."""
    (repo / "app.py").write_text("committed\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")
    row = contract.Row("C1", "local", "inspect", "cli", None, flow, False, False)
    ctx, runner = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "calibrate")
    monkeypatch.chdir(repo)
    return ctx, runner, row


def test_CalibrationRow_ASurvivorCommitInTheBaseWorkspace_IsNotWhatTheRowRunsOn(repo, tmp_path, monkeypatch):
    # Review 12, the audit: calibration ran its rows on the base workspace's HEAD, which a survivor can move.
    ctx, _, row = _calibration_row(repo, tmp_path, monkeypatch, "grep -q hacked app.py")
    (repo / "app.py").write_text("hacked\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "a survivor's commit")

    with completion.base_build(ctx):
        assert evaluate.run_row(ctx, row) == 1


def test_CalibrationRow_AnUncommittedEditInTheBaseWorkspace_TakesNoPart(repo, tmp_path, monkeypatch):
    ctx, _, row = _calibration_row(repo, tmp_path, monkeypatch, "grep -q committed app.py")
    (repo / "app.py").write_text("hacked\n")
    (repo / "new.py").write_text("untracked\n")

    with completion.base_build(ctx):
        assert evaluate.run_row(ctx, row) == 0


def test_CalibrationRow_WithoutTheEnginesCheckoutOfTheBaseBuild_IsRefused(repo, tmp_path, monkeypatch, capsys):
    ctx, _, row = _calibration_row(repo, tmp_path, monkeypatch, "true")

    assert evaluate.run_row(ctx, row) == 1
    assert "the engine holds no checkout of its base build" in capsys.readouterr().err


def test_Calibration_StartsTheAppAndRunsTheRowsFromTheEnginesCheckoutOfTheFlowsCommit(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_new_row()], [_calibration_needs_work()])
    base_commit = ctx.store.read_state()["base_commit"]
    flows = engine_flows(ctx, {"e2e/flow.py": (b"print('flow')\n", False)}, ["e2e/**"])
    base = tmp_path / "base"
    git(repo, "clone", "-q", str(repo), str(base))
    git(base, "checkout", "--detach", base_commit)
    gitutil.trust_repository(base)
    workspace = workspaces.Workspace("base", base, "base", range(31000, 31010))
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspace)
    started = []

    class CheckedStack(RunningStack):
        def __init__(self, workspace, checkout):
            super().__init__("http://127.0.0.1:31000")
            started.append((workspace.path, checkout))

        def __enter__(self):
            held = completion.recorded(ctx.store.read_state())
            assert held is not None and held.revision == started[0][1].revision
            return self

    monkeypatch.setattr(stack, "start", lambda _ctx, workspace, checkout=None: CheckedStack(workspace, checkout))

    assert evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet") is None

    workspace_path, checkout = started[0]
    assert checkout is not None and checkout.revision == flows and workspace_path == base
    assert checkout.source != base and not checkout.source.exists()
    assert completion.STATE_KEY not in ctx.store.read_state()
    assert len(runner.requests) == 1 and runner.requests[0].cwd == base


def test_Calibration_ASourceChangeInTheEnginesCheckoutDuringTheRun_IsRefused(repo, tmp_path, monkeypatch):
    ctx, _, _base = _calibration_fixture(repo, tmp_path, monkeypatch, [_calibration_needs_work()])

    class RewritingStack(RunningStack):
        def __init__(self, checkout):
            super().__init__("http://127.0.0.1:31000")
            self.checkout = checkout

        def __enter__(self):
            (self.checkout.source / "tests/test_app.py").write_text("swapped for the run\n")
            return self

    monkeypatch.setattr(stack, "start", lambda _ctx, _workspace, checkout=None: RewritingStack(checkout))

    with pytest.raises(completion.CompletionRefused, match="changed during the run"):
        evaluate._calibrate(ctx, [_new_row()], "evaluator", "claude:sonnet")
    assert not calibration.is_current(ctx.repo, "claude:sonnet")


@pytest.mark.parametrize("relative_path", ["app.py", ".runtime/sakura.json", "e2e/flow.py"])
def test_agent_blocks_same_path_byte_changes(repo, tmp_path, relative_path):
    target = repo / relative_path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("before evaluator\n")
    if relative_path.startswith(".runtime/"):
        (repo / ".gitignore").write_text(".runtime/\n")
    elif relative_path == "app.py":
        git(repo, "add", "app.py")
        git(repo, "commit", "-qm", "add app")
    ctx, runner = _context(repo, tmp_path, [_row()], [{}])
    original_run = runner.run

    def edit_same_path(request):
        target.write_text("after evaluator\n")
        return original_run(request)

    runner.run = edit_same_path
    workspace = workspaces.Workspace("base", repo, "base", range(31000, 31010))

    with pytest.raises(evaluate.EvaluationFailure, match="modified the evaluated workspace"):
        evaluate._agent(ctx, "evaluator", "calibrate", workspace, [_row()], "http://localhost", "evidence/base")


def test_cli_row_runs_without_configured_app_stack(repo, tmp_path, monkeypatch, capsys):
    row = contract.Row("C1", "local", "inspect", "cli", None, "printf cli-row-ok", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.delenv("SAKURA_FLOW_BASE_URL", raising=False)
    monkeypatch.chdir(repo)

    with completion.base_build(ctx):  # the checkout the engine holds while it calibrates
        assert evaluate.run_row(ctx, row) == 0
    assert capsys.readouterr().out == "cli-row-ok"


@pytest.mark.parametrize(("flow", "code"), [("true", 0), ("exit 3", 3)])
def test_row_reports_its_exit_code_even_when_the_flow_prints_nothing(repo, tmp_path, monkeypatch, capsys, flow, code):
    row = contract.Row("C1", "local", "inspect", "cli", None, flow, False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.delenv("SAKURA_FLOW_BASE_URL", raising=False)
    monkeypatch.chdir(repo)

    with completion.base_build(ctx):
        assert evaluate.run_row(ctx, row) == code
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == f"run-row C1: exit {code}\nevidence: evidence/base/evaluator/C1-api.log\n"


def test_final_row_run_writes_the_engine_receipt_and_calibration_none(repo, tmp_path, monkeypatch, capsys):
    row = contract.Row("C1", "local", "inspect", "cli", None, "true", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.chdir(repo)
    digest = contract.rows_digest([row])
    revision = git(repo, "rev-parse", "HEAD").strip()

    monkeypatch.setenv("SAKURA_FLOW_PHASE", "calibrate")
    with completion.base_build(ctx):
        assert evaluate.run_row(ctx, row) == 0
    assert not checks.receipt_passes(ctx.store.read_state(), row, digest, revision, 0, checks.EVALUATOR_RECEIPTS_KEY)

    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    with completion.opened(ctx):  # the checkout the engine holds while its evaluator runs
        assert evaluate.run_row(ctx, row) == 0
    checks.ingest_row_receipts(ctx.store)  # the engine process takes in what its child wrote
    assert checks.receipt_passes(ctx.store.read_state(), row, digest, revision, 0, checks.EVALUATOR_RECEIPTS_KEY)
    assert "receipts" not in ctx.store.read_state()  # the engine's own check receipts are a different key
    assert (ctx.store.evidence_dir / "evaluator" / "C1-api.log").read_text().startswith("exit 0\n")
    assert "evidence: evidence/evaluator/C1-api.log" in capsys.readouterr().err


def test_an_uncommitted_edit_or_a_new_commit_in_the_workspace_is_not_what_the_final_row_runs_on(repo, tmp_path,
                                                                                               monkeypatch, capsys):
    # Review 11: the final evaluation's row runs in the engine's checkout of its head; the workspace (an edit, an
    # untracked file, a commit a survivor added) takes no part, so it can neither pass a row nor stop one.
    (repo / "app.py").write_text("committed\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")
    row = contract.Row("C1", "local", "inspect", "cli", None, "grep -q hacked app.py", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(repo)
    head = git(repo, "rev-parse", "HEAD")
    (repo / "app.py").write_text("hacked\n")
    (repo / "new.py").write_text("untracked\n")
    (repo / "hacked.txt").write_text("hacked\n")
    git(repo, "add", "hacked.txt")
    git(repo, "commit", "-qm", "a survivor's commit")

    with completion.opened(ctx):
        assert evaluate.run_row(ctx, row) == 1

    checks.ingest_row_receipts(ctx.store)
    receipt = ctx.store.read_state()["evaluator_receipts"]["C1/api"]
    assert receipt["revision"] == head and receipt["exit"] == 1


def test_RunRow_ThatRewritesASourceFileTestsItAndRestoresIt_GetsNoPassingReceipt(repo, tmp_path, monkeypatch, capsys):
    (repo / "app.py").write_text("committed\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")
    flow = "printf 'hacked\\n' > app.py && grep -q hacked app.py && git checkout -- app.py"
    row = contract.Row("C1", "local", "inspect", "cli", None, flow, False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(repo)

    with completion.opened(ctx):
        assert evaluate.run_row(ctx, row) != 0
    checks.ingest_row_receipts(ctx.store)
    revision = git(repo, "rev-parse", "HEAD").strip()
    assert not checks.receipt_passes(ctx.store.read_state(), row, contract.rows_digest([row]), revision, 0,
                                     checks.EVALUATOR_RECEIPTS_KEY)


def test_RunRow_ACheckoutChangedDuringTheRun_GetsNoReceipt(repo, tmp_path, monkeypatch, capsys):
    (repo / "app.py").write_text("committed\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add app")
    row = contract.Row("C1", "local", "inspect", "cli", None, "true", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(repo)

    def substituted(_ctx, checkout, *_args, **_kwargs):
        (checkout / "app.py").write_text("hacked\n")
        return {"C1": 0}

    monkeypatch.setattr(evaluate.baseline, "run_rows", substituted)
    with completion.opened(ctx):
        assert evaluate.run_row(ctx, row) == 1
    assert "app.py" in capsys.readouterr().err
    checks.ingest_row_receipts(ctx.store)
    assert "evaluator_receipts" not in ctx.store.read_state()


def test_the_row_runs_in_an_engine_checkout_of_the_revision_that_is_removed_after(repo, tmp_path, monkeypatch, capsys):
    row = contract.Row("C1", "local", "inspect", "cli", None, "touch \"$TMPDIR/ran-here\"", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(repo)

    with completion.opened(ctx):
        assert evaluate.run_row(ctx, row) == 0

    assert not (repo / "ran-here").exists()  # the flow never touched the agent's workspace
    assert [path for path in repo.parent.iterdir() if path.name.startswith(".sakura-row-")] == []
    assert len(git(repo, "worktree", "list").splitlines()) == 1


def test_run_row_serves_evaluator_rows_only_and_keeps_its_receipts_apart(repo, tmp_path, monkeypatch):
    row = contract.Row("C1", "local", "inspect", "cli", None, "true", False, False, "check")
    ctx, _ = _context(repo, tmp_path, [row], [])
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.chdir(repo)
    with pytest.raises(contract.ContractError, match="evaluator rows only"):
        evaluate.run_row(ctx, row)
    owner = contract.Row("C2", "local", "inspect", "cli", None, "look", False, False, "owner")
    with pytest.raises(contract.ContractError, match="evaluator rows only"):
        evaluate.run_row(ctx, owner)
    # An evaluator receipt never lands in, or replaces, the engine's check receipts.
    ctx.store.update_state(lambda state: state.update(receipts={"C1/api": {"exit": 0, "marker": "check"}}))
    log = ctx.store.evidence_dir / "x.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text("exit 0")
    checks.write_row_receipt(ctx.store, row, "d", "r", 0, log)
    checks.ingest_row_receipts(ctx.store)
    state = ctx.store.read_state()
    assert state["receipts"]["C1/api"]["marker"] == "check" and state["evaluator_receipts"]["C1/api"]["exit"] == 0


def test_row_whose_command_is_not_approved_does_not_run(repo, tmp_path, monkeypatch, capsys):
    row = contract.Row("C1", "local", "inspect", "cli", None, "touch ran", False, False)
    ctx, _ = _context(repo, tmp_path, [row], [])
    ctx.store.update_state(lambda state: state.update(approvals={}))
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(repo)

    with completion.opened(ctx):
        assert evaluate.run_row(ctx, row) == 1
    assert "not approved" in capsys.readouterr().err
    assert not (repo / "ran").exists()


def test_stack_heartbeat_uses_fresh_store_without_sweeping(repo, tmp_path, monkeypatch):
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    fresh_stores = []

    class FreshStore:
        def __init__(self, task_repo, task_id):
            assert task_repo == ctx.repo
            assert task_id == ctx.store.task_id
            self.state = {"stacks": {"demo:0": {"last_used": 0}}}
            fresh_stores.append(self)

        def read_state(self):
            return self.state

        def update_state(self, change):
            change(self.state)

    monkeypatch.setattr(evaluate, "TaskStore", FreshStore, raising=False)
    monkeypatch.setattr(evaluate.stacks, "lifecycle_lock", lambda *_args: nullcontext())
    monkeypatch.setattr(evaluate.stacks, "sweep", lambda *_args: pytest.fail("child must not sweep stacks"))
    monkeypatch.setattr(evaluate.time, "time", lambda: 100)

    evaluate._refresh_stack(ctx, "demo:0")

    assert len(fresh_stores) == 1
    assert fresh_stores[0] is not ctx.store
    assert fresh_stores[0].state["stacks"]["demo:0"]["last_used"] == 100


def test_row_refreshes_stack_before_waiting_for_heavy_slot(repo, tmp_path, monkeypatch):
    row = _row()
    ctx, _ = _context(repo, tmp_path, [row], [])
    ctx.store.update_state(lambda state: state.update(stacks={"demo:0": {
        "pid": 123, "command": "app", "workspace": str(repo), "log": "app.log",
        "port": 31000, "last_used": 0}}))
    monkeypatch.setenv("SAKURA_FLOW_STACK", "demo:0")
    monkeypatch.setenv("SAKURA_FLOW_BASE_URL", "http://127.0.0.1:31000")
    monkeypatch.chdir(repo)
    events = []

    class Slot:
        def __enter__(self):
            events.append("slot")

        def __exit__(self, *_args):
            return None

    def run_tracked(_store, _argv, *, log_path, **_kwargs):
        events.append("run")
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        return SimpleNamespace(exit_code=0)

    def refresh(_ctx, _stack_id):
        events.append("refresh")
        return ctx.store.read_state()["stacks"]["demo:0"]

    monkeypatch.setattr(evaluate, "_refresh_stack", refresh)
    monkeypatch.setattr(evaluate.baseline.process, "listener_problem", lambda *_args: "")  # no app runs here
    monkeypatch.setattr(evaluate.process, "heavy_slot", lambda *_args, **_kwargs: Slot())
    monkeypatch.setattr(evaluate.process, "external_tracked_run", run_tracked)

    with completion.base_build(ctx):
        assert evaluate.run_row(ctx, row) == 0
    assert events[0] == "refresh" and events[-2:] == ["slot", "run"]


def test_test_only_marker_in_production_path_blocks(repo, tmp_path, monkeypatch):
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    monkeypatch.setattr(evaluate, "_diff", lambda *_args: [("app.py", "+SAKURA_TEST_ONLY")])
    assert "SAKURA_TEST_ONLY" in evaluate._test_only_problem(ctx, git(repo, "rev-parse", "HEAD"))


def test_marker_in_test_file_is_allowed(repo, tmp_path, monkeypatch):
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    monkeypatch.setattr(evaluate, "_diff", lambda *_args: [("tests/test_app.py", "+SAKURA_TEST_ONLY")])
    assert evaluate._test_only_problem(ctx, git(repo, "rev-parse", "HEAD")) == ""


def test_Evaluate_TheEvaluatorWorksOnTheEnginesHead_NotOnACommitASurvivorAddedToWorkspaceZero(repo, tmp_path,
                                                                                               monkeypatch):
    # Review 11, finding 1: workspace 0 is moved back to the engine's head before the evaluator runs, and its pass
    # counts only with a receipt at that head.
    ctx, runner = _context(repo, tmp_path, [_row()], [])
    runner.results = iter([_verdict(ctx)])
    head = git(repo, "rev-parse", "HEAD")
    (repo / "survivor.txt").write_text("never reviewed\n")
    git(repo, "add", "survivor.txt")
    git(repo, "commit", "-qm", "a survivor's commit")
    seen = []
    original = runner.run

    def look(request):
        seen.append((git(request.cwd, "rev-parse", "HEAD"), (request.cwd / "survivor.txt").exists()))
        return original(request)

    runner.run = look
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)

    assert evaluate.run(ctx).kind == "next"
    assert seen == [(head, False)]
    assert ctx.store.read_state()["evaluator_receipts"]["R1/api"]["revision"] == head


def _calibration_receipts(rows, revision, digest, exits):
    return {f"{row.id}/api": {"row": row.id, "viewport": None, "contract": digest, "revision": revision,
                              "exit": exits[row.id]} for row in rows}


def test_calibration_judge_detects_generous_evaluator():
    rows = [_row("new"), _row("base", True)]
    receipts = _calibration_receipts(rows, "r" * 40, "d" * 64, {"new": 1, "base": 0})
    errors = calibration.judge(rows, [
        {"id": "new", "verdict": "PASS", "evidence": "evidence/base/new.png"},
        {"id": "base", "verdict": "NEEDS_WORK", "evidence": ""},
    ], receipts, "r" * 40, "d" * 64)
    assert len(errors) == 2


def test_CalibrationJudge_ABaselineRowTheEngineSawFailOnTheBase_IsAnErrorWhateverTheVerdict():
    rows = [_row("new"), _row("base", True)]
    receipts = _calibration_receipts(rows, "r" * 40, "d" * 64, {"new": 1, "base": 1})
    errors = calibration.judge(rows, [
        {"id": "new", "verdict": "NEEDS_WORK", "evidence": ""},
        {"id": "base", "verdict": "PASS", "evidence": "evidence/base/base.log"},
    ], receipts, "r" * 40, "d" * 64)
    assert errors == ["base baseline fails on the base build (exit 1)"]


def test_CalibrationJudge_ANewRowTheEngineSawPassOnTheBase_IsAnErrorWhateverTheVerdict():
    # Review 14, finding 4: a non-baseline row must fail on the base build; an evaluator saying NEEDS_WORK does not
    # hide that it passed there.
    rows = [_row("new"), _row("base", True)]
    receipts = _calibration_receipts(rows, "r" * 40, "d" * 64, {"new": 0, "base": 0})
    errors = calibration.judge(rows, [
        {"id": "new", "verdict": "NEEDS_WORK", "evidence": ""},
        {"id": "base", "verdict": "PASS", "evidence": "evidence/base/role/base.txt"},
    ], receipts, "r" * 40, "d" * 64)
    assert errors == ["new non-baseline passes on the base build (exit 0)"]


def test_CalibrationRecord_WithoutAReceiptOfEveryRowAtTheBaseBuild_IsRefused(repo):
    # Review 13, finding 2: calibration is recorded only on the engine's receipts of every row on the base build.
    rows = [_row("new"), _row("base", True)]
    revision, digest = "r" * 40, "d" * 64
    receipts = _calibration_receipts(rows, revision, digest, {"new": 1, "base": 0})
    for broken in ({"base/api": receipts["base/api"]}, {**receipts, "new/api": {**receipts["new/api"], "revision": "x"}},
                   {**receipts, "base/api": {**receipts["base/api"], "contract": "other"}}):
        with pytest.raises(calibration.CalibrationRefused):
            calibration.record(repo, "claude:opus", "t1", rows, broken, revision, digest)
        assert not calibration.is_current(repo, "claude:opus")

    calibration.record(repo, "claude:opus", "t1", rows, receipts, revision, digest)
    assert calibration.is_current(repo, "claude:opus")


def _calibration_with_rows(repo, tmp_path, monkeypatch, rows, verdicts):
    """A calibration whose evaluator returns `verdicts` without running a single row itself."""
    ctx, runner = _context(repo, tmp_path, rows, [{"rows": verdicts}])
    base_commit = ctx.store.read_state()["base_commit"]
    base = tmp_path / "base"
    git(repo, "clone", "-q", str(repo), str(base))
    git(base, "checkout", "--detach", base_commit)
    gitutil.trust_repository(base)
    workspace = workspaces.Workspace("base", base, "base", range(31000, 31010))
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspace)
    monkeypatch.setattr(stack, "start", lambda *_args: RunningStack("http://127.0.0.1:31000"))
    original_run = runner.run

    def with_fresh_evidence(request):
        for entry in verdicts:
            if entry["verdict"] == "PASS":
                target = ctx.store.root / entry["evidence"]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("evidence the evaluator wrote itself\n")
        return original_run(request)

    runner.run = with_fresh_evidence
    return ctx, runner


def test_Calibration_AnEvaluatorThatSkipsRunRow_CannotGetCalibrationRecorded(repo, tmp_path, monkeypatch):
    # The evaluator writes fresh evidence and returns the verdicts calibration expects, running nothing; the engine's
    # own run of the rows on the base build shows the baseline row failing there, so nothing is recorded.
    rows = [contract.Row("R1", "j", "new", "cli", None, "exit 1", False, False),
            contract.Row("B1", "j", "old", "cli", None, "exit 1", True, False)]
    verdicts = [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK", "evidence": "", "notes": "new"},
                {"id": "B1", "viewport": None, "verdict": "PASS", "evidence": "evidence/base/B1.txt", "notes": "ok"}]
    ctx, _ = _calibration_with_rows(repo, tmp_path, monkeypatch, rows, verdicts)

    outcome = evaluate._calibrate(ctx, rows, "evaluator", "claude:opus")

    assert outcome is not None and outcome.kind == "blocked"
    assert "B1 baseline fails on the base build (exit 1)" in outcome.reason
    assert not calibration.is_current(ctx.repo, "claude:opus")


def test_Calibration_TheEngineRunsEveryRowOnTheBaseBuild_AndReceiptsEveryResult(repo, tmp_path, monkeypatch):
    rows = [contract.Row("R1", "j", "new", "cli", None, "exit 3", False, False),
            contract.Row("B1", "j", "old", "cli", None, "true", True, False)]
    verdicts = [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK", "evidence": "", "notes": "new"},
                {"id": "B1", "viewport": None, "verdict": "PASS", "evidence": "evidence/base/B1.txt", "notes": "ok"}]
    ctx, _ = _calibration_with_rows(repo, tmp_path, monkeypatch, rows, verdicts)

    assert evaluate._calibrate(ctx, rows, "evaluator", "claude:opus") is None

    state = ctx.store.read_state()
    build = final_head.base_build(ctx)[1]
    receipts = state[checks.CALIBRATION_RECEIPTS_KEY]
    assert {key: (receipt["exit"], receipt["revision"]) for key, receipt in receipts.items()} == {
        "R1/api": (3, build), "B1/api": (0, build)}
    assert calibration.is_current(ctx.repo, "claude:opus")


def test_calibration_skipped_when_current(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row()], [])
    runner.results = iter([_verdict(ctx)])
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)
    outcome = evaluate.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert [request.phase for request in runner.requests] == ["evaluate"]


def test_new_head_invalidates_old_passes_and_evidence(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row("R1", baseline=True)], [
        {"rows": [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK", "evidence": "", "notes": "check again"}]}
    ])
    old_head = git(repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state.update(evaluation_head=old_head))
    evidence = ctx.store.evidence_dir / "R1.txt"
    evidence.parent.mkdir(parents=True)
    evidence.write_text("old revision evidence")
    contract.save(ctx.store, [contract.Row("R1", "journey", "step", "api", None, "true", True, True)])
    ctx.store.update_state(lambda state: state.update(contract_sha256=__import__("hashlib").sha256(ctx.store.contract.read_bytes()).hexdigest()))
    (repo / "revision.txt").write_text("new revision\n")
    git(repo, "add", "revision.txt")
    git(repo, "commit", "-qm", "change implementation")
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: evaluate.PhaseOutcome("blocked", "stop"))

    outcome = evaluate.run(ctx)

    assert outcome.kind == "blocked"
    assert [request.phase for request in runner.requests] == ["evaluate"]
    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes
    assert not evidence.exists()


def test_evaluate_inputs_hash_changes_when_evaluator_changes(repo, tmp_path):
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    before = evaluate.inputs_hash(ctx)

    ctx.config.values["roles"]["evaluator"] = "claude:sonnet"

    assert evaluate.inputs_hash(ctx) != before


def test_same_head_evaluator_change_rechecks_existing_pass(repo, tmp_path, monkeypatch):
    ctx, runner = _context(repo, tmp_path, [_row(), _row("R2")], [])
    rows = [contract.Row("R1", "journey", "step", "api", None, "true", False, True), _row("R2")]
    digest = contract.save(ctx.store, rows)
    head = git(repo, "rev-parse", "HEAD").strip()
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest, evaluation_head=head))
    previous_inputs = evaluate.inputs_hash(ctx)
    ctx.store.update_state(lambda state: state.update(evaluation_inputs_sha256=previous_inputs))
    ctx.config.values["roles"]["evaluator"] = "claude:sonnet"
    runner.results = iter([{"rows": [
        {"id": row.id, "viewport": None, "verdict": "PASS", "evidence": f"evidence/{row.id}.txt",
         "notes": "checked", "_write_evidence": True} for row in rows]}])
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)

    outcome = evaluate.run(ctx)

    assert outcome.kind == "next"
    assert [request.phase for request in runner.requests] == ["evaluate"]
    assert {row["id"] for row in json.loads(runner.requests[0].values["rows"])} == {"R1", "R2"}
    assert all(row.passes for row in contract.load(ctx.store, ctx.store.read_state()["contract_sha256"]))


def test_calibration_copies_contract_flows_to_base(repo, tmp_path):
    base_head = git(repo, "rev-parse", "HEAD")
    (repo / "e2e").mkdir()
    (repo / "e2e/flow.py").write_text("print('flow')\n")
    git(repo, "add", "e2e/flow.py")
    git(repo, "commit", "-qm", "add flow")
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    ctx.store.update_state(lambda state: state.update(base_commit=base_head))
    engine_head(ctx)
    base = tmp_path / "base"
    base.mkdir()
    evaluate._sync_contract_flows(ctx, base)
    assert (base / "e2e/flow.py").read_text() == "print('flow')\n"


def _head_with_test_files(repo, tmp_path, files):
    base_head = git(repo, "rev-parse", "HEAD")
    for name, data in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_bytes(data)
    git(repo, "add", *files)
    git(repo, "commit", "-qm", "the builder's tests")
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    ctx.store.update_state(lambda state: state.update(base_commit=base_head))
    engine_head(ctx)
    base = tmp_path / "base"
    base.mkdir()
    return ctx, base


def _no_blob_read(monkeypatch):
    """Fail the test when calibration reads a blob's content: its size must decide first."""
    real = evaluate.gitutil.run

    def run(cwd, *args, **kwargs):
        if args[:1] == ("show",):
            raise AssertionError(f"a blob was read before its size was checked: {args}")
        return real(cwd, *args, **kwargs)

    monkeypatch.setattr(evaluate.gitutil, "run", run)


def test_Calibration_AnOversizedTestBlob_IsRefusedBeforeItIsRead(repo, tmp_path, monkeypatch):
    # Review 15, finding 5: calibration's copies of the head's test files obey the flow-file limits.
    from sakura.flow import flow_paths

    monkeypatch.setattr(flow_paths, "MAX_FLOW_FILE_BYTES", 16)
    ctx, base = _head_with_test_files(repo, tmp_path, {"tests/test_big.py": b"x" * 17})
    _no_blob_read(monkeypatch)
    with pytest.raises(evaluate.EvaluationFailure, match=r"tests/test_big\.py.*per-file limit of 16 bytes"):
        evaluate._sync_contract_flows(ctx, base)
    assert not (base / "tests/test_big.py").exists()


def test_Calibration_TooManyTestBlobs_AreRefusedBeforeAnyIsRead(repo, tmp_path, monkeypatch):
    from sakura.flow import flow_paths

    monkeypatch.setattr(flow_paths, "MAX_FLOW_FILES", 2)
    ctx, base = _head_with_test_files(repo, tmp_path, {f"tests/test_{n}.py": b"pass\n" for n in range(3)})
    _no_blob_read(monkeypatch)
    with pytest.raises(evaluate.EvaluationFailure, match="3 files, more than the limit of 2"):
        evaluate._sync_contract_flows(ctx, base)


def test_Calibration_TestBlobsOverTheTotalLimit_AreRefused(repo, tmp_path, monkeypatch):
    from sakura.flow import flow_paths

    monkeypatch.setattr(flow_paths, "MAX_FLOW_TOTAL_BYTES", 10)
    ctx, base = _head_with_test_files(repo, tmp_path, {"tests/test_a.py": b"123456\n", "tests/test_b.py": b"123456\n"})
    with pytest.raises(evaluate.EvaluationFailure, match="total limit of 10 bytes"):
        evaluate._sync_contract_flows(ctx, base)


def test_fix_rounds_escalate(repo, tmp_path, monkeypatch):
    needs_work = {"rows": [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK", "evidence": "", "notes": "still broken"}]}
    ctx, runner = _context(repo, tmp_path, [_row()], [needs_work] * (config.DIAL["save"].fix_rounds + 1), dial="save")
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(evaluate.testing, "run", lambda *_args: 0)
    monkeypatch.setattr(evaluate.integrate, "run_full_tests", lambda *_args: None)
    fix_calls = []
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: fix_calls.append(1))
    outcome = evaluate.run(ctx)
    assert outcome.kind == "blocked", outcome.reason
    assert len(fix_calls) == config.DIAL["save"].fix_rounds


def test_evaluate_skipped_when_preset_has_no_evaluator(repo, tmp_path):
    ctx, _ = _context(repo, tmp_path, [_row()], [], evaluate_on=False)
    outcome = evaluate.run(ctx)
    assert outcome.kind == "skip"
    assert "not verified by an evaluator" in ctx.store.ledger.read_text()


def test_test_only_marker_blocks_non_evaluator_preset(repo, tmp_path):
    (repo / "app.py").write_text("def run(): return True\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add application")
    ctx, _ = _context(repo, tmp_path, [_row()], [], evaluate_on=False)
    (repo / "app.py").write_text("def run(): return True\n# SAKURA_TEST_ONLY\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add test-only marker")
    engine_head(ctx)

    outcome = evaluate.run(ctx)

    assert outcome.kind == "blocked"
    assert "SAKURA_TEST_ONLY" in outcome.reason


def test_evaluator_fix_cannot_change_existing_test(repo, tmp_path, monkeypatch):
    test_file = repo / "tests" / "test_existing.py"
    test_file.parent.mkdir()
    test_file.write_text("def test_original(): assert True\n")
    git(repo, "add", "tests/test_existing.py")
    git(repo, "commit", "-qm", "add protected test")
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    _install_fix_runner(ctx, repo, lambda: test_file.write_text("def test_original(): assert False\n"))
    monkeypatch.setattr(evaluate.testing, "run", lambda *_args, **_kwargs: 0)

    outcome = evaluate._run_fix(ctx, repo, 1, [_row()])

    assert outcome is not None and outcome.kind == "blocked"
    assert "protected paths" in outcome.reason


def test_rejected_fix_stays_pending_across_resume_and_checks_cumulative_diff(repo, tmp_path, monkeypatch):
    test_file = repo / "tests" / "test_existing.py"
    test_file.parent.mkdir()
    test_file.write_text("def test_original(): assert True\n")
    git(repo, "add", "tests/test_existing.py")
    git(repo, "commit", "-qm", "add protected test")
    baseline = git(repo, "rev-parse", "HEAD").strip()
    ctx, _ = _context(repo, tmp_path, [_row()], [], dial="lean")
    roles = []
    attempts = 0

    class RetryRunner:
        def run(self, request):
            nonlocal attempts
            roles.append(request.role)
            if request.role == "builder":
                attempts += 1
                if attempts == 1:
                    test_file.write_text("def test_original(): assert False\n")
                    git(repo, "add", "tests/test_existing.py")
                else:
                    (repo / "app.py").write_text("fix follow-up\n")
                    git(repo, "add", "app.py")
                git(repo, "commit", "-qm", f"fix attempt {attempts}")
                result = {}
            elif request.role == "evaluator":
                if roles.count("evaluator") == 1:
                    result = {"rows": [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK",
                                         "evidence": "", "notes": "repair"}]}
                else:
                    proof = ctx.store.root / "evidence/R1.txt"
                    proof.parent.mkdir(parents=True, exist_ok=True)
                    proof.write_text("second-head pass")
                    result = {"rows": [{"id": "R1", "viewport": None, "verdict": "PASS",
                                         "evidence": "evidence/R1.txt", "notes": "checked"}]}
            else:
                result = {"verdict": "APPROVE", "findings": []}
            return SimpleNamespace(exit_code=0, stalled=False, stopped=False, timed_out=False,
                                   log_path=Path("retry.log"), result=result)

    object.__setattr__(ctx, "runner", RetryRunner())
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(evaluate.testing, "run", lambda *_args, **_kwargs: 0)
    monkeypatch.setattr(evaluate.integrate, "run_full_tests", lambda *_args: None)
    protected_starts = []
    check_protected = evaluate.integrate.check_protected

    def record_protected_start(context, label, start, head):
        protected_starts.append(start)
        return check_protected(context, label, start, head)

    monkeypatch.setattr(evaluate.integrate, "check_protected", record_protected_start)

    first = evaluate.run(ctx)
    assert first.kind == "blocked" and "protected paths" in first.reason
    progress = ctx.store.read_state().get("evaluation_fix", {})
    assert progress.get("accepted_head") == baseline
    assert progress.get("pending", {}).get("baseline") == baseline

    second = evaluate.run(ctx)

    assert second.kind == "blocked" and "protected paths" in second.reason
    assert roles == ["evaluator", "builder", "builder"]
    assert protected_starts == [baseline, baseline]


def test_final_pending_fix_resumes_validation_without_another_builder(repo, tmp_path, monkeypatch):
    baseline = git(repo, "rev-parse", "HEAD").strip()
    ctx, _ = _context(repo, tmp_path, [_row()], [], dial="save")
    _install_stack(monkeypatch, repo)
    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    roles = []
    builder_calls = 0

    class ResumeRunner:
        def run(self, request):
            nonlocal builder_calls
            roles.append(request.role)
            if request.role == "builder":
                builder_calls += 1
                (repo / "app.py").write_text("resumed candidate\n")
                git(repo, "add", "app.py")
                git(repo, "commit", "-qm", "candidate fix")
                result = {}
            elif request.role == "evaluator" and roles.count("evaluator") == 1:
                result = {"rows": [{"id": "R1", "viewport": None, "verdict": "NEEDS_WORK",
                                     "evidence": "", "notes": "repair"}]}
            elif request.role == "evaluator":
                proof = ctx.store.root / "evidence/R1.txt"
                proof.parent.mkdir(parents=True, exist_ok=True)
                proof.write_text("candidate passes")
                _record_run_row(ctx, request.cwd, "R1", None)
                result = {"rows": [{"id": "R1", "viewport": None, "verdict": "PASS",
                                     "evidence": "evidence/R1.txt", "notes": "checked"}]}
            else:
                result = {"verdict": "APPROVE", "findings": []}
            return SimpleNamespace(exit_code=0, stalled=False, stopped=False, timed_out=False,
                                   log_path=Path("resume.log"), result=result)

    object.__setattr__(ctx, "runner", ResumeRunner())
    focused_calls = 0

    def run_focused(*_args, **_kwargs):
        nonlocal focused_calls
        focused_calls += 1
        if focused_calls == 1:
            raise evaluate.process.TestCommandInterrupted(
                evaluate.process.ProcessOutcome(None, False, False, True, 1234))
        return 0

    monkeypatch.setattr(evaluate.testing, "run", run_focused)
    full_test_calls = []
    monkeypatch.setattr(evaluate.integrate, "run_full_tests", lambda *_args: full_test_calls.append(True))
    protected_starts = []
    check_protected = evaluate.integrate.check_protected

    def record_protected_start(context, label, start, head):
        protected_starts.append(start)
        return check_protected(context, label, start, head)

    monkeypatch.setattr(evaluate.integrate, "check_protected", record_protected_start)

    interrupted = evaluate.run(ctx)
    pending = ctx.store.read_state()["evaluation_fix"]["pending"]
    resumed = evaluate.run(ctx)

    assert interrupted.kind == "stopped"
    assert pending == {"baseline": baseline, "head": git(repo, "rev-parse", "HEAD").strip(), "round": 1}
    assert resumed.kind == "next"
    assert builder_calls == 1
    assert focused_calls == 2
    assert full_test_calls == [True]
    assert protected_starts == [baseline]
    assert roles == ["evaluator", "builder", "code_reviewer", "evaluator"]
    progress = ctx.store.read_state()["evaluation_fix"]
    assert progress["attempts"] == 1
    assert progress["pending"] is None
    assert contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_evaluator_fix_cannot_add_test_only_marker_to_production(repo, tmp_path, monkeypatch):
    app_file = repo / "app.py"
    app_file.write_text("def run(): return True\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add application")
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    _install_fix_runner(ctx, repo, lambda: app_file.write_text("def run(): return True\n# SAKURA_TEST_ONLY\n"))
    monkeypatch.setattr(evaluate.testing, "run", lambda *_args, **_kwargs: 0)

    outcome = evaluate._run_fix(ctx, repo, 1, [_row()])

    assert outcome is not None and outcome.kind == "blocked"
    assert "SAKURA_TEST_ONLY" in outcome.reason


def test_evaluator_fix_must_receive_code_review_approval(repo, tmp_path, monkeypatch):
    app_file = repo / "app.py"
    app_file.write_text("def run(): return True\n")
    git(repo, "add", "app.py")
    git(repo, "commit", "-qm", "add application")
    ctx, _ = _context(repo, tmp_path, [_row()], [])
    roles = []

    class ReviewRunner:
        def run(self, request):
            roles.append(request.role)
            if request.role == "builder":
                app_file.write_text("def run(): return False\n")
                git(repo, "add", "app.py")
                git(repo, "commit", "-qm", "evaluator fix")
                result = {}
            else:
                result = {"verdict": "REVISE", "findings": ["fix needs another pass"]}
            return SimpleNamespace(exit_code=0, stalled=False, stopped=False, timed_out=False,
                                   log_path=Path("review.log"), result=result)

    object.__setattr__(ctx, "runner", ReviewRunner())
    monkeypatch.setattr(evaluate.testing, "run", lambda *_args, **_kwargs: 0)

    outcome = evaluate._run_fix(ctx, repo, 1, [_row()])

    assert outcome is not None and outcome.kind == "blocked"
    assert "code review needs revision" in outcome.reason
    assert roles == ["builder", "code_reviewer"]


def _install_fix_runner(ctx, repo, edit):
    class FixRunner:
        def run(self, request):
            assert request.role == "builder"
            edit()
            git(repo, "add", "-A")
            git(repo, "commit", "-qm", "evaluator fix")
            return SimpleNamespace(exit_code=0, stalled=False, stopped=False, timed_out=False,
                                   log_path=Path("build.log"), result={})

    object.__setattr__(ctx, "runner", FixRunner())
