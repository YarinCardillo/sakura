"""App stack lifecycle: start, readiness, stop by exact PID."""

import select
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import process, runner, stack, stacks
from sakura.flow.workspaces import Workspace

from flow_support import make_context, task_workspace

SERVER = "import http.server,sys; http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), http.server.SimpleHTTPRequestHandler).serve_forever()"
# An app that listens on its port, as every app.start must (review 15): no ready URL needed.
LISTENING = f"exec {sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"


def _workspace(tmp_path, start_port=45500):
    return Workspace("0", tmp_path, "b", range(start_port, start_port + 10))


def _context(repo, **app):
    return make_context(repo, own_repository=True, overrides={"app": {"ready_seconds": 10, **app}})


def _port_open(port):
    import socket

    with socket.socket() as probe:
        return probe.connect_ex(("127.0.0.1", port)) == 0


def test_stack_waits_for_ready_and_stops_by_pid(repo, tmp_path, monkeypatch, stack_process, stack_clock):
    ctx = _context(repo, start=LISTENING, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    replies = iter((False, True))
    monkeypatch.setattr(stack.ReadyProbe, "answers", lambda _probe: next(replies))
    with stack.start(ctx, _workspace(tmp_path)) as app:
        port = app.port
        assert stack_clock.pauses == [stack.READY_POLL_SECONDS]
        assert app.base_url == f"http://127.0.0.1:{port}"
        assert stack_process.listeners(port) == [app.pid]
        assert process.is_alive(app.pid)
        assert "demo:0" in ctx.store.read_state()["stacks"]
    assert not process.is_alive(app.pid)
    assert ctx.store.read_state()["stacks"] == {}
    assert stack_process.listeners(port) == []


def test_stack_exports_configured_origin_and_host(repo, tmp_path):
    ctx = make_context(repo, own_repository=True, overrides={"builders": {"max": 1}, "app": {
        "ready_seconds": 10, "start": f"{sys.executable} -m http.server $SAKURA_FLOW_PORT --bind 127.0.0.1",
        "ready_url": "http://localhost:$SAKURA_FLOW_PORT/", "origin": "http://localhost:45500"}})
    with stack.start(ctx, _workspace(tmp_path)) as app:
        assert app.base_url == "http://localhost:45500"
        assert app._env["SAKURA_FLOW_HOST"] == "localhost"
        assert app._env["SAKURA_FLOW_BASE_URL"] == "http://localhost:45500"


def test_idle_supervisor_does_not_refresh_its_stack(repo, tmp_path, monkeypatch, stack_process, stack_clock):
    ctx = _context(repo, start=LISTENING, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    ctx.config.values["load"]["idle_stack_seconds"] = 1
    monkeypatch.setattr(stacks, "time", SimpleNamespace(time=lambda: stack_clock.value))
    app = stack.start(ctx, _workspace(tmp_path))
    try:
        assert ctx.store.read_state()["stacks"]["demo:0"]["last_used"] == 100
        stack_clock.value += 2
        stack_process.may_poll = True
        stack_process.tick.set()
        assert stack_process.poll_returned.wait(10)
        assert not process.is_alive(app.pid), "an idle app supervisor must let its stack expire"
        assert "demo:0" not in ctx.store.read_state()["stacks"]
        assert "stack demo:0 stopped after" in ctx.store.ledger.read_text()
    finally:
        app.stop()


def test_stack_start_failure_reports_log_tail(repo, tmp_path):
    ctx = _context(repo, start="echo cannot bind the socket; exit 7", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    with pytest.raises(stack.StackError) as raised:
        stack.start(ctx, _workspace(tmp_path))
    assert "cannot bind the socket" in str(raised.value)
    assert "exit 7" in str(raised.value)
    assert str(ctx.store.runs_dir / "operations" / "app-0.log") in str(raised.value)


def test_stack_times_out_when_never_ready(repo, tmp_path):
    ctx = _context(repo, start="sleep 30", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/", ready_seconds=1)
    with pytest.raises(stack.StackError, match="not ready within 1"):
        stack.start(ctx, _workspace(tmp_path))


def test_stack_runs_app_stop_command(repo, tmp_path):
    marker = tmp_path / "stopped"
    start = f"{sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"
    ctx = _context(repo, start=start, stop=f"touch {marker}", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    app = stack.start(ctx, _workspace(tmp_path))
    app.stop()
    assert marker.exists()
    assert (ctx.store.runs_dir / "operations" / "app-0.stop.log").exists()


def test_stack_process_is_recorded_until_stopped(repo, tmp_path):
    ctx = _context(repo, start=LISTENING)
    app = stack.start(ctx, _workspace(tmp_path))
    recorded = ctx.store.read_state()["active_stacks"]

    assert recorded[0]["pid"] == app.pid
    assert recorded[0]["command"]

    app.stop()

    assert ctx.store.read_state()["active_stacks"] == []


def test_stack_start_exports_its_id_for_heavy_commands(repo, tmp_path):
    marker = tmp_path / "stack-id"
    ctx = _context(repo, start=f"printf '%s' \"$SAKURA_FLOW_STACK\" > {marker}; {LISTENING}")

    app = stack.start(ctx, _workspace(tmp_path))
    try:
        assert marker.read_text() == "demo:0"
    finally:
        app.stop()


def test_app_and_agent_logs_have_distinct_top_level_runs_namespace(repo, tmp_path):
    import textwrap

    agent_script = tmp_path / "agent.py"
    agent_script.write_text(textwrap.dedent("""
        import sys
        sys.stdin.read()
        print('SAKURA_RESULT {"verdict": "APPROVE"}')
    """))
    ctx = make_context(repo, own_repository=True, overrides={
        "app": {"ready_seconds": 10,
                "start": f"{sys.executable} -m http.server $SAKURA_FLOW_PORT --bind 127.0.0.1",
                "ready_url": "http://127.0.0.1:$SAKURA_FLOW_PORT/"},
        "roles": {"planner": f"exec:{sys.executable} {agent_script}"},
    })
    workspace = _workspace(tmp_path)
    app = stack.start(ctx, workspace)

    try:
        result = runner.AgentRunner(ctx.store, ctx.config).run(
            runner.RunRequest("planner", "intake", "hello", {}, task_workspace(ctx))
        )
        assert result.result == {"verdict": "APPROVE"}
        assert ctx.store.read_state()["stacks"]["demo:0"]["log"] == str(
            ctx.store.runs_dir / "operations" / "app-0.log"
        )
        assert [path.name for path in ctx.store.runs_dir.glob("*.log")] == ["intake-1.log"]
    finally:
        app.stop()


def test_stack_identity_mismatch_keeps_recovery_record(repo, tmp_path, monkeypatch):
    ctx = _context(repo, start=LISTENING)
    app = stack.start(ctx, _workspace(tmp_path))
    original_alive, original_kill = process.is_alive, process.kill_tree
    monkeypatch.setattr(process, "is_alive", lambda _pid: True)
    monkeypatch.setattr(process, "kill_tree", lambda _pid, _command: False)

    with pytest.raises(stack.StackError, match="no longer matches"):
        app.stop()

    state = ctx.store.read_state()
    assert state["active_stacks"][0]["pid"] == app.pid
    assert "demo:0" in state["stacks"]
    monkeypatch.setattr(process, "is_alive", original_alive)
    monkeypatch.setattr(process, "kill_tree", original_kill)
    app.stop()
    assert ctx.store.read_state()["active_stacks"] == []


def test_stack_supervisor_crash_surfaces_as_stack_error(repo, tmp_path, stack_process, stack_clock):
    stack_process.crash = OSError("cannot spawn")
    ctx = _context(repo, start="anything", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    with pytest.raises(stack.StackError, match="cannot spawn"):
        stack.start(ctx, _workspace(tmp_path))
    assert stack_clock.pauses == []


def _foreign_server(port=0):
    """Start a listener in another group and wait for acknowledgment from that process."""
    import subprocess

    script = (
        "import http.server, sys; "
        "server = http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), http.server.SimpleHTTPRequestHandler); "
        "print(server.server_port, flush=True); server.serve_forever()"
    )
    server = subprocess.Popen([sys.executable, "-c", script, str(port)], start_new_session=True,
                              stdout=subprocess.PIPE, text=True)
    try:
        assert select.select([server.stdout], [], [], 10)[0], "listener never bound"
        address = int(server.stdout.readline().strip())
    except BaseException:
        server.kill()
        server.wait()
        raise
    finally:
        server.stdout.close()
    return server, address


def test_stack_warns_when_daemonized_start_has_no_stop_command(repo, tmp_path):
    # The daemon stays in the app's own process group, so its listener is the app's.
    daemon = f"(sleep 1; {sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT) > /dev/null 2>&1 & exit 0"
    ctx = _context(repo, start=daemon, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    app = stack.start(ctx, _workspace(tmp_path))
    try:
        assert _port_open(app.port)
    finally:
        app.stop()
    assert "app may still be running" in ctx.store.ledger.read_text()


def test_Stack_ALeftoverListenerOnTheWorkspacePort_IsNeverWhatTheAppAnswersFrom(repo, tmp_path):
    # Review 14, finding 2: each session's app gets a fresh port no earlier session's cage was granted.
    survivor, workspace_port = _foreign_server()
    workspace = _workspace(tmp_path, workspace_port)
    try:
        start = f"{sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"
        ctx = _context(repo, start=start, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
        with stack.start(ctx, workspace) as app:
            assert app.port != workspace.ports[0] and app.base_url == f"http://127.0.0.1:{app.port}"
            assert ctx.store.read_state()["stacks"]["demo:0"]["port"] == app.port
        assert survivor.poll() is None, "the engine must not touch a process it did not start"
    finally:
        survivor.kill()
        survivor.wait()


def test_Stack_TwoSessionsInARow_NeverShareAPortWhileTheFirstOneStillListens(repo, tmp_path):
    start = f"{sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"
    ctx = _context(repo, start=start, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    workspace = _workspace(tmp_path)
    with stack.start(ctx, workspace) as first:
        pass
    survivor, _ = _foreign_server(first.port)
    try:
        with stack.start(ctx, workspace) as second:
            assert second.port != first.port
    finally:
        survivor.kill()
        survivor.wait()


def test_Stack_AnOriginPortAnotherProcessListensOn_IsRefusedBeforeTheAppStarts(repo, tmp_path):
    survivor, port = _foreign_server()
    marker = tmp_path / "started"
    try:
        ctx = make_context(repo, own_repository=True, overrides={"builders": {"max": 1}, "app": {
            "ready_seconds": 10, "start": f"touch {marker}; sleep 30", "origin": f"http://127.0.0.1:{port}",
            "ready_url": "http://127.0.0.1:$SAKURA_FLOW_PORT/"}})
        with pytest.raises(stack.StackError, match=f"port {port} is in use"):
            stack.start(ctx, _workspace(tmp_path, port))
        assert not marker.exists()
    finally:
        survivor.kill()
        survivor.wait()


def test_Stack_AStartThatExitsWhileAnotherProcessAnswers_IsRefused(repo, tmp_path, monkeypatch):
    # A listener that is not the app's (here one that took the port between the check and the start) is never
    # what a proof runs against, whatever it answers.
    survivor, port = _foreign_server()
    monkeypatch.setattr(stack.ports, "allocate", lambda *_args, **_kwargs: range(port, port + 1))
    try:
        ctx = _context(repo, start="sleep 0.2", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
        with pytest.raises(stack.StackError, match="not this app's"):
            stack.start(ctx, _workspace(tmp_path))
    finally:
        survivor.kill()
        survivor.wait()


def test_stack_error_after_supervisor_crash_stops_the_running_app(repo, tmp_path, monkeypatch):
    import subprocess

    spawned = []

    def start_then_crash(argv, *, on_start, **_kwargs):
        child = subprocess.Popen(["sleep", "30"])
        spawned.append(child)
        on_start(child.pid)
        raise OSError("monitor crashed")

    monkeypatch.setattr(process, "monitored_run", start_then_crash)
    ctx = _context(repo, start="anything", ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    try:
        with pytest.raises(stack.StackError, match="monitor crashed"):
            stack.start(ctx, _workspace(tmp_path))
        assert not process.is_alive(spawned[0].pid)
    finally:
        spawned[0].kill()
        spawned[0].wait()


# -- review 4: the readiness probe asks only the app's own origin, from the cage ----------------------------------


@pytest.mark.parametrize("ready_url", [
    "http://169.254.169.254/latest/meta-data/", "http://127.0.0.1:1/", "http://localhost:$SAKURA_FLOW_PORT/",
    "http://user@127.0.0.1:$SAKURA_FLOW_PORT/", "https://127.0.0.1:$SAKURA_FLOW_PORT/", "http://192.0.2.1:$SAKURA_FLOW_PORT/"])
def test_ReadyUrl_AnotherHostOrPort_IsRefusedBeforeTheAppStarts(repo, tmp_path, ready_url):
    marker = tmp_path / "started"
    ctx = _context(repo, start=f"touch {marker}", ready_url=ready_url)
    with pytest.raises(stack.StackError, match="app.ready_url"):
        stack.start(ctx, _workspace(tmp_path))
    assert not marker.exists()


def test_ReadyProbe_RunsInTheCageAndReachesOnlyTheAppPort(repo, tmp_path, monkeypatch):
    from sakura.flow import cage

    port = 45500
    start = f"{sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"
    ctx = _context(repo, start=start, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    probes = []
    real_run = stack.subprocess.run

    def recorded(argv, *args, **kwargs):
        if stack.PROBE_SCRIPT in argv:
            probes.append(list(argv))
        return real_run(argv, *args, **kwargs)

    monkeypatch.setattr(stack.subprocess, "run", recorded)
    with stack.start(ctx, _workspace(tmp_path, port)) as app:
        app_port = app.port
    assert probes and all(argv[0] == cage.SEATBELT for argv in probes)
    profile = Path(probes[0][2]).read_text()
    assert f'(remote ip "localhost:{app_port}")' in profile and profile.count("network-outbound (remote ip") == 1


# -- review 15 ------------------------------------------------------------------------------------------------------


def test_Stack_TwoSessionsOfOneTask_NeverGetTheSamePort(repo, tmp_path):
    # Review 15, finding 2: a port granted in a task is never handed out again in that task.
    start = f"{sys.executable} -c \"{SERVER}\" $SAKURA_FLOW_PORT"
    ctx = _context(repo, start=start, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    workspace = _workspace(tmp_path)
    with stack.start(ctx, workspace) as first:
        pass
    with stack.start(ctx, workspace) as second:
        assert second.port != first.port


def test_Stack_AListenerOutsideTheGroupCarryingTheNonce_IsRefused(repo, tmp_path, monkeypatch, stack_process):
    foreign_pid = 742
    port = 45510
    stack_process.foreign.append(foreign_pid)
    monkeypatch.setattr(stack.ports, "allocate", lambda *_args, **_kwargs: range(port, port + 1))
    monkeypatch.setattr(process, "new_nonce", lambda: "shared-nonce")
    monkeypatch.setattr(process, "_census", lambda *_args: process._Census((foreign_pid,), (stack_process.pid,), True))
    ctx = _context(repo, start=LISTENING, ready_url="http://127.0.0.1:$SAKURA_FLOW_PORT/")
    with pytest.raises(stack.StackError, match=f"process {foreign_pid}, not this app's"):
        stack.start(ctx, _workspace(tmp_path))
    assert stack_process.listeners(port) == [foreign_pid]


def test_Stack_AStartThatNeverListens_FailsToStartWithoutAReadyUrl(repo, tmp_path):
    # Review 15, finding 4: with app.start set, an owned listener is required whatever the ready URL.
    ctx = _context(repo, start="sleep 30", ready_seconds=1)
    with pytest.raises(stack.StackError, match="no listener of its own"):
        stack.start(ctx, _workspace(tmp_path))


def test_Rows_AnEndpointNotOwnedByTheAppsGroup_FailTheRowWithNoReceipt(repo, tmp_path):
    # Review 15, finding 2: ownership is checked again around every proof row.
    import subprocess

    from sakura.flow import baseline, contract, stacks

    survivor, port = _foreign_server()
    holder = subprocess.Popen(["sleep", "30"], start_new_session=True)
    try:
        ctx = _context(repo)
        row = contract.Row("R1", "j", "s", "api", None, "true", False, False)
        ctx.store.update_state(lambda state: state["approvals"].update(checks={"commands": ["true"]}))
        stacks.register(ctx, "demo:0", holder.pid, "sleep 30", tmp_path, tmp_path / "app.log", port, "n")
        with pytest.raises(baseline.FlowInterrupted, match="not the app's"):
            baseline.run_rows(ctx, repo, f"http://127.0.0.1:{port}", port, [row])
        assert "receipts" not in ctx.store.read_state()
    finally:
        for process_ in (survivor, holder):
            process_.kill()
            process_.wait()
