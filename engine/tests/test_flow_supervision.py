"""Every caged command and role runs in a session and process group the engine made for it, tagged with a nonce: its
result counts only once its output reached EOF and nothing of the group, nor any process carrying the nonce, is
left. Otherwise the engine ends all of it and the step fails."""

import errno
import io
import os
import select
import re
import signal
import subprocess
import sys
import textwrap
import threading
from types import SimpleNamespace

import pytest

from conftest import git
from flow_support import make_context, task_workspace
from process_support import while_registered
from sakura.flow import checks, config, contract, process, runner
from sakura.flow.phases import evaluate, runs

STEPS_OFF = config.PresetSteps(False, False, False)
GONE_SECONDS = 10


@pytest.fixture(autouse=True)
def short_settle(monkeypatch):
    """A group gets one second to go after its leader exits: the late writers below wait three."""
    monkeypatch.setattr(process, "SETTLE_SECONDS", 1)


def _gone(pid):
    if not process.is_alive(pid):
        return True
    try:
        if hasattr(select, "kqueue"):
            with select.kqueue() as queue:
                event = select.kevent(pid, filter=select.KQ_FILTER_PROC, flags=select.KQ_EV_ADD, fflags=select.KQ_NOTE_EXIT)
                return bool(queue.control([event], 1, GONE_SECONDS))
        descriptor = os.pidfd_open(pid)
    except OSError as error:
        if error.errno == errno.ESRCH:
            return True
        raise
    try:
        return bool(select.select([descriptor], [], [], GONE_SECONDS)[0])
    finally:
        os.close(descriptor)


def _run(tmp_path, argv, **kwargs):
    options = dict(cwd=tmp_path, env={"PATH": os.environ["PATH"]}, log_path=tmp_path / "run.log", budget_seconds=30,
                   idle_seconds=30, stop_file=None, on_start=lambda pid: None)
    options.update(kwargs)
    return process.monitored_run(argv, **options)


def test_MonitoredRun_ACommandThatBackgroundsSleepAndExitsZero_FailsAndNoSleepRemains(tmp_path):
    pid_file = tmp_path / "child.pid"
    outcome = _run(tmp_path, ["sh", "-c", f"sleep 300 & echo $! > {pid_file}; exit 0"])
    assert outcome.left_running and outcome.exit_code is None
    assert _gone(int(pid_file.read_text()))


@pytest.fixture
def surviving_child(monkeypatch):
    now = SimpleNamespace(value=0)
    child = SimpleNamespace(pid=742, pgid=741, nonce="", marked=True, running=True, signals=[])
    parent = SimpleNamespace(pid=741, returncode=0, stdin=io.BytesIO(), wait=lambda: 0, poll=lambda: 0)
    eof = threading.Event()

    def launch(_argv, _cwd, _env, _log, _nice, nonce, *, piped):
        assert piped
        child.nonce = nonce
        if child.pgid != parent.pid:
            eof.set()
        return parent, SimpleNamespace(eof=eof, close=lambda: None)

    def ps(argv, **_kwargs):
        assert argv[1:] == list(process.PS_GROUP_ARGUMENTS)
        mark = f"{process.NONCE_VARIABLE}={child.nonce}" if child.marked else ""
        rows = f"{child.pid} {child.pgid} S {mark}\n" if child.running else ""
        return subprocess.CompletedProcess(argv, 0, stdout=rows)

    def kill(pid, sig):
        assert pid == child.pid
        if not child.running:
            raise ProcessLookupError
        child.signals.append(sig)
        child.running = False
        eof.set()

    def killpg(pid, sig):
        assert pid == parent.pid
        if child.running and child.pgid == pid:
            kill(child.pid, sig)

    monkeypatch.setattr(process, "_start", launch)
    monkeypatch.setattr(process, "_has_exited", lambda *_: True)
    monkeypatch.setattr(process, "subprocess", SimpleNamespace(run=ps, DEVNULL=subprocess.DEVNULL))
    monkeypatch.setattr(process, "os", SimpleNamespace(getpid=os.getpid, getpgrp=os.getpgrp, kill=kill, killpg=killpg))
    monkeypatch.setattr(process, "time", SimpleNamespace(
        monotonic=lambda: now.value, sleep=lambda elapsed: setattr(now, "value", now.value + elapsed),
    ))
    return child


def test_MonitoredRun_AChildThatWritesAfterTheParentExits_FailsAndNeverWrites(tmp_path, surviving_child):
    surviving_child.marked = False
    outcome = _run(tmp_path, ["fixture-parent"])
    assert outcome.left_running and outcome.exit_code is None
    assert surviving_child.signals == [signal.SIGTERM] and not surviving_child.running
    log = tmp_path / "run.log"
    if surviving_child.running:
        log.write_text("late")
    assert log.read_bytes() == b""


def test_MonitoredRun_AChildThatLeftTheGroupAndClosedItsOutput_IsFoundByItsNonceAndEnded(tmp_path, surviving_child):
    surviving_child.pgid = surviving_child.pid
    outcome = _run(tmp_path, ["fixture-parent"])
    assert outcome.left_running and outcome.exit_code is None
    assert surviving_child.signals == [signal.SIGTERM] and not surviving_child.running


def test_MonitoredRun_AChildThatEndsBeforeItsParent_IsAccepted(tmp_path):
    outcome = _run(tmp_path, ["sh", "-c", "sleep 0.2 & wait; echo done"])
    assert outcome.exit_code == 0 and not outcome.left_running
    assert "done" in (tmp_path / "run.log").read_text()


def test_MonitoredRun_TheRunLeadsItsOwnGroupAndCarriesANonce(tmp_path):
    script = "import os; print(os.getpgid(0) == os.getpid(), os.getsid(0) == os.getpid(), os.environ.get(sys.argv[1]))"
    outcome = _run(tmp_path, [sys.executable, "-c", f"import sys; {script}", process.NONCE_VARIABLE])
    leader, session, nonce = (tmp_path / "run.log").read_text().split()
    assert outcome.exit_code == 0 and leader == "True" and session == "True"
    assert re.fullmatch(r"[0-9a-f]{32}", nonce)


def test_MonitoredRun_AStalledRunEndsItsWholeGroup(tmp_path):
    pid_file = tmp_path / "child.pid"
    script = f"(trap '' TERM; sleep 300) & echo $! > {pid_file}; sleep 300"
    outcome = _run(tmp_path, ["sh", "-c", script], idle_seconds=1)
    assert outcome.stalled
    assert _gone(int(pid_file.read_text()))


# -- the steps that use it -----------------------------------------------------------------------------------------


def _check_context(repo, flow):
    row = contract.Row("R1", "j", "s", "cli", None, flow, False, False, "check")
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [41990, 41999]}},
             "contract_sha256": "", "base_commit": git(repo, "rev-parse", "HEAD"), "branch": "main",
             "approvals": {"checks": {"commands": [flow]}}}
    ctx = make_context(repo, state=state, overrides={"app": {"start": ""}}, own_repository=True)
    object.__setattr__(ctx, "steps", STEPS_OFF)
    contract.save(ctx.store, [row])
    return ctx


def test_CheckRow_ThatBackgroundsSleep_FailsTheStepWithNoReceiptAndNoSleepRemains(repo):
    ctx = _check_context(repo, "sleep 300 & echo child=$!")
    outcome = evaluate.run(ctx)
    assert outcome.kind == "blocked" and "left processes running" in outcome.reason
    assert checks.RECEIPTS_KEY not in ctx.store.read_state() or not ctx.store.read_state()[checks.RECEIPTS_KEY]
    log = (ctx.store.evidence_dir / "checks" / "R1-api.log").read_text()
    assert _gone(int(re.search(r"child=(\d+)", log).group(1)))


def test_Role_ThatLeavesABackgroundChild_FailsThePhaseAndTheChildIsEnded(repo, tmp_path):
    agent = tmp_path / "agent.py"
    agent.write_text(textwrap.dedent("""
        import subprocess, sys
        sys.stdin.read()
        child = subprocess.Popen(["sleep", "300"])
        print(f"child={child.pid}")
        print('SAKURA_RESULT {"verdict": "APPROVE"}')
    """))
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} {agent}"}})
    request = runner.RunRequest("planner", "intake", "hello", {}, task_workspace(ctx))
    result = runner.AgentRunner(ctx.store, ctx.config).run(request)
    failure = runs.failure_outcome(result, "planner", "intake")
    assert result.left_running and failure.kind == "blocked" and "left processes running" in failure.reason
    assert _gone(int(re.search(r"child=(\d+)", result.log_path.read_text()).group(1)))


# -- the engine signals only a group it can confirm is the run's own ---------------------------------------------


def _orphaned_group(env_extra=None):
    """A group whose leader already exited and whose one member (`sleep 300`) is still there: (pgid, member)."""
    leader = subprocess.Popen(["sh", "-c", "sleep 300 & echo $!"], stdout=subprocess.PIPE, text=True,
                              stdin=subprocess.DEVNULL, start_new_session=True,
                              env={"PATH": os.environ["PATH"], **(env_extra or {})})
    member = int(leader.stdout.readline())
    leader.wait()
    return leader.pid, member


def _cleanup(pid):
    if process.is_alive(pid):
        os.kill(pid, signal.SIGKILL)


def test_EndGroup_AnOrphanedGroupWithNoNonceCarrier_IsNeverSignalled():
    pgid, member = _orphaned_group()
    try:
        process.end_group(pgid, process.new_nonce())
        assert process.is_alive(member), "a group the engine cannot confirm as the run's own was signalled"
    finally:
        _cleanup(member)


def test_EndGroup_WhenTheProcessTableCannotBeRead_SignalsNoGroup(monkeypatch):
    nonce = process.new_nonce()
    pgid, member = _orphaned_group({process.NONCE_VARIABLE: nonce})
    monkeypatch.setattr(process.programs, "path", lambda name, *args: "/usr/bin/false")
    try:
        assert process.end_group(pgid, nonce) is None
        assert process.is_alive(member), "the group was signalled blind"
    finally:
        _cleanup(member)


def test_EndGroup_AConfirmedGroup_EndsAMemberThatClearedItsEnvironmentToo(tmp_path):
    pid_file = tmp_path / "cleared.pid"
    outcome = _run(tmp_path, ["sh", "-c", f"(env -i /bin/sleep 300 & echo $! > {pid_file}); sleep 300 & exit 0"])
    assert outcome.left_running
    assert _gone(int(pid_file.read_text()))


# -- recovery: a dead engine's successor ends what is left of a run ---------------------------------------------------

HEX_NONCE = re.compile(r"[0-9a-f]{32}")
ORPHAN = ("import subprocess, sys\n"
          "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(300)'])\n"
          "print(child.pid, flush=True)\n")


def _python_orphan(nonce):
    """A run's group whose leader already exited, with a Python child (whose environment `ps -E` reads) left in it:
    (leader pid, child pid)."""
    leader = subprocess.Popen([sys.executable, "-c", ORPHAN], stdout=subprocess.PIPE, text=True,
                              stdin=subprocess.DEVNULL, start_new_session=True,
                              env={"PATH": os.environ["PATH"], process.NONCE_VARIABLE: nonce})
    child = int(leader.stdout.readline())
    leader.wait()
    return leader.pid, child


def test_ExternalTrackedRun_RecordsItsProcessGroupAndNonceWhileItRuns(repo, tracked_child):
    ctx = make_context(repo)
    seen = []

    def run():
        return process.external_tracked_run(
            ctx.store, [sys.executable, "-c", "pass"], cwd=repo, env={},
            log_path=ctx.store.operations_dir / "external.log", budget_seconds=30, idle_seconds=30, stop_file=None,
            on_poll=lambda: seen.extend(ctx.store.external_runs()),
        )

    result = while_registered(tracked_child, run, lambda: seen.extend(ctx.store.external_runs()))
    assert result.exit_code == 0 and len(seen) == 2 and seen[0] == seen[1]
    assert seen[0]["pgid"] == seen[0]["pid"] == result.pid and HEX_NONCE.fullmatch(seen[0]["nonce"])
    assert tracked_child.nonce == seen[0]["nonce"]
    assert ctx.store.external_runs() == []


def test_ExternalRunRegistry_RefusesAMalformedGroupOrNonce(repo):
    ctx = make_context(repo)
    with pytest.raises(ValueError):
        ctx.store.register_external_run(4242, "cmd", 4242, "not a nonce")
    with pytest.raises(ValueError):
        ctx.store.register_external_run(4242, "cmd", 0, process.new_nonce())


def test_StaleExternalRun_ItsLeaderGoneAndANonceChildLeft_IsEndedOnResume(repo):
    from sakura.flow import engine

    ctx = make_context(repo)
    nonce = process.new_nonce()
    leader, child = _python_orphan(nonce)
    try:
        ctx.store.register_external_run(leader, "the run's leader, gone", leader, nonce)
        engine.kill_stale_run(ctx)
        assert _gone(child)
        assert ctx.store.external_runs() == []
    finally:
        _cleanup(child)


def test_LocalLauncherKill_ALeaderAlreadyGone_StillEndsTheChildCarryingTheNonce():
    from sakura.flow import launchers

    nonce = process.new_nonce()
    leader, child = _python_orphan(nonce)
    try:
        launchers.LocalLauncher().kill({"pid": leader, "command": "the run's leader, gone", "pgid": leader,
                                        "nonce": nonce})
        assert _gone(child)
    finally:
        _cleanup(child)


def test_StaleRun_ItsLeaderGoneAndANonceChildLeft_IsEndedOnResume(repo):
    from sakura.flow import engine

    nonce = process.new_nonce()
    leader, child = _python_orphan(nonce)
    entry = {"pid": leader, "command": "the run's leader, gone", "pgid": leader, "nonce": nonce, "run": "build-1",
             "host": "local"}
    ctx = make_context(repo, state={"active_runs": [entry], "active_run": entry})
    try:
        engine.kill_stale_run(ctx)
        assert _gone(child)
        assert ctx.store.read_state()["active_runs"] == []
    finally:
        _cleanup(child)
