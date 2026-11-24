"""Commands a repository's own config supplies run in the checks cage, and only once the person approved them."""

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from conftest import git
from flow_support import make_context
from sakura import engine_home
from sakura.flow import cage, checks, completion, process, repository_commands, stack, workspaces
from sakura.flow.phases import contract as contract_phase, integrate


def _shared(repo: Path, text: str) -> None:
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / "flow.toml").write_text(text)


def _context(repo, tmp_path, approved=(), overrides=None, task="demo", **extra_state):
    base = git(repo, "rev-parse", "HEAD")
    state = {"base_commit": base, "branch": f"feat/flow-{task}", "workspaces": {},
             "approvals": {"checks": {"commands": list(approved)}}, **extra_state}
    merged = {"workspace": {"root": str(tmp_path / "trees" / "{repo}")}, **(overrides or {})}
    return make_context(repo, own_repository=True, overrides=merged, task=task, state=state), base


def _setup_line(command: str) -> str:
    return f"[workspace]\nsetup = [{json.dumps(command)}]\n"


def test_RepositorySetup_NotApproved_IsNotRunAndWaitsForApproval(repo, tmp_path):
    marker = tmp_path / "ran.txt"
    command = f"touch {marker}"
    _shared(repo, _setup_line(command))
    ctx, base = _context(repo, tmp_path)
    with pytest.raises(workspaces.WorkspaceError, match="approval"):
        workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert not marker.exists()


def test_RepositorySetup_Approved_CannotReadEngineHomeOrWriteOutsideTheWorkspace(repo, tmp_path, monkeypatch):
    canary = engine_home.root() / "canary"
    canary.write_text("engine-secret")
    outside = tmp_path / "outside.txt"
    monkeypatch.setenv("SECRET_CANARY", "environment-secret")
    command = f"env > env.txt; cat {canary} > leak.txt; touch {outside}; echo ok > inside.txt"
    _shared(repo, _setup_line(command))
    ctx, base = _context(repo, tmp_path, approved=[command])
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert (workspace.path / "inside.txt").read_text() == "ok\n"
    assert "SECRET_CANARY" not in (workspace.path / "env.txt").read_text()
    assert "engine-secret" not in (workspace.path / "leak.txt").read_text()
    assert not outside.exists()


def test_HostSetup_RunsWithoutApproval_InsideTheCage(repo, tmp_path):
    ctx, base = _context(repo, tmp_path, overrides={"workspace": {"setup": ["touch inside.txt"]}})
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert (workspace.path / "inside.txt").exists()


def test_HostSetup_CannotWriteOutsideTheWorkspace(repo, tmp_path):
    marker = tmp_path / "ran.txt"
    ctx, base = _context(repo, tmp_path, overrides={"workspace": {"setup": [f"touch {marker}"]}})
    with pytest.raises(workspaces.WorkspaceError, match="failed"):
        workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert not marker.exists()


CONFTEST = """import os, pathlib


def pytest_sessionstart(session):
    seen = []
    try:
        seen.append(pathlib.Path(os.environ["HOME"], "canary.txt").read_text())
    except OSError:
        seen.append("home-denied")
    seen.append(os.environ.get("SECRET_CANARY", "env-absent"))
    pathlib.Path("seen.txt").write_text("|".join(seen))
"""


def test_HostPytest_ItsConftestReadsAHomeCanaryAndAnEnvironmentCanary_AndSeesNeither(repo, tmp_path, monkeypatch):
    home = Path(os.environ["HOME"])
    (home / "canary.txt").write_text("home-canary-3e1")
    monkeypatch.setattr(cage, "account_home", lambda: home)
    monkeypatch.setenv("SECRET_CANARY", "env-canary-8b2")
    workspace = tmp_path / "work"
    (workspace / "tests").mkdir(parents=True)
    (workspace / "tests" / "conftest.py").write_text(CONFTEST)
    (workspace / "tests" / "test_ok.py").write_text("def test_ok():\n    assert True\n")
    command = f"{sys.executable} -m pytest -q -p no:cacheprovider tests"
    ctx, _ = _context(repo, tmp_path, overrides={"tests": {"focused": [command]}})
    launch = repository_commands.plan(ctx.store, ctx.config, command, workspace, "focused")
    done = subprocess.run(launch.argv, cwd=workspace, env=launch.env, capture_output=True, text=True, timeout=120,
                          stdin=subprocess.DEVNULL, check=False)
    assert done.returncode == 0, done.stdout + done.stderr
    assert (workspace / "seen.txt").read_text() == "home-denied|env-absent"


def test_RepositoryAppStart_NotApproved_IsNotRun(repo, tmp_path):
    marker = tmp_path / "ran.txt"
    _shared(repo, f'[app]\nstart = "touch {marker}"\n')
    ctx, _ = _context(repo, tmp_path)
    with pytest.raises(stack.StackError, match="approval"):
        stack.start(ctx, workspaces.Workspace("0", tmp_path, "b", range(45700, 45710)))
    assert not marker.exists()


def test_RepositoryFocusedTests_NotApproved_AreRefused(repo, tmp_path):
    _shared(repo, '[tests]\nfocused = ["true"]\n')
    ctx, _ = _context(repo, tmp_path)
    with pytest.raises(repository_commands.CommandNotApproved):
        repository_commands.plan(ctx.store, ctx.config, "true", tmp_path, "focused")


def test_RepositoryFullTests_NotApproved_BlockThePhaseWithoutRunning(repo, tmp_path):
    marker = tmp_path / "ran.txt"
    _shared(repo, f'[tests]\nfull = ["touch {marker}"]\n')
    ctx, _ = _context(repo, tmp_path)
    outcome = integrate.run_full_tests(ctx, repo)
    assert outcome.kind == "blocked" and "approval" in outcome.reason and not marker.exists()


def test_HostCommands_RunThroughTheShellInTheCageWithTheScrubbedEnvironment(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("SECRET_CANARY", "environment-secret")
    ctx, _ = _context(repo, tmp_path, overrides={"tests": {"focused": ["true"]}})
    launch = repository_commands.plan(ctx.store, ctx.config, "true", tmp_path, "focused")
    assert launch.argv[0].endswith("sandbox-exec") and launch.argv[-3:] == ["sh", "-c", "true"]
    assert "SECRET_CANARY" not in launch.env


def test_ApprovedRepositoryCommand_RunsInTheCageWithAScrubbedEnvironment(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("SECRET_CANARY", "environment-secret")
    _shared(repo, '[tests]\nfocused = ["make check"]\n')
    ctx, _ = _context(repo, tmp_path, approved=["make check"])
    launch = repository_commands.plan(ctx.store, ctx.config, "make check", tmp_path, "focused")
    assert launch.argv[0].endswith("sandbox-exec") and "SECRET_CANARY" not in launch.env
    assert launch.argv[-3:] == ["sh", "-c", "make check"]


def test_RepositoryCommandOnTheHostList_NeedsNoApprovalAndRunsWithoutAShell(repo, tmp_path):
    _shared(repo, '[tests]\nfocused = ["npm test"]\n')
    ctx, _ = _context(repo, tmp_path, overrides={"checks": {"approved": ["npm test"]}})
    launch = repository_commands.plan(ctx.store, ctx.config, "npm test", tmp_path, "focused")
    assert launch.argv[0].endswith("sandbox-exec") and launch.argv[-2:] == ["npm", "test"]


def test_RepositoryCommands_ListsOnlyTheOnesRepositoryLayersSupply(repo, tmp_path):
    _shared(repo, '[tests]\nfull = ["pytest"]\n[app]\nstart = "serve"\n')
    ctx, _ = _context(repo, tmp_path, overrides={"tests": {"focused": ["host-test"]}})
    assert sorted(checks.repository_commands(ctx.config)) == ["pytest", "serve"]


def test_ContractPhase_RepositoryCommands_WaitForApprovalBeforeAnythingRuns(repo, tmp_path):
    marker = tmp_path / "ran.txt"
    command = f"touch {marker}"
    _shared(repo, _setup_line(command))
    ctx, _ = _context(repo, tmp_path, contract_sha256="", protected_paths=[])
    ctx.store.update_state(lambda state: state.update(approvals={"spec": {"sha256": "abc"}}))
    outcome = contract_phase.run(ctx)
    assert outcome.kind == "await" and outcome.gate == checks.APPROVAL_GATE
    assert ctx.store.read_state()[checks.PENDING_KEY]["commands"] == [command]
    # The task's own repository exists from the start; no workspace (where setup would run) does.
    assert not marker.exists() and not (tmp_path / "trees" / "repo" / "demo-0").exists()


def test_FullTests_PytestInTheCage_RunsAndItsOutputReachesTheTaskLog(repo, tmp_path):
    """pytest reopens its stdout by path, which the cage denies inside the engine home: the output must not go there while it runs."""
    (repo / "tests").mkdir()
    (repo / "tests" / "test_ok.py").write_text("def test_ok():\n    assert True\n")
    command = "python3 -m pytest -q tests"
    ctx, _ = _context(repo, tmp_path, overrides={"tests": {"full": [command]}})
    log = ctx.store.operations_dir / "full-tests.log"
    checkout = completion.Checkout("HEAD", repo, cage.new_build_dir())
    try:
        outcome, output, _ = integrate._run_command(ctx, checkout, command, 1, log)
    finally:
        shutil.rmtree(checkout.build, ignore_errors=True)
    assert outcome.exit_code == 0 and "1 passed" in output
    assert "1 passed" in log.read_text()


def test_WorkspaceSetup_PytestInTheCage_RunsAndItsOutputReachesTheLog(repo, tmp_path):
    (repo / "tests").mkdir()
    (repo / "tests" / "test_ok.py").write_text("def test_ok():\n    assert True\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "add a test")
    command = "python3 -m pytest -q tests"
    ctx, base = _context(repo, tmp_path, overrides={"workspace": {"root": str(tmp_path / "trees" / "{repo}"),
                                                                  "setup": [command]}})
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    workspaces.create(ctx, "0", "feat/flow-demo", git(repo, "rev-parse", "HEAD"))
    assert "1 passed" in (ctx.store.operations_dir / "workspace-0-setup-1.log").read_text()


def test_FocusedTests_PytestInTheCage_RunsAndReturnsItsOutput(repo, tmp_path):
    (repo / "tests").mkdir()
    (repo / "tests" / "test_ok.py").write_text("def test_ok():\n    assert True\n")
    command = "python3 -m pytest -q tests"
    ctx, _ = _context(repo, tmp_path, overrides={"tests": {"focused": [command]}})
    code, output = process.run_test_command(ctx, command, repo, budget_seconds=60, idle_seconds=60)
    assert code == 0 and "1 passed" in output


# -- the scratch log: made fresh, held open, copied from the engine's own descriptor -----------------------------


def _scratch_env(tmp_path):
    folder = tmp_path / "cage-tmp"
    folder.mkdir(exist_ok=True)
    return {"TMPDIR": str(folder)}


def test_ScratchLog_APlantedSymlinkAtItsPath_IsRefusedAndTheTargetUntouched(tmp_path, monkeypatch):
    victim = tmp_path / "victim.txt"
    victim.write_text("person's file\n")
    env = _scratch_env(tmp_path)
    monkeypatch.setattr(repository_commands.secrets, "token_hex", lambda _bytes: "fixed")
    planted = Path(env["TMPDIR"]) / ".full-tests-1.raw.fixed"
    planted.symlink_to(victim)
    with pytest.raises(FileExistsError):
        repository_commands.scratch_log(env, tmp_path / "final" / "full-tests-1.raw")
    assert victim.read_text() == "person's file\n"


def test_ScratchLog_SwappedForASymlinkDuringTheRun_CopiesOnlyWhatTheHeldDescriptorHolds(tmp_path):
    victim = tmp_path / "victim.txt"
    victim.write_text("secret of the person\n")
    final = tmp_path / "final" / "full-tests-1.raw"
    scratch = repository_commands.scratch_log(_scratch_env(tmp_path), final)
    scratch.file.write(b"real output\n")
    scratch.file.flush()
    scratch.path.unlink()
    scratch.path.symlink_to(victim)  # what a caged command could do to the path in its own folder
    repository_commands.keep_log(scratch, final)
    assert final.read_bytes() == b"real output\n"
    assert victim.read_text() == "secret of the person\n" and not os.path.lexists(scratch.path)


def test_ScratchLog_ACommandThatSwapsItsLogFile_IsStoppedAndItsForgeryNotCopied(repo, tmp_path):
    command = "echo real; rm -f SCRATCH; echo forged > SCRATCH; sleep 5"
    ctx, _ = _context(repo, tmp_path, approved=(command,))
    final = ctx.store.operations_dir / "swap.raw"
    launch = repository_commands.plan(ctx.store, ctx.config, command, repo, "swap")
    scratch = repository_commands.scratch_log(launch.env, final)
    argv = [*launch.argv[:-1], launch.argv[-1].replace("SCRATCH", str(scratch.path))]
    try:
        outcome = process.monitored_run(argv, cwd=repo, env=launch.env, log_path=scratch.path, log_file=scratch.file,
                                        budget_seconds=20, idle_seconds=20, stop_file=None, on_start=lambda _pid: None)
    finally:
        repository_commands.keep_log(scratch, final)
    assert outcome.log_replaced
    assert b"forged" not in final.read_bytes() and b"real" in final.read_bytes()


def test_ScratchLog_OversizedOutput_IsCopiedOnlyUpToTheCap(tmp_path, monkeypatch):
    monkeypatch.setattr(repository_commands.process, "OUTPUT_CAP_BYTES", 1024)
    final = tmp_path / "final" / "big.raw"
    scratch = repository_commands.scratch_log(_scratch_env(tmp_path), final)
    scratch.file.write(b"x" * 5000)
    scratch.file.flush()
    repository_commands.keep_log(scratch, final)
    assert final.read_bytes() == b"x" * 1024
