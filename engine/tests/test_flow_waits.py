"""One list of waits: what every task of every project waits on from the person, from engine records only."""

import hashlib
import importlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from conftest import git
from flow_support import make_context
from sakura import engine_home
from sakura.flow import checks, contract, owner_marks, progress, waits

AWAITING = "awaiting_approval"


def _row(row_id, prover="owner", flow="open the page and look", passes=False):
    return contract.Row(row_id, "journey", "step", "cli", None, flow, False, passes, prover)


def _task(repo, task, rows=None, **state):
    ctx = make_context(repo, task=task, state=state)
    if rows:
        contract.save(ctx.store, rows)
    return ctx


def _at(moment):
    return [{"phase": "build", "status": "running", "detail": "", "at": moment}]


def test_Waits_TaskAtChecksGate_OneApprovalNamingCommandAndRowCounts(repo):
    rows = [_row("R1", "check", passes=True), _row("R2", "check"), _row("R3", "check")]
    _task(repo, "demo", rows, status=AWAITING, phase="contract",
          reason="awaiting approval: sakura flow approve demo checks (read the contract first)",
          **{checks.PENDING_KEY: {"commands": ["pytest -q"]}})

    found = waits.collect([repo])

    assert [wait.kind for wait in found] == ["approval"]
    assert found[0].title == "Run this check?" and found[0].detail.endswith("- pytest -q")
    assert "rows" not in found[0].detail
    *head, key = json.loads(found[0].id)
    assert head == ["wait", str(repo), "demo", "approval"] and waits.split_key(key)[0] == "checks"
    assert waits.split_key(key)[1], "the id names the revision the person is shown (review 17, finding 4)"


def test_Waits_CandidateContractAtChecksGate_CountsTheRowsTheLaneCounts(repo):
    """The candidate contract waits for command approval before the state binds it: the approval and the lane's
    progress read the same five rows."""
    rows = [_row(f"R{number}", "check") for number in range(5)]
    ctx = _task(repo, "demo", rows, status=AWAITING, phase="contract",
                reason="awaiting approval: sakura flow approve demo checks (read the contract first)")
    digest = hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda state: state.update({"contract_sha256": "", checks.PENDING_KEY: {
        "commands": ["pytest -q"], "contract": digest}}))

    (wait,) = waits.collect([repo])

    assert wait.title == "Run this check?" and wait.detail.endswith("- pytest -q")
    done = progress.of_task(ctx.store, ctx.store.read_state(), ctx.config)[0]
    assert (done.done, done.total) == (0, 5)


def test_Waits_TaskAtSpecGate_DetailIsTheSpecNotTheEngineLine(repo):
    ctx = _task(repo, "demo", status=AWAITING, phase="intake",
                reason=f"awaiting approval: sakura flow approve demo spec (read {repo}/spec.md first)")
    ctx.store.spec.write_text("## Goal\nAdd a version option.\x1b[31m\n")

    (wait,) = waits.collect([repo])

    assert wait.detail == "## Goal\nAdd a version option.\n"
    assert "sakura flow" not in wait.detail and str(repo) not in wait.detail


def test_Waits_TaskAtSpecGateWithoutASpec_DetailSaysSo(repo):
    _task(repo, "demo", status=AWAITING, phase="intake",
          reason="awaiting approval: sakura flow approve demo spec (read /x/spec.md first)")

    (wait,) = waits.collect([repo])

    assert wait.detail == waits.SPEC_NOT_SHOWN


def test_Waits_TaskAtSpecGateWithAHugeSpec_DetailSaysSo(repo):
    ctx = _task(repo, "demo", status=AWAITING, phase="intake",
                reason="awaiting approval: sakura flow approve demo spec (read /x/spec.md first)")
    ctx.store.spec.write_text("x" * (waits.SPEC_DETAIL_BYTES + 1))

    (wait,) = waits.collect([repo])

    assert wait.detail == waits.SPEC_NOT_SHOWN


def test_Waits_TaskAtBudgetGate_DetailKeepsTheReasonWithoutCommandOrPath(repo):
    _task(repo, "demo", status=AWAITING, phase="build",
          reason="awaiting approval: sakura flow approve demo budget (about 12 USD of 40; read /x/plan.md first)")

    (wait,) = waits.collect([repo])

    assert wait.detail == "about 12 USD of 40"


def test_Waits_TaskAtAGateWithOnlyAReadClause_DetailIsEmpty(repo):
    _task(repo, "demo", status=AWAITING, phase="build",
          reason="awaiting approval: sakura flow approve demo budget (read /x/plan.md first)")

    (wait,) = waits.collect([repo])

    assert wait.detail == ""


def test_Waits_OpenOwnerRow_OneOwnerRowWithFlowText(repo):
    _task(repo, "demo", [_row("R1", flow="look at the greeting"), _row("R2", "check")], phase="pr")

    found = waits.collect([repo])

    assert [(wait.kind, wait.task) for wait in found] == [("owner_row", "demo")]
    assert "look at the greeting" in found[0].detail
    assert json.loads(found[0].id)[3] == "owner_row" and waits.split_key(json.loads(found[0].id)[4])[0] == "R1"


@pytest.mark.parametrize("phase", ["intake", "contract", "plan", "build", "evaluate"])
def test_Waits_OwnerRowBeforeTheEngineHeadIsFinal_NotAWait(repo, phase):
    _task(repo, "demo", [_row("R1"), _row("R2", "check")], phase=phase)

    assert waits.collect([repo]) == []


@pytest.mark.parametrize("phase", ["pr", "report", "cleanup"])
def test_Waits_OwnerRowOnceTheBuildIsMerged_OneWaitPerOpenRow(repo, phase):
    rows = [_row("R1"), _row("R2", passes=True)]
    head = git(repo, "rev-parse", "HEAD")
    # R2 carries the person's ok for this contract and the engine's head (review 17, finding 7: a green row alone is
    # no longer enough to leave the wait list).
    marked = {checks.row_key(rows[1]): {"verdict": "ok", "contract": contract.rows_digest(rows), "head": head}}
    _task(repo, "demo", rows, phase=phase, base_commit=head, **{owner_marks.MARKS_KEY: marked})

    assert [(wait.kind, waits.split_key(json.loads(wait.id)[4])[0]) for wait in waits.collect([repo])] == [("owner_row", "R1")]


def test_Waits_BlockedTask_BlockedWithReason(repo):
    _task(repo, "demo", status="blocked", reason="push stalled; see the log")

    found = waits.collect([repo])

    assert [(wait.kind, wait.detail) for wait in found] == [("blocked", "Pushing the branch took too long. Check the connection to the remote, then resume the lane.")]


def test_Waits_CageDenial_NotAWait(repo):
    _task(repo, "demo", **{owner_marks.REQUESTS_KEY: [{"tool": "Bash", "run": "r1", "detail": "docker", "reason": "denied"}]})

    assert waits.collect([repo]) == []


def test_Waits_TwoProjects_OneListOrderedBySince(repo, tmp_path):
    other = tmp_path / "other"
    git(tmp_path, "init", "-q", "-b", "main", str(other))
    (other / "README").write_text("x\n")
    git(other, "add", "README")
    git(other, "-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-qm", "x")
    _task(repo, "late", status="blocked", reason="r", history=_at("2026-11-16T10:00:00+00:00"))
    _task(other, "early", status="blocked", reason="r", history=_at("2026-11-16T08:00:00+00:00"))

    found = waits.collect([repo, other])

    assert [(wait.task, wait.project) for wait in found] == [("early", "other"), ("late", "repo")]
    assert found[0].since == "2026-11-16T08:00:00+00:00"


def test_Waits_ReadTwice_SameIds(repo):
    _task(repo, "demo", [_row("R1"), _row("R2")], status="blocked", reason="r", phase="pr")

    assert [w.id for w in waits.collect([repo])] == [w.id for w in waits.collect([repo])]
    assert len({w.id for w in waits.collect([repo])}) == 3


def test_Waits_UnreadableContract_BlockedNamingTheProblem(repo):
    ctx = _task(repo, "demo", [_row("R1")])
    ctx.store.contract.write_text("not json")

    found = waits.collect([repo])

    assert [(wait.kind, json.loads(wait.id)[4]) for wait in found] == [("blocked", "contract")]
    assert "contract unreadable" in found[0].detail


# -- the acceptance harness and the canaries never touch the default engine home ----------------------------------

ACCEPTANCE = Path(__file__).resolve().parents[1] / "acceptance"
CANARIES = ("cage_canary.py", "survivor_canary.py", "loopback_canary.py")
ENGINE_HOME_VARIABLE = "SAKURA_ENGINE_HOME"


def _harness():
    sys.path.insert(0, str(ACCEPTANCE))
    try:
        return importlib.import_module("harness")
    finally:
        sys.path.remove(str(ACCEPTANCE))


def test_Harness_DefaultEngineHome_Refused(tmp_path, monkeypatch):
    harness = _harness()
    default = str(engine_home.account_root())

    with pytest.raises(SystemExit, match=ENGINE_HOME_VARIABLE):
        harness.require_private_engine_home({})
    with pytest.raises(SystemExit, match=ENGINE_HOME_VARIABLE):
        harness.require_private_engine_home({ENGINE_HOME_VARIABLE: default})
    harness.require_private_engine_home({ENGINE_HOME_VARIABLE: str(tmp_path)})


def test_Harness_DemoWithoutEngineHome_RefusedBeforeAnythingIsMade(tmp_path, monkeypatch):
    harness = _harness()
    monkeypatch.delenv(ENGINE_HOME_VARIABLE)

    with pytest.raises(SystemExit, match=ENGINE_HOME_VARIABLE):
        harness.Demo(tmp_path / "demo-root")

    assert not (tmp_path / "demo-root").exists()


@pytest.mark.parametrize("canary", CANARIES)
def test_Harness_CanaryWithoutEngineHome_ExitsWithMessageAndMakesNothing(canary, tmp_path):
    environment = {key: value for key, value in os.environ.items() if key != ENGINE_HOME_VARIABLE}
    scratch = tmp_path / "scratch"

    done = subprocess.run([sys.executable, str(ACCEPTANCE / canary), str(scratch)], env=environment,
                          capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)

    assert done.returncode != 0 and ENGINE_HOME_VARIABLE in done.stderr
    assert not scratch.exists()
