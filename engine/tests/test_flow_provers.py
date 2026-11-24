"""Contract rows name their prover; only the engine flips a row, and delivery waits for every row to be green."""

import json
import re
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from conftest import git
from flow_support import engine_head, make_context, task_workspace
from sakura import engine_home, repository
from sakura.flow import cage, cage_server, checks, config, contract, owner_marks, runner, store
from sakura.flow.phases import build_task, evaluate, pull_request, runs
from test_flow_evaluate import RunningStack
from sakura.flow import stack, workspaces

STEPS_ON = config.PresetSteps(True, True, True)
STEPS_OFF = config.PresetSteps(False, False, False)


def _row(row_id, prover="", flow="true", baseline=False):
    return contract.Row(row_id, "journey", "step", "cli", None, flow, baseline, False, prover)


def _saved(repo, rows, steps=STEPS_ON, task="demo", overrides=None):
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [31000, 31009]}},
             "contract_sha256": "", "base_commit": git(repo, "rev-parse", "HEAD"), "branch": "main",
             "approvals": {"checks": {"commands": sorted({row.flow for row in rows})}}}
    ctx = make_context(repo, own_repository=True, state=state, task=task, overrides=overrides)
    object.__setattr__(ctx, "steps", steps)
    contract.save(ctx.store, rows)
    return ctx


def _rows(ctx):
    return {row.id: row for row in contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])}


# -- parsing and validation -----------------------------------------------------------------------


def test_rows_without_prover_keep_their_saved_bytes_and_default_by_preset(repo):
    ctx = _saved(repo, [_row("R1")])
    assert "prover" not in json.loads(ctx.store.contract.read_text())["rows"][0]
    row = _rows(ctx)["R1"]
    assert contract.prover_of(row, STEPS_ON) == contract.EVALUATOR
    assert contract.prover_of(row, STEPS_OFF) == contract.CHECK


def test_explicit_prover_round_trips(repo):
    ctx = _saved(repo, [_row("R1", "owner"), _row("R2", "check")])
    assert [row.prover for row in _rows(ctx).values()] == ["owner", "check"]


def test_validate_refuses_unknown_prover_and_evaluator_rows_without_an_evaluator(repo):
    cfg = config.load(repo)
    rows = [_row("B1", "check", baseline=True), _row("R1", "magic"), _row("R2", "evaluator")]
    errors = contract.validate(rows, cfg, STEPS_OFF)
    assert any("R1: prover must be one of check, evaluator, owner" in error for error in errors)
    assert any("R2: prover evaluator needs the evaluator" in error for error in errors)
    assert not any("R2" in error for error in contract.validate(rows, cfg, STEPS_ON) if "prover evaluator" in error)


# -- check rows: the engine runs them --------------------------------------------------------------


@pytest.fixture
def app(monkeypatch, repo):
    monkeypatch.setattr(workspaces, "create",
                        lambda *_args: workspaces.Workspace("0", repo, "main", range(31000, 31010)))
    monkeypatch.setattr(stack, "start", lambda *_args: RunningStack("http://127.0.0.1:31000"))


def test_check_rows_run_by_the_engine_flip_on_exit_zero_only(repo, app):
    ctx = _saved(repo, [_row("B1", "check", "true", baseline=True), _row("R1", "check", "exit 3"),
                        _row("R2", "owner", "try it")], STEPS_OFF)
    outcome = evaluate.run(ctx)
    rows = _rows(ctx)
    assert outcome.kind == "blocked" and "check rows fail on the task branch: R1" in outcome.reason
    assert rows["B1"].passes and not rows["R1"].passes and not rows["R2"].passes
    log = ctx.store.evidence_dir / "checks" / "B1-api.log"
    assert log.read_text().startswith("exit 0")


def test_a_blocked_check_row_names_its_log_and_the_commands_the_person_has(repo, app):
    ctx = _saved(repo, [_row("B1", "check", "true", baseline=True), _row("R1", "check", "exit 3")], STEPS_OFF)

    reason = evaluate.run(ctx).reason

    task = ctx.store.task_id
    assert str(ctx.store.evidence_dir / "checks" / "R1-api.log") in reason
    assert f"`sakura flow resume {task}`" in reason and f"`sakura flow stop {task}`" in reason
    assert "`sakura flow start --new <file or text>`" in reason
    assert "fix them and resume" not in reason


def test_without_an_evaluator_passing_checks_skip_the_phase(repo, app):
    ctx = _saved(repo, [_row("B1", "check", "true", baseline=True), _row("R1", "check", "true")], STEPS_OFF)
    outcome = evaluate.run(ctx)
    assert (outcome.kind, outcome.reason) == ("skip", "check rows passed")
    assert all(row.passes for row in _rows(ctx).values())


def test_with_the_checks_switched_off_no_saved_check_command_runs(repo, app, monkeypatch):
    ctx = _saved(repo, [_row("B1", "check", "true", baseline=True), _row("R1", "check", "exit 3")], STEPS_OFF,
                 overrides={"knobs": {"checks": False}, "relax": ["knobs.checks"]})
    ran = []

    def refuse(ctx, source, url, port, rows, *args, **kwargs):
        ran.append([row.id for row in rows])
        raise AssertionError(f"the engine ran the check rows {ran[-1]} with the checks switched off")

    monkeypatch.setattr(evaluate.baseline, "run_rows", refuse)

    outcome = evaluate.run(ctx)

    assert (outcome.kind, outcome.reason) == ("skip", "checks are switched off")
    assert not any(row.passes for row in _rows(ctx).values())


class _Evaluator:
    """An evaluator double that claims PASS for every row it is handed, with evidence.

    With a context it runs the rows it is handed through the real `run_row` tool: the cage's prompt server runs
    the engine's `sakura flow run-row` for the row, as a caged Claude Code would.
    """

    def __init__(self, ctx=None):
        self.ctx = ctx
        self.rows_seen = []

    def run(self, request):
        handed = json.loads(request.values["rows"])
        self.rows_seen.append([row["id"] for row in handed])
        evidence = Path(request.values["evidence_dir"])
        evidence.mkdir(parents=True, exist_ok=True)
        verdicts = []
        for row in [*handed, {"id": "OWN", "viewport": None}, {"id": "CHK", "viewport": None}]:
            path = f"evidence/{row['id']}.txt"
            (evidence / f"{row['id']}.txt").write_text("looks fine")
            if self.ctx is not None and row in handed:
                path = self._through_the_tool(request, row["id"])
            verdicts.append({"id": row["id"], "viewport": row["viewport"], "verdict": "PASS", "evidence": path})
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, timed_out=False, log_path=Path("e.log"),
                               result={"rows": verdicts})

    def _through_the_tool(self, request, row_id):
        ctx = self.ctx
        env = runner.AgentRunner(ctx.store, ctx.config)._row_env(request, "evaluate-1")
        site = cage.RunSite(ctx.store.root, "evaluate-1", "evaluator", request.cwd,
                            repository.common_dir(ctx.repo), (engine_home.root(),), ctx.repo, env)
        prepared = cage.prepare(site, cage.Policy("default"), 0, home=Path("/home/someone"))
        answer = cage_server.run_row(json.loads(prepared.policy.read_text()), row_id)
        assert f"run-row {row_id}: exit 0" in answer, answer
        return re.search(r"evidence: (\S+)", answer).group(1)


def test_evaluator_text_cannot_flip_check_or_owner_rows(repo, app, monkeypatch):
    from sakura.flow import calibration

    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    monkeypatch.setattr(evaluate.integrate, "run_full_tests", lambda *_args: None)
    ctx = _saved(repo, [_row("EVA", "evaluator"), _row("CHK", "check", "exit 1"), _row("OWN", "owner", "try it")])
    # The row tool serves only a workspace the engine made in the task's own repository (review 10, finding 1).
    lane = task_workspace(ctx, "0", "main-lane")
    monkeypatch.setattr(workspaces, "create", lambda *_args: workspaces.Workspace("0", lane, "main-lane",
                                                                                 range(31000, 31010)))
    evaluator = _Evaluator(ctx)
    object.__setattr__(ctx, "runner", evaluator)
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: None)
    evaluate.run(ctx)
    rows = _rows(ctx)
    assert evaluator.rows_seen[0] == ["EVA"]
    # EVA passes on the engine receipt of its run_row run; the evaluator's claims for the others change nothing.
    assert rows["EVA"].passes and not rows["CHK"].passes and not rows["OWN"].passes
    assert ctx.store.read_state()["evaluator_receipts"]["EVA/api"]["exit"] == 0


def test_an_evaluator_that_never_runs_the_row_cannot_pass_it(repo, app, monkeypatch):
    from sakura.flow import calibration

    monkeypatch.setattr(calibration, "is_current", lambda *_args: True)
    monkeypatch.setattr(evaluate.integrate, "run_full_tests", lambda *_args: None)
    ctx = _saved(repo, [_row("EVA", "evaluator")])
    object.__setattr__(ctx, "runner", _Evaluator())
    monkeypatch.setattr(evaluate, "_run_fix", lambda *_args: None)
    evaluate.run(ctx)
    assert not _rows(ctx)["EVA"].passes


# -- owner rows: only the person's mark ----------------------------------------------------------


def test_mark_flips_an_owner_row_with_evidence_and_ledger(repo):
    ctx = _saved(repo, [_row("R1", "owner", "open the page and look")])
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "looks right", "cli:me")
    assert _rows(ctx)["R1"].passes
    evidence = json.loads((ctx.store.evidence_dir / "owner" / "R1.json").read_text())
    assert (evidence["verdict"], evidence["actor"], evidence["note"]) == ("ok", "cli:me", "looks right")
    assert evidence["head"] == git(repo, "rev-parse", "HEAD")
    assert "row R1 marked ok by cli:me: looks right" in ctx.store.ledger.read_text()
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ko", "", "cli:me")
    assert not _rows(ctx)["R1"].passes


@pytest.mark.parametrize("prover", ["check", "evaluator"])
def test_mark_refuses_rows_the_engine_proves(repo, prover):
    ctx = _saved(repo, [_row("R1", prover)])
    with pytest.raises(owner_marks.MarkRefused, match=f"proven by {prover}"):
        owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "", "cli:me")
    assert not _rows(ctx)["R1"].passes


def test_mark_refuses_unknown_rows_and_verdicts(repo):
    ctx = _saved(repo, [_row("R1", "owner")])
    with pytest.raises(owner_marks.MarkRefused, match="no contract row R9"):
        owner_marks.mark(ctx.store, STEPS_ON, "R9", "ok", "", "cli:me")
    with pytest.raises(owner_marks.MarkRefused, match="ok or ko"):
        owner_marks.mark(ctx.store, STEPS_ON, "R1", "yes", "", "cli:me")


def test_waiting_lists_open_owner_rows_and_denied_requests_are_information_apart(repo):
    ctx = _saved(repo, [_row("R1", "owner", "try the login"), _row("R2", "owner")])
    owner_marks.mark(ctx.store, STEPS_ON, "R2", "ok", "", "cli:me")
    ctx.store.update_state(lambda state: state.update(cage_requests=[
        {"run": "build-1", "tool": "WebFetch", "detail": "https://evil.example", "reason": "not in the allow-list"}]))
    items = owner_marks.waiting(ctx.store, STEPS_ON)
    assert items == ["owner row R1: try the login (sakura flow mark demo R1 ok|ko)"]
    assert owner_marks.denials(ctx.store.read_state()) == [
        "denied WebFetch in run build-1: https://evil.example: not in the allow-list"]


def test_flow_mark_cli_refuses_inside_an_engine_role(repo, monkeypatch, capsys):
    from sakura.flow import cli

    _saved(repo, [_row("R1", "owner")])
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["mark", "demo", "R1", "ok"]) != 0
    assert "cannot run inside an engine role" in capsys.readouterr().out


def test_flow_mark_cli_marks_and_status_lists_what_waits(repo, monkeypatch, capsys):
    from sakura.flow import cli

    ctx = _saved(repo, [_row("R1", "owner", "try it"), _row("R2", "check")])
    monkeypatch.chdir(repo)
    assert cli.main(["status", "demo"]) == 0
    assert "waiting on you: owner row R1: try it (sakura flow mark demo R1 ok|ko)" in capsys.readouterr().out
    assert cli.main(["mark", "demo", "R2", "ok"]) != 0
    assert "proven by check" in capsys.readouterr().out
    assert cli.main(["mark", "demo", "R1", "ok", "--note", "fine"]) == 0
    fresh = store.TaskStore(repo, "demo")
    assert contract.load(fresh, fresh.read_state()["contract_sha256"])[0].passes


def test_operator_brake_refuses_flow_mark_from_a_role_shell():
    from sakura.flow import brakes

    tokens = ["sakura", "flow", "mark", "demo", "R1", "ok"]
    assert brakes._operator_invocation(tokens) == "flow mark"
    assert brakes._operator_invocation(["sakura", "preset", "use", "solo"]) == "preset use"
    assert brakes._operator_invocation(["sakura", "stack", "up"]) == "stack up"


# -- delivery refuses unless every row is green ---------------------------------------------------


def _green_state(repo, ctx):
    head = git(repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state.update(full_green_head=head))
    return ctx.store.read_state(), head


@pytest.mark.parametrize("steps", [STEPS_ON, STEPS_OFF])
def test_delivery_refuses_open_rows_whatever_the_preset(repo, steps):
    ctx = _saved(repo, [_row("R1", "check")], steps)
    state, head = _green_state(repo, ctx)
    rows = list(_rows(ctx).values())
    assert pull_request._precondition(state, rows, steps, head, "demo") == "contract rows are still open"


def test_delivery_waits_for_the_person_on_owner_rows_then_goes(repo):
    ctx = _saved(repo, [_row("R1", "owner")])
    state, head = _green_state(repo, ctx)
    problem = pull_request._precondition(state, list(_rows(ctx).values()), STEPS_ON, head, "demo")
    assert problem == "waiting for the person on owner rows: sakura flow mark demo R1 ok|ko"
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "", "cli:me")
    assert pull_request._precondition(ctx.store.read_state(), list(_rows(ctx).values()), STEPS_ON, head, "demo") == ""


def test_pr_phase_refuses_before_any_push(repo, monkeypatch):
    ctx = _saved(repo, [_row("R1", "owner")])
    _green_state(repo, ctx)
    monkeypatch.setattr(pull_request.integrate, "full_tests_inputs_hash", lambda _ctx: "same")
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256="same"))
    pushed = []
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pushed.append(1))
    outcome = pull_request.run(ctx)
    assert outcome.kind == "blocked" and "waiting for the person on owner rows" in outcome.reason
    assert pushed == []


# -- a reviewer's reject blocks ------------------------------------------------------------------


def test_reject_verdict_in_any_case_blocks_with_its_findings():
    assert runs.parse_verdict({"verdict": "reject", "findings": ["unsafe"]}) == ("REJECT", ["unsafe"])
    assert runs.parse_verdict({"verdict": "REJECT", "findings": []}) == ("REJECT", [runs.REJECTED_FINDING])
    assert runs.parse_verdict({"verdict": "approve", "findings": []}) == ("APPROVE", [])
    assert runs.parse_verdict({"verdict": "maybe", "findings": []}) == ("REVISE", [runs.NO_RESULT_FINDING])


def _review_task():
    return {"id": "T1", "title": "title T1", "depends_on": [], "rows": ["R1"], "files": ["t1.txt"], "seam": False}


@pytest.mark.parametrize("fix_rounds, builder_runs", [(0, 1), (1, 2)])
def test_reject_stops_the_task_after_the_presets_fix_rounds(repo, tmp_path, fix_rounds, builder_runs):
    from test_flow_build import _commit, _ctx, _use
    from sakura.flow.phases import build

    ctx = _ctx(repo, tmp_path, [_review_task()])
    values = ctx.config.values
    object.__setattr__(ctx, "config", config.FlowConfig({**values, "knobs": {**values["knobs"], "fix_rounds": fix_rounds}},
                                                        ctx.config.origins, ctx.config.relaxations))
    rejected = {"verdict": "reject", "findings": ["deletes the audit log"]}
    runner = _use(ctx, _commit, reviews=[rejected] * 3)
    outcome = build.run(ctx)
    assert outcome.kind == "blocked"
    assert f"still needs changes after {fix_rounds} fix rounds: deletes the audit log" in outcome.reason
    assert len(runner.of("builder")) == builder_runs
    assert ctx.store.read_state()["build"]["T1"]["status"] != build_task.MERGED
    assert "review round 1: REJECT" in ctx.store.ledger.read_text()


# -- rows sharing an id must share their prover --------------------------------------------------


def test_validate_refuses_rows_sharing_an_id_with_different_provers(repo):
    cfg = config.load(repo)
    rows = [contract.Row("R1", "j", "s", "web", "1440x900", "true", False, False, "owner"),
            contract.Row("R1", "j", "s", "web", "390x844", "true", False, False, "check"),
            _row("B1", "check", baseline=True)]
    errors = contract.validate(rows, cfg, STEPS_ON)
    assert any("row R1: rows sharing an id must share one prover" in error for error in errors)


def test_mark_refuses_when_any_row_with_that_id_is_proven_by_the_engine(repo):
    rows = [contract.Row("R1", "j", "s", "web", "1440x900", "look", False, False, "owner"),
            contract.Row("R1", "j", "s", "web", "390x844", "exit 1", False, False, "check")]
    ctx = _saved(repo, rows)
    with pytest.raises(owner_marks.MarkRefused, match="proven by check"):
        owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "", "cli:me")
    assert not any(row.passes for row in contract.load(ctx.store, ctx.store.read_state()["contract_sha256"]))


# -- review 4: every pass, receipt and mark is bound to the delivery head and contract ---------------------------


def _commit_b(repo):
    (repo / "b.txt").write_text("b\n")
    git(repo, "add", "b.txt")
    git(repo, "commit", "-qm", "B")
    return git(repo, "rev-parse", "HEAD")


def test_OwnerMark_RecordsTheContractDigestAndTheHead(repo):
    ctx = _saved(repo, [_row("R1", "owner")])
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "", "cli:me")
    recorded = ctx.store.read_state()["owner_marks"]["R1/api"]
    digest = contract.rows_digest(list(_rows(ctx).values()))
    assert recorded == {"verdict": "ok", "contract": digest, "head": git(repo, "rev-parse", "HEAD")}


def test_OwnerMark_MarkOnACommitB_DeliveryRefused(repo, monkeypatch):
    ctx = _saved(repo, [_row("R1", "owner")])
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ok", "", "cli:me")
    head_b = _commit_b(repo)
    ctx.store.update_state(lambda state: state.update(full_green_head=head_b))
    rows = list(_rows(ctx).values())
    assert rows[0].passes, "the mark itself stays; it no longer covers the delivery head"
    problem = pull_request._precondition(ctx.store.read_state(), rows, STEPS_ON, head_b, "demo")
    assert problem == "waiting for the person on owner rows: sakura flow mark demo R1 ok|ko"
    monkeypatch.setattr(pull_request.integrate, "full_tests_inputs_hash", lambda _ctx: "same")
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256="same"))
    pushed = []
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pushed.append(1))
    assert pull_request.run(ctx).kind == "blocked" and pushed == []


def test_OwnerMark_AKoMark_NeverCoversDelivery(repo):
    ctx = _saved(repo, [_row("R1", "owner")])
    owner_marks.mark(ctx.store, STEPS_ON, "R1", "ko", "", "cli:me")
    contract.mark_passed(ctx.store, ctx.store.read_state(), "R1", None, ctx.store.evidence_dir / "owner" / "R1.json")
    state, head = _green_state(repo, ctx)
    assert pull_request._precondition(state, list(_rows(ctx).values()), STEPS_ON, head, "demo").startswith("waiting")


def test_Delivery_AnEvaluatorPassWithoutAReceiptForTheHead_IsRefused(repo):
    ctx = _saved(repo, [_row("R1", "evaluator")])
    evidence = ctx.store.evidence_dir / "R1.log"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text("exit 0\n")
    contract.mark_passed(ctx.store, ctx.store.read_state(), "R1", None, evidence)
    rows = list(_rows(ctx).values())
    digest = contract.rows_digest(rows)
    state, head = _green_state(repo, ctx)
    assert pull_request._precondition(state, rows, STEPS_ON, head, "demo") == "contract rows are still open"
    checks.record_receipt(ctx.store, rows[0], digest, "0" * 40, 0, evidence, checks.EVALUATOR_RECEIPTS_KEY)
    assert pull_request._precondition(ctx.store.read_state(), rows, STEPS_ON, head, "demo") == "contract rows are still open"
    checks.record_receipt(ctx.store, rows[0], digest, head, 0, evidence, checks.EVALUATOR_RECEIPTS_KEY)
    assert pull_request._precondition(ctx.store.read_state(), rows, STEPS_ON, head, "demo") == ""


def test_Evaluate_OwnerOnlyContractWithTheEvaluatorOff_InvalidatesTheMarkOnANewHead(repo):
    ctx = _saved(repo, [_row("R1", "owner")], STEPS_OFF)
    assert evaluate.run(ctx).kind == "skip"
    owner_marks.mark(ctx.store, STEPS_OFF, "R1", "ok", "", "cli:me")
    assert _rows(ctx)["R1"].passes
    _commit_b(repo)
    engine_head(ctx)  # the engine's head moves: a fix it integrated, never a branch alone
    assert evaluate.run(ctx).kind == "skip"
    assert not _rows(ctx)["R1"].passes, "a new head clears the person's earlier mark"
    assert "R1/api" not in ctx.store.read_state().get("owner_marks", {})


# -- checks turned on during a lane: the commands are an ask, not a block ---------------------------------------


def _unapproved(repo, steps=STEPS_OFF, overrides=None):
    ctx = _saved(repo, [_row("B1", "check", "true", baseline=True), _row("R1", "check", "exit 0")], steps, overrides=overrides)
    ctx.store.update_state(lambda state: state["approvals"].pop("checks"))
    return ctx


def test_ChecksNeverApproved_TheEvaluateStepAsksInsteadOfBlocking(repo, app):
    ctx = _unapproved(repo)

    outcome = evaluate.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", checks.APPROVAL_GATE)
    assert "see sakura flow status" not in outcome.reason and "changed" not in outcome.reason
    assert sorted(ctx.store.read_state()[checks.PENDING_KEY]["commands"]) == ["exit 0", "true"]
    assert not any(row.passes for row in _rows(ctx).values())  # nothing ran


def test_ChecksAsk_IsTheApprovalAskOfTheChecksGate_WithTheCommandsInPlainWords(repo, app):
    from sakura.cockpit import wait_actions
    from sakura.flow import waits

    ctx = _unapproved(repo)
    outcome = evaluate.run(ctx)
    ctx.store.update_state(lambda state: state.update(
        status="awaiting_approval", reason=f"awaiting approval: sakura flow approve {ctx.store.task_id} checks ({outcome.reason})"))

    found = [wait for wait in waits.collect_stores({repo: [ctx.store]}) if wait.kind == waits.APPROVAL]

    assert len(found) == 1 and wait_actions.WaitTarget(found[0]).subject == checks.APPROVAL_GATE
    assert found[0].title == "Run these 2 checks?"
    assert "exit 0" in found[0].detail and "true" in found[0].detail
    assert "sakura flow" not in found[0].title + found[0].detail


def test_ChecksTurnedOnAtBuild_FinalReviewAccepted_AskAppears_ApproveRunsThemAndTheLaneMovesToDeliver(repo, app, monkeypatch):
    from sakura.cockpit import wait_actions
    from sakura.flow import cli, engine, inbox, waits

    monkeypatch.setattr(engine, "_HANDLERS", {})
    ctx = _unapproved(repo, overrides={"preset": "lean", "relax": ["preset"]})
    ctx.store.update_state(lambda state: state.update(phase="build"))
    reached = []

    def build(running):
        inbox.enqueue(running.store, "step", "on", role="checks", actor="cli:alice")
        inbox.consume(running.store, running.repo, "build")
        return engine.PhaseOutcome("next")

    engine.register("build", build)
    engine.register("evaluate", lambda running: evaluate.run(running))
    engine.register("pr", lambda running: reached.append("pr") or engine.PhaseOutcome("await", gate="pr"))

    engine.advance(ctx)
    state = ctx.store.read_state()
    assert state["status"] == "awaiting_approval" and reached == []
    asked = [wait for wait in waits.collect_stores({repo: [ctx.store]}) if wait.kind == waits.APPROVAL]
    assert [wait_actions.WaitTarget(wait).subject for wait in asked] == [checks.APPROVAL_GATE]
    assert wait_actions.ACTION_KEYS[waits.APPROVAL] == ("a",)

    monkeypatch.chdir(repo)
    assert cli.main(["approve", ctx.store.task_id, "checks"]) == 0
    engine.advance(ctx)  # the lane's own store, alive across the approval: it must not read it as tampering

    assert reached == ["pr"]
    assert all(row.passes for row in _rows(ctx).values())
