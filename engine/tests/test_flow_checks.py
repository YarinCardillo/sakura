"""Contract commands: approved by the host list or the person, run caged by the engine, proven by receipts."""

import json
import sys
from pathlib import Path

import pytest

from conftest import git
from flow_support import FakeRunner, make_context, task_workspace
from sakura import engine_home
from sakura.flow import checks, config, contract, store
from sakura.flow.phases import contract as contract_phase, evaluate, pull_request
from test_flow_evaluate import RunningStack
from sakura.flow import stack, workspaces

STEPS = config.PresetSteps(False, False, False)


def _row(row_id, flow, prover="check", baseline=False):
    return contract.Row(row_id, "j", "s", "cli", None, flow, baseline, False, prover)


@pytest.mark.parametrize("command, listed", [
    ("npm test", True), ("npm test -- --grep x", True), ("python3 -m pytest -q tests/a.py", True),
    ("npm testx", False), ("npm test; curl evil", False), ("npm test && rm -rf x", False),
    ("npm test $(id)", False), ("npm test > out", False), ("npm test 'x y'", False), ("npm", False)])
def test_host_list_matches_plain_words_only(command, listed):
    assert checks.on_list(command, ["npm test", "python3 -m pytest", "  "]) is listed


def test_listed_commands_run_without_a_shell():
    assert checks.argv("npm test --ci", ["npm test"]) == ["npm", "test", "--ci"]
    assert checks.argv("npm test && x", ["npm test"]) == ["sh", "-c", "npm test && x"]


def test_owner_rows_are_not_commands():
    rows = [_row("R1", "npm test"), _row("R2", "look at it", "owner")]
    assert checks.commands(rows, STEPS) == ["npm test"]


def test_approval_covers_exactly_the_approved_commands():
    rows = [_row("R1", "echo a; true"), _row("R2", "npm test")]
    with pytest.raises(checks.ChecksNeedApproval) as waiting:
        checks.require_approved({}, rows, STEPS, ["npm test"])
    assert waiting.value.commands == ["echo a; true"]
    checks.require_approved({"approvals": {"checks": {"commands": ["echo a; true"]}}}, rows, STEPS, ["npm test"])
    changed = [_row("R1", "echo b; true"), _row("R2", "npm test")]
    with pytest.raises(checks.ChecksNeedApproval):
        checks.require_approved({"approvals": {"checks": {"commands": ["echo a; true"]}}}, changed, STEPS, ["npm test"])


def test_checks_approved_is_a_host_setting(repo):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text('[checks]\napproved = ["sh"]\n')
    with pytest.raises(config.FlowConfigError, match="checks.approved"):
        config.load(repo)


# -- the contract phase waits for the person, then goes on with the same rows ----------------------------


FLOW_NEW = f"{sys.executable} -c \"import sys; sys.exit(1)\""
FLOW_OLD = f"{sys.executable} -c \"pass\""


def _phase(repo, tmp_path, approved=(), new_flow=FLOW_NEW, old_flow=FLOW_OLD, task="demo"):
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}")}, "tests": {"focused": ["true"]},
                 "app": {"start": "", "ready_url": ""}, "checks": {"approved": list(approved)}}
    state = {"base_commit": git(repo, "rev-parse", "HEAD"), "branch": f"feat/flow-{task}", "workspaces": {},
             "contract_sha256": "", "protected_paths": [], "approvals": {"spec": {"sha256": "abc"}}}
    rows = [{"id": "R1", "journey": "J1", "step": "new", "kind": "cli", "viewport": None, "flow": new_flow,
             "baseline": False, "passes": False, "prover": "check"},
            {"id": "B1", "journey": "J1", "step": "old", "kind": "cli", "viewport": None, "flow": old_flow,
             "baseline": True, "passes": False, "prover": "check"}]
    holder = {}

    def planner(request):
        if request.role != "planner":
            return
        (request.cwd / "e2e").mkdir(exist_ok=True)
        (request.cwd / "e2e" / "flow.py").write_text("# flow\n")
        holder["ctx"].store.contract.write_text(json.dumps({"rows": rows}))

    runner = FakeRunner(planner, by_role={"code_reviewer": {"verdict": "APPROVE", "findings": []}})
    ctx = make_context(repo, runner=runner, overrides=overrides, state=state, task=task)
    holder["ctx"] = ctx
    ctx.store.spec.write_text("## Journeys\n- J1\n")
    return ctx


def _awaiting_checks(ctx):
    """What the engine records when the contract phase waits at the checks gate (an approval is accepted only then)."""
    ctx.store.update_state(lambda state: state.update(
        status="awaiting_approval", reason=f"awaiting approval: sakura flow approve {ctx.store.task_id} checks"))


def _planner_runs(ctx):
    return len([request for request in ctx.runner.requests if request.role == "planner"])


def test_unapproved_commands_stop_the_contract_before_any_run(repo, tmp_path):
    ctx = _phase(repo, tmp_path)
    outcome = contract_phase.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "checks")
    pending = ctx.store.read_state()[checks.PENDING_KEY]
    assert pending["commands"] == sorted([FLOW_NEW, FLOW_OLD])
    assert not (ctx.store.evidence_dir / "contract-base").exists()
    assert "contract commands need your approval" in checks.waiting_line(ctx.store.read_state(), "demo")


def test_approval_goes_on_with_the_same_rows_without_a_new_planner_run(repo, tmp_path, monkeypatch, capsys):
    from sakura.flow import cli

    ctx = _phase(repo, tmp_path)
    contract_phase.run(ctx)
    _awaiting_checks(ctx)
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "demo", "checks"]) == 0
    approved = store.TaskStore(repo, "demo")
    planner_runs = _planner_runs(ctx)
    object.__setattr__(ctx, "store", approved)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert _planner_runs(ctx) == planner_runs
    assert approved.read_state()["contract_sha256"]


def test_remember_adds_the_exact_commands_and_later_contracts_run_them_without_asking(repo, tmp_path, monkeypatch, capsys):
    from sakura.flow import cli

    plain = f"{sys.executable} -c pass"
    ctx = _phase(repo, tmp_path, new_flow=f"{sys.executable} -c 'import sys; sys.exit(1)'", old_flow=plain)
    assert contract_phase.run(ctx).gate == "checks"
    _awaiting_checks(ctx)
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "demo", "checks", "--remember"]) == 0
    out = capsys.readouterr().out
    assert "remembered 1 commands" in out and "1 commands use shell syntax" in out
    # Only the plain-words command is remembered, exactly as written; the quoted one stays per task.
    assert config.load(repo).values["checks"]["remembered"] == [plain]
    later = _phase(repo, tmp_path, new_flow=plain + " --extra", old_flow=plain, task="later")
    outcome = contract_phase.run(later)
    assert outcome.gate == "checks", outcome  # an added argument is a different command
    assert checks.PENDING_KEY in later.store.read_state() and later.store.read_state()[checks.PENDING_KEY]["commands"] \
        == [plain + " --extra"]


def test_a_remembered_command_needs_no_approval_in_a_later_contract(repo, tmp_path):
    from sakura import engine_home

    plain = f"{sys.executable} -c pass"
    checks.remember(repo, [plain, "echo a; true"])
    assert engine_home.remembered_checks_file(repo).read_text().count(plain) == 1
    ctx = _phase(repo, tmp_path, new_flow=plain, old_flow=plain)
    outcome = contract_phase.run(ctx)
    assert checks.PENDING_KEY not in ctx.store.read_state()
    assert outcome.gate != "checks"


def test_remember_twice_keeps_one_entry_and_approval_without_it_remembers_nothing(repo, tmp_path, monkeypatch):
    from sakura.flow import cli

    checks.remember(repo, ["npm test"])
    assert checks.remember(repo, ["npm   test", "npm run lint"]) == (["npm test", "npm run lint"], [])
    assert config.load(repo).values["checks"]["remembered"] == ["npm test", "npm run lint"]
    ctx = _phase(repo, tmp_path)
    contract_phase.run(ctx)
    _awaiting_checks(ctx)
    monkeypatch.chdir(repo)
    before = config.load(repo).values["checks"]["remembered"]
    assert cli.main(["approve", "demo", "checks"]) == 0
    assert config.load(repo).values["checks"]["remembered"] == before


def test_remember_belongs_to_the_checks_gate_and_the_host(repo, monkeypatch, capsys):
    from sakura.flow import cli

    monkeypatch.chdir(repo)
    assert cli.main(["approve", "demo", "spec", "--remember"]) != 0
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text('[checks]\nremembered = ["sh"]\n')
    with pytest.raises(config.FlowConfigError, match="checks.remembered may only be set by the host"):
        config.load(repo)


def test_the_waiting_line_offers_remember(repo, tmp_path):
    ctx = _phase(repo, tmp_path)
    contract_phase.run(ctx)
    assert "--remember" in checks.waiting_line(ctx.store.read_state(), "demo")


def test_commands_on_the_host_list_need_no_approval(repo, tmp_path):
    ctx = _phase(repo, tmp_path, approved=["false", "true"], new_flow="false", old_flow="true")
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "next", outcome.reason
    assert checks.PENDING_KEY not in ctx.store.read_state()


def test_shell_text_is_never_matched_by_the_host_list(repo, tmp_path):
    ctx = _phase(repo, tmp_path, approved=[sys.executable])
    assert contract_phase.run(ctx).gate == "checks"


# -- the engine runs check rows caged and keeps receipts ------------------------------------------------


@pytest.fixture
def app(monkeypatch, repo):
    monkeypatch.setattr(workspaces, "create", lambda ctx, *_args, **_kwargs: workspaces.Workspace(
        "0", _zero(ctx), "main", range(31000, 31010)))
    monkeypatch.setattr(stack, "start", lambda *_args: RunningStack("http://127.0.0.1:31000"))


def _evaluated(repo, rows):
    """A task at evaluation whose workspace 0, a worktree of the task's own repository, is on the base commit."""
    flows = sorted({row.flow for row in rows})
    state = {"workspaces": {}, "contract_sha256": "", "base_commit": git(repo, "rev-parse", "HEAD"),
             "branch": "main", "approvals": {"checks": {"commands": flows}}}
    ctx = make_context(repo, state=state)
    task_workspace(ctx)
    ctx.store.update_state(lambda current: current["workspaces"]["0"].update(ports=[31000, 31009]))
    object.__setattr__(ctx, "steps", STEPS)
    contract.save(ctx.store, rows)
    return ctx


def _zero(ctx):
    return Path(ctx.store.read_state()["workspaces"]["0"]["path"])


def test_check_rows_leave_receipts_and_run_caged(repo, app, tmp_path):
    outside = tmp_path / "outside.txt"
    ctx = _evaluated(repo, [_row("B1", "true", baseline=True), _row("R1", f"touch {outside}")])
    evaluate.run(ctx)
    receipts = ctx.store.read_state()[checks.RECEIPTS_KEY]
    head = git(_zero(ctx), "rev-parse", "HEAD")
    assert receipts["B1/api"]["exit"] == 0 and receipts["B1/api"]["revision"] == head
    assert receipts["R1/api"]["exit"] != 0 and not outside.exists()
    rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    assert receipts["B1/api"]["contract"] == contract.rows_digest(rows)


def test_delivery_wants_a_receipt_for_a_check_row_not_just_its_flag(repo):
    ctx = _evaluated(repo, [_row("R1", "true")])
    evidence = ctx.store.evidence_dir / "forged.log"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text("exit 0")
    contract.mark_passed(ctx.store, ctx.store.read_state(), "R1", None, evidence)
    head = git(repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state.update(full_green_head=head))
    rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    assert rows[0].passes
    assert pull_request._precondition(ctx.store.read_state(), rows, STEPS, head, "demo") == "contract rows are still open"


def test_changed_contract_commands_stop_the_engine_checks(repo, app):
    ctx = _evaluated(repo, [_row("R1", "true")])
    ctx.store.update_state(lambda state: state["approvals"].update(checks={"commands": ["false"]}))
    outcome = evaluate.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "checks") and "to approve" in outcome.reason


# -- check rows run on a clean, captured revision, in a checkout the engine made --------------------------


def test_CheckRow_RunsInADetachedCheckoutOfTheCapturedRevision_NotInTheLiveWorkspace(repo, app):
    ctx = _evaluated(repo, [_row("R1", "pwd")])
    evaluate.run(ctx)
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert ".sakura-row-" in log.split("\n", 2)[-1]
    assert ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]["revision"] == git(_zero(ctx), "rev-parse", "HEAD")
    assert not [path for path in _zero(ctx).parent.iterdir() if path.name.startswith(".sakura-row-")]


def test_CheckRow_AStrayFileInWorkspaceZero_IsNotSeenAndDoesNotBlock(repo, app):
    # Review 11: the rows run on the engine's head; nothing of workspace 0 takes part.
    ctx = _evaluated(repo, [_row("R1", "test ! -e stray.txt")])
    (_zero(ctx) / "stray.txt").write_text("uncommitted\n")
    evaluate.run(ctx)
    receipt = ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]
    assert receipt["exit"] == 0 and receipt["revision"] == ctx.store.read_state()["base_commit"]


def test_CheckRow_CommandThatCommitsDuringItsRun_IsRefusedTheWrite(repo, app):
    ctx = _evaluated(repo, [_row("R1", "true")])
    zero = _zero(ctx)
    flows = [f"git -C {zero} commit --allow-empty -q -m moved-by-the-check"]
    contract.save(ctx.store, [_row("R1", flows[0])])
    ctx.store.update_state(lambda state: state["approvals"].update(checks={"commands": flows}))
    before = git(zero, "rev-parse", "HEAD")
    evaluate.run(ctx)
    assert git(zero, "rev-parse", "HEAD") == before, "a row writes neither the workspace nor the Git folder"
    assert ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]["exit"] != 0
    rows = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])
    assert not rows[0].passes


def test_CheckRow_ACommitAddedToWorkspaceZeroDuringTheRun_IsIgnored_AndTheReceiptNamesTheEnginesHead(repo, app,
                                                                                                     monkeypatch):
    ctx = _evaluated(repo, [_row("R1", "true")])
    real_run_rows = evaluate.baseline.run_rows
    moved = []

    def moving(*args, **kwargs):
        codes = real_run_rows(*args, **kwargs)
        git(_zero(ctx), "commit", "--allow-empty", "-q", "-m", "moved-while-the-check-ran")
        moved.append(git(_zero(ctx), "rev-parse", "HEAD"))
        return codes

    monkeypatch.setattr(evaluate.baseline, "run_rows", moving)
    evaluate.run(ctx)
    receipt = ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]
    assert receipt["revision"] == ctx.store.read_state()["base_commit"] != moved[0] and receipt["exit"] == 0


# -- check rows run on read-only source: what the receipt names is what ran -----------------------------


def _committed(repo, name, text):
    (repo / name).write_text(text)
    git(repo, "add", name)
    git(repo, "commit", "-qm", f"add {name}")


def test_CheckRow_ThatRewritesASourceFileTestsItAndRestoresIt_GetsNoPassingReceipt(repo, app):
    _committed(repo, "src.txt", "original\n")
    flow = "printf 'patched\\n' > src.txt && grep -q patched src.txt && git checkout -- src.txt"
    ctx = _evaluated(repo, [_row("R1", flow)])
    evaluate.run(ctx)
    receipt = ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]
    assert receipt["exit"] != 0, "the rewrite of a tracked file was not refused"
    assert not contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0].passes


def test_CheckRow_ACheckoutChangedDuringTheRun_GetsNoReceipt(repo, app, monkeypatch):
    _committed(repo, "src.txt", "original\n")
    ctx = _evaluated(repo, [_row("R1", "true")])

    def substituted(_ctx, checkout, *_args, **_kwargs):
        (checkout / "src.txt").write_text("patched\n")  # whatever got past the cage, the engine checks the tree
        return {"R1": 0}

    monkeypatch.setattr(evaluate.baseline, "run_rows", substituted)
    outcome = evaluate.run(ctx)
    assert outcome.kind == "blocked" and "src.txt" in outcome.reason
    assert not ctx.store.read_state().get(checks.RECEIPTS_KEY)


def test_CheckRow_ReadOnlySource_StillGetsWritableCachesBuildOutputAndTmp(repo, app):
    _committed(repo, "mod.py", "VALUE = 1\n")
    flow = (f"touch \"$TMPDIR/t\" && mkdir -p \"$CARGO_TARGET_DIR\" && touch \"$npm_config_cache/c\" && "
            f"{sys.executable} -c 'import mod, sys; sys.exit(mod.VALUE - 1)' && test -d \"$PYTHONPYCACHEPREFIX\"")
    ctx = _evaluated(repo, [_row("R1", flow)])
    evaluate.run(ctx)
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]["exit"] == 0, log


def test_WaitingLine_ShowsAContractCommandWithItsControlCharactersEscaped():
    state = {checks.PENDING_KEY: {"commands": ["echo ok\x1b]0;owned\x07\nrm -rf x"]}}
    line = checks.waiting_line(state, "demo")
    assert "\x1b" not in line and "\x07" not in line and "\n" not in line
    assert "\\x1b]0;owned\\x07\\x0arm -rf x" in line


def test_ContractRow_WithAnEscapeSequence_IsRefusedAtLoadAndNeverRemembered(repo, tmp_path):
    hostile = "echo ok\x1b]0;owned\x07"
    ctx = _phase(repo, tmp_path, new_flow=hostile)
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "blocked" and "control character" in outcome.reason
    assert checks.PENDING_KEY not in ctx.store.read_state()  # it never reached the approval gate
    remembered, skipped = checks.remember(repo, [hostile, "npm test"])
    assert remembered == ["npm test"] and skipped == [hostile]
    assert "\x1b" not in engine_home.remembered_checks_file(repo).read_text()
    config.load(repo)  # the host config the remembered list feeds still loads


def test_the_waiting_line_offers_local_sockets_and_says_what_it_risks(repo, tmp_path):
    ctx = _phase(repo, tmp_path)
    contract_phase.run(ctx)
    line = checks.waiting_line(ctx.store.read_state(), "demo")
    assert "--local-sockets" in line
    assert "high loopback ports" in line and "debuggers" in line and "remembered for the project" in line


def test_approve_local_sockets_is_remembered_for_the_project_and_read_back_as_host_config(repo, tmp_path, monkeypatch, capsys):
    from sakura import engine_home
    from sakura.flow import cli

    ctx = _phase(repo, tmp_path)
    contract_phase.run(ctx)
    monkeypatch.chdir(repo)
    assert config.load(repo).values["checks"]["local_sockets"] is False
    assert cli.main(["approve", "demo", "checks", "--local-sockets"]) == 0
    assert "local sockets" in capsys.readouterr().out
    assert "local_sockets = true" in engine_home.remembered_checks_file(repo).read_text()
    assert config.load(repo).values["checks"]["local_sockets"] is True


def test_remembering_commands_keeps_the_local_sockets_decision(repo, tmp_path, monkeypatch):
    from sakura.flow import cli

    ctx = _phase(repo, tmp_path, new_flow="false", old_flow="true")
    contract_phase.run(ctx)
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "demo", "checks", "--remember", "--local-sockets"]) == 0
    values = config.load(repo).values["checks"]
    assert values["local_sockets"] is True and sorted(values["remembered"]) == ["false", "true"]
    assert cli.main(["approve", "demo", "spec", "--local-sockets"]) != 0


# -- review 6: the approval counts commands and rows correctly ---------------------------------------------------


def test_the_approval_counts_the_commands_and_the_rows_they_cover(repo, tmp_path):
    ctx = _phase(repo, tmp_path, new_flow="false", old_flow="false")
    outcome = contract_phase.run(ctx)
    assert outcome.reason == "1 command for 2 rows to approve; see sakura flow status"
    ledger = ctx.store.ledger.read_text()
    assert "1 command for 2 rows wait for the person's approval" in ledger


def test_the_approval_counts_two_commands_for_two_rows(repo, tmp_path):
    ctx = _phase(repo, tmp_path)
    assert contract_phase.run(ctx).reason == "2 commands for 2 rows to approve; see sakura flow status"


def test_the_summary_separates_commands_that_come_from_the_repository_config():
    from sakura.flow import contract as contract_module

    rows = [contract_module.Row(f"R{index}", "J1", "s", "cli", None, "pytest -q", False, False, "check") for index in range(10)]
    summary = checks.pending_summary(["pytest -q", "make setup"], rows, config.PresetSteps(True, True, True))
    assert summary == "2 commands for 10 rows, 1 of them from the repository's config"


def test_CheckRow_ThatWritesNextToItsScript_FailsWithAMessageNamingTheReadOnlyCheckoutAndTheWritableFolder(repo, app):
    (repo / "e2e").mkdir()
    _committed(repo, "e2e/check.sh", 'echo ok > "$(dirname "$0")/result.txt"\n')
    ctx = _evaluated(repo, [_row("R1", "sh e2e/check.sh")])
    evaluate.run(ctx)
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert ctx.store.read_state()[checks.RECEIPTS_KEY]["R1/api"]["exit"] != 0
    assert "the checkout this row ran in is read-only" in log and "$TMPDIR" in log
    assert "read-only" in ctx.store.ledger.read_text().split("check R1:")[1].splitlines()[0]


def test_CheckRow_ThatFailsForAnotherReason_GetsNoReadOnlyMessage(repo, app):
    ctx = _evaluated(repo, [_row("R1", "echo nope; false")])
    evaluate.run(ctx)
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert "read-only" not in log
    assert "read-only" not in ctx.store.ledger.read_text()


# -- a row whose own sockets the cage refused: the person is told the exact command, nobody edits a config ----------

OWN_SOCKET_FLOW = f"{sys.executable} -c \"import socket; s = socket.socket(); s.bind(('127.0.0.1', 0)); print('bound')\""


def test_CheckRow_ThatOpensItsOwnSocket_IsBlockedWithTheExactApprovalCommandAndItsRisk(repo, app):
    ctx = _evaluated(repo, [_row("R1", OWN_SOCKET_FLOW)])
    outcome = evaluate.run(ctx)
    assert outcome.kind == "blocked"
    assert "R1" in outcome.reason and "own loopback sockets" in outcome.reason
    assert f"sakura flow approve {ctx.store.task_id} checks --local-sockets" in outcome.reason
    assert "high loopback ports" in outcome.reason
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert "read-only" not in log, "a refused socket is not a refused write"


def test_Approve_LocalSocketsAloneIsAccepted_WhenNoCommandWaits_AndTheNextRunAllowsTheSocket(repo, app, monkeypatch, capsys):
    from sakura.flow import cli

    ctx = _evaluated(repo, [_row("R1", OWN_SOCKET_FLOW)])
    assert evaluate.run(ctx).kind == "blocked"
    monkeypatch.chdir(repo)
    assert cli.main(["approve", ctx.store.task_id, "checks", "--local-sockets"]) == 0
    assert "local sockets approved" in capsys.readouterr().out
    assert config.load(repo).values["checks"]["local_sockets"] is True


def test_Approve_ChecksWithNothingWaitingAndNoFlag_IsStillRefused(repo, app, monkeypatch, capsys):
    from sakura.flow import cli

    ctx = _evaluated(repo, [_row("R1", "true")])
    _awaiting_checks(ctx)
    monkeypatch.chdir(repo)
    assert cli.main(["approve", ctx.store.task_id, "checks"]) != 0
    assert "no contract commands wait for approval" in capsys.readouterr().out


def test_CheckRow_ThatFailsForAnotherPermissionReason_IsNotTakenForASocketDenial(repo, app):
    ctx = _evaluated(repo, [_row("R1", "echo 'PermissionError: Operation not permitted'; false")])
    assert "local-sockets" not in evaluate.run(ctx).reason


def _receipted_row(repo):
    ctx = make_context(repo)
    row = contract.Row("C1", "j", "s", "cli", None, "true", False, True, "check")
    log = ctx.store.evidence_dir / "checks" / "C1-api.log"
    log.parent.mkdir(parents=True)
    log.write_text("exit 0\n$ true\n")
    checks.record_receipt(ctx.store, row, "d" * 64, "r" * 40, 0, log)
    return ctx, row, log


def test_ReceiptPasses_OnceTheLogItNamesWasReplaced_IsFalse(repo):
    # Review 14, finding 3: a receipt is consumed only with the very log it recorded.
    ctx, row, log = _receipted_row(repo)
    assert checks.receipt_passes(ctx.store.read_state(), row, "d" * 64, "r" * 40, root=ctx.store.root)
    log.write_text("exit 0\n$ true\nanother run's output\n")
    assert not checks.receipt_passes(ctx.store.read_state(), row, "d" * 64, "r" * 40, root=ctx.store.root)

