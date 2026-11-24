import subprocess
import sys
import time

import pytest

from sakura.flow import engine, launchers, process, runner
from flow_support import RemoteStyleLauncher, make_context, task_workspace

SLEEPER = ["sleep", "60"]


def _request(tmp_path, argv):
    return launchers.LaunchRequest(argv=argv, cwd=tmp_path, env={"PATH": "/usr/bin:/bin"}, log_path=tmp_path / "run.log",
                                   budget_seconds=30, idle_seconds=30, stop_file=None, stdin_text="", nice=None)


def test_local_launcher_identity_and_liveness(tmp_path):
    launcher = launchers.LocalLauncher()
    seen = []
    outcome = launcher.run(_request(tmp_path, [sys.executable, "-c", "import time; time.sleep(1)"]), seen.append)
    assert launcher.host == "local" and outcome.exit_code == 0
    assert set(seen[0]) == {"pid", "command", "pgid", "nonce"} and seen[0]["pid"] == seen[0]["pgid"] == outcome.pid
    assert "time.sleep(1)" in seen[0]["command"]
    assert not launcher.is_alive(seen[0])
    proc = subprocess.Popen(SLEEPER)
    try:
        identity = {"pid": proc.pid, "command": process.command_of(proc.pid)}
        assert launcher.is_alive(identity)
        assert not launcher.is_alive({"pid": proc.pid, "command": "some other command"})
        launcher.kill(identity)
        proc.wait(timeout=10)
        assert not launcher.is_alive(identity)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def test_local_launcher_treats_a_command_that_differs_only_by_a_suffix_as_another_process():
    launcher = launchers.LocalLauncher()
    proc = subprocess.Popen(SLEEPER)
    try:
        recorded = process.command_of(proc.pid)[:-1]
        identity = {"pid": proc.pid, "command": recorded}
        assert not launcher.is_alive(identity)
        launcher.kill(identity)
        assert proc.poll() is None
    finally:
        proc.kill()
        proc.wait()


def test_local_launcher_kill_refuses_a_reused_pid():
    proc = subprocess.Popen(SLEEPER)
    try:
        launchers.LocalLauncher().kill({"pid": proc.pid, "command": "some other command"})
        time.sleep(0.3)
        assert proc.poll() is None
    finally:
        proc.kill()
        proc.wait()


def test_for_host_knows_local_only():
    assert isinstance(launchers.for_host("local"), launchers.LocalLauncher)
    assert launchers.for_host("far") is None


def test_runner_never_touches_pids_with_fake_remote_launcher(repo, monkeypatch):
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} -c pass"}})
    seen = []
    remote = RemoteStyleLauncher(on_run=lambda _request: seen.append(ctx.store.read_state()["active_runs"]))

    def refuse(_pid):
        raise AssertionError("the runner must not look at PIDs")

    monkeypatch.setattr(process, "command_of", refuse)
    request = runner.RunRequest("planner", "intake", "hello", {}, task_workspace(ctx), {})
    result = runner.AgentRunner(ctx.store, ctx.config, launcher=remote).run(request)

    assert result.exit_code == 0 and result.final == "remote output\n"
    assert seen == [[{"run": "intake-1", "host": "far", "token": "x"}]]
    state = ctx.store.read_state()
    assert state["active_runs"] == [] and state["active_run"] is None
    assert state["runs"]["intake-1"]["host"] == "far" and state["runs"]["intake-1"]["identity"] == {"host": "far", "token": "x"}


def test_stop_routes_to_host_launcher_and_reports_unknown_host(repo, monkeypatch):
    ctx = make_context(repo)
    far = {"run": "build-1", "host": "far", "token": "x"}
    ghost = {"run": "build-2", "host": "ghost", "token": "y"}
    ctx.store.update_state(lambda state: state.update(active_runs=[far, ghost], active_run=ghost))
    remote = RemoteStyleLauncher()
    monkeypatch.setattr(launchers, "for_host", lambda host: remote if host == "far" else None)

    engine.kill_stale_run(ctx)

    assert remote.killed == [far]
    state = ctx.store.read_state()
    assert state["active_runs"] == [ghost] and state["active_run"] == ghost
    ledger = ctx.store.ledger.read_text()
    assert "killed stale run build-1" in ledger
    assert "run build-2 on ghost has no launcher here" in ledger


def test_stop_skips_a_run_the_launcher_says_is_gone(repo, monkeypatch):
    ctx = make_context(repo)
    far = {"run": "build-1", "host": "far", "token": "x"}
    ctx.store.update_state(lambda state: state.update(active_runs=[far], active_run=far))
    remote = RemoteStyleLauncher()
    remote.alive = False
    monkeypatch.setattr(launchers, "for_host", lambda host: remote)

    engine.kill_stale_run(ctx)

    assert remote.killed == [] and ctx.store.read_state()["active_runs"] == []


def test_stop_marks_the_run_records_of_killed_and_dead_runs_orphaned(repo, monkeypatch):
    ctx = make_context(repo)
    ctx.store.update_state(lambda state: state.update(runs={"build-1": {
        "name": "build-1", "state": "running", "host": "far", "identity": {"host": "far", "token": "x"}}}))
    remote = RemoteStyleLauncher()
    remote.alive = False
    monkeypatch.setattr(launchers, "for_host", lambda host: remote)

    engine.kill_stale_run(ctx)

    assert ctx.store.read_state()["runs"]["build-1"]["state"] == "orphaned"
