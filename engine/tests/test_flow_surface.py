"""The tmux surface of a task: one session per task, one live window per run, never a failed run."""

from __future__ import annotations

import dataclasses
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from flow_support import FakeRunner, MonitorLauncher, make_context, task_workspace
from tmux_support import TMUX_PROGRAM, needs_tmux, private_tmux, wait_until

from sakura.flow import config, engine, process, runner, store, surface
from sakura.flow.phases import cleanup
from sakura.flow.surface import NullSurface, TmuxSurface, build_surface, session_name
from sakura.flow.tmux import TmuxClient, marked_name


PREFIX = "flow-"
LIB = str(Path(__file__).resolve().parents[1] / "lib")
WARNING = "surface unavailable"


class RecordingSurface:
    """Surface double: records every call with the run state it could see."""

    def __init__(self):
        self.calls = []

    def task_opened(self, store):
        self.calls.append(("task_opened",))

    def run_started(self, store, run, role):
        self.calls.append(("run_started", run, role, store.read_state()["runs"][run]["state"]))

    def task_closed(self, store):
        self.calls.append(("task_closed",))

    def task_settled(self, store):
        self.calls.append(("task_settled", store.read_state()["status"]))


def surface_for(client, environ=None):
    return TmuxSurface(client, PREFIX, environ={} if environ is None else environ)


def session_of(ctx):
    return session_name(PREFIX, ctx.repo, ctx.store.task_id)


def window_names(client, session):
    return sorted(window.name for window in client.windows(session))


def ledger_text(ctx):
    return ctx.store.ledger.read_text() if ctx.store.ledger.exists() else ""


def failing_client(message="no server running"):
    def run(argv, **kwargs):
        return subprocess.CompletedProcess(argv, 1, "", message)

    return TmuxClient(socket="unreachable", run=run)


def start_command(client, window_id):
    done = subprocess.run([TMUX_PROGRAM, "-L", client.socket, "-f", "/dev/null", "display-message", "-p", "-t", window_id,
                           "#{pane_start_command}"], capture_output=True, text=True, check=True)
    return done.stdout.strip()


def test_session_name_is_marked_and_safe_for_hostile_names(tmp_path):
    repo = tmp_path / "re po;$(x)"
    name = session_name(PREFIX, repo, "t 1")
    assert name == marked_name(f"{PREFIX}{repo.name}-t 1", f"{repo}:t 1")
    assert all(character.isalnum() or character in "._-" for character in name)


def test_null_surface_when_disabled_or_absent(repo, tmp_path, monkeypatch):
    ctx = make_context(repo)
    enabled = config.load(repo, home=tmp_path / "nohome")
    disabled = config.load(repo, home=tmp_path / "nohome", overrides={"surface": {"tmux": False}})
    assert enabled.values["surface"] == {"tmux": True, "session_prefix": "flow-", "cockpit_session": "flow"}
    assert isinstance(build_surface(disabled), NullSurface)

    empty = tmp_path / "empty-path"
    empty.mkdir()
    monkeypatch.setenv("PATH", str(empty))
    assert isinstance(build_surface(enabled), NullSurface)
    for call in (lambda: NullSurface().task_opened(ctx.store), lambda: NullSurface().run_started(ctx.store, "a", "b"),
                 lambda: NullSurface().task_closed(ctx.store)):
        assert call() is None


@needs_tmux
def test_build_surface_uses_tmux_when_enabled_and_present(repo, tmp_path):
    enabled = config.load(repo, home=tmp_path / "nohome")
    built = build_surface(enabled, client=TmuxClient(socket="sakura-test-never-started"))
    assert isinstance(built, TmuxSurface)


@needs_tmux
def test_run_window_runs_watch(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        surface_for(client).run_started(ctx.store, "build-1", "builder")
        session = session_of(ctx)
        windows = {window.name: window for window in client.windows(session)}
        assert sorted(windows) == ["build-1-builder", "events"]
        run_window = windows["build-1-builder"]
        assert run_window.options["@sakura-run"] == "build-1"
        command = start_command(client, run_window.id)
        assert all(part in command for part in ("bin/sakura", "flow", "watch", "demo", "build-1"))
        assert all(part in start_command(client, windows["events"].id) for part in ("flow", "events", "demo"))
        assert client.option(session, "@sakura-task") == f"{repo}:demo"
    assert WARNING not in ledger_text(ctx)


@needs_tmux
def test_run_window_is_not_duplicated_for_the_same_run(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        for _ in range(2):
            surface_for(client).run_started(ctx.store, "build-1", "builder")
        assert window_names(client, session_of(ctx)) == ["build-1-builder", "events"]


@needs_tmux
def test_server_killed_between_runs_is_rebuilt_same_driver(repo):
    ctx = make_context(repo)
    driver = None
    with private_tmux() as client:
        driver = surface_for(client)
        driver.run_started(ctx.store, "plan-1", "planner")
        subprocess.run([TMUX_PROGRAM, "-L", client.socket, "kill-server"], check=True, capture_output=True)
        assert not client.has_session(session_of(ctx))

        driver.run_started(ctx.store, "build-1", "builder")

        assert window_names(client, session_of(ctx)) == ["build-1-builder", "events"]
    assert WARNING not in ledger_text(ctx)
    assert not ctx.store.read_state().get("surface_warned")


def test_warning_ledgered_once_across_restarts(repo):
    ctx = make_context(repo)
    for _ in range(2):
        restarted = surface_for(failing_client())
        restarted.task_opened(ctx.store)
        restarted.run_started(ctx.store, "build-1", "builder")
        restarted.task_closed(ctx.store)
    assert ledger_text(ctx).count(WARNING) == 1
    assert ctx.store.read_state()["surface_warned"] is True


def test_warning_text_is_one_line_without_terminal_controls(repo):
    ctx = make_context(repo)
    surface_for(failing_client("boom\x1b]2;owned\x07 \x1b[2Jfailed\nsecond line")).task_opened(ctx.store)
    warning = [line for line in ledger_text(ctx).splitlines() if WARNING in line]
    assert len(warning) == 1
    assert "\x1b" not in warning[0] and "\x07" not in warning[0]
    assert "failed" in warning[0]
    assert ledger_text(ctx).count("\n") == len(ledger_text(ctx).splitlines())


@pytest.mark.parametrize("broken", ["update_state", "append_ledger"])
@pytest.mark.parametrize("error", [store.StateTampered(Path("state.json")), OSError("disk full")])
def test_a_failing_warning_write_never_fails_the_run(repo, monkeypatch, broken, error):
    ctx = make_context(repo)

    def fail(*args, **kwargs):
        raise error

    monkeypatch.setattr(store.TaskStore, broken, fail)
    surface_for(failing_client()).task_opened(ctx.store)


def test_a_socket_path_with_a_comma_is_still_recognised_as_the_own_server():
    class StubClient:
        @staticmethod
        def socket_path():
            return "/tmp/odd,dir/default"

    driver = surface_for(StubClient())
    assert driver._is_own_server("/tmp/odd,dir/default,4242,0")
    assert not driver._is_own_server("/tmp/odd,dir/other,4242,0")


@needs_tmux
def test_foreign_session_with_the_task_name_is_never_used_or_killed(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        foreign = ("@sakura-task", "someone-else")
        client.ensure_session(session_of(ctx), repo, "x", ["sleep", "600"], {}, foreign)
        driver = surface_for(client)

        driver.run_started(ctx.store, "build-1", "builder")
        driver.task_closed(ctx.store)

        assert window_names(client, session_of(ctx)) == ["x"]
        assert client.has_session(session_of(ctx))
    assert ledger_text(ctx).count(WARNING) == 1


@needs_tmux
def test_cleanup_outside_kills_marked_session_only(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        driver = surface_for(client)
        driver.run_started(ctx.store, "build-1", "builder")
        client.ensure_session("bystander", repo, "work", ["sleep", "600"], {}, ("@sakura-task", "other:task"))

        driver.task_closed(ctx.store)

        assert not client.has_session(session_of(ctx))
        assert client.has_session("bystander")


@needs_tmux
def test_a_pane_id_from_another_server_does_not_count_as_inside(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        driver_outside = surface_for(client)
        driver_outside.task_opened(ctx.store)
        pane = client.windows(session_of(ctx))[0].panes[0].id
        foreign_environment = {"TMUX": "/tmp/other-server,1,0", "TMUX_PANE": pane}

        surface_for(client, foreign_environment).task_closed(ctx.store)

        assert not client.has_session(session_of(ctx))


ENGINE_WINDOW = textwrap.dedent("""
    import dataclasses, sys
    from pathlib import Path
    from sakura.flow import engine, phases, store
    from sakura.flow.phases import cleanup
    from sakura.flow.surface import TmuxSurface
    from sakura.flow.tmux import TmuxClient

    repo, task, socket, exit_file = sys.argv[1:5]
    cleanup._pr_state = lambda url: "MERGED"
    ctx = engine.open_context(Path(repo), store.TaskStore(Path(repo), task))
    ctx = dataclasses.replace(ctx, surface=TmuxSurface(TmuxClient(socket=socket), "flow-"))
    code = engine.advance(ctx)
    Path(exit_file).write_text(str(code))
    sys.exit(code)
""")


@needs_tmux
def test_cleanup_from_engine_window_finishes_then_session_ends(repo, tmp_path):
    ctx = make_context(repo, state={"phase": "cleanup", "pr": {"url": ""}})
    exit_file = tmp_path / "exit-code"
    environment = {"HOME": os.environ["HOME"], "PATH": os.environ["PATH"], "PYTHONPATH": LIB}
    with private_tmux() as client:
        surface_for(client).task_opened(ctx.store)
        session = session_of(ctx)
        argv = [sys.executable, "-c", ENGINE_WINDOW, str(repo), "demo", client.socket, str(exit_file)]
        client.new_window(session, "engine", argv, repo, environment)

        assert wait_until(exit_file.exists, 30), "the detached driver reached its end"
        assert exit_file.read_text() == "0"
        assert store.TaskStore(repo, "demo").read_state()["status"] == "cleaned"
        assert wait_until(lambda: not client.has_session(session), 15), "the session ends with the driver"


def test_advance_opens_the_surface_before_any_phase(repo, monkeypatch):
    recording = RecordingSurface()
    ctx = dataclasses.replace(make_context(repo, runner=FakeRunner()), surface=recording)
    ctx.store.update_state(lambda state: state.update(phase="cleanup", status="cleaned"))
    assert engine.advance(ctx) == 0
    assert recording.calls == []

    ctx.store.update_state(lambda state: state.update(phase="intake", status="running"))
    monkeypatch.setitem(engine._HANDLERS, "intake", lambda context: engine.PhaseOutcome("blocked", "stop here"))
    monkeypatch.delitem(engine._INPUTS, "intake", raising=False)
    engine.advance(ctx)
    assert recording.calls == [("task_opened",)]


def test_cleanup_closes_the_surface_only_when_cleaned(repo, monkeypatch):
    recording = RecordingSurface()
    ctx = dataclasses.replace(make_context(repo, state={"phase": "cleanup", "pr": {"url": ""}}), surface=recording)
    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "OPEN")
    assert cleanup.run(ctx).kind == "blocked"
    assert recording.calls == []

    monkeypatch.setattr(cleanup, "_pr_state", lambda _url: "MERGED")
    assert cleanup.run(ctx).kind == "next"
    assert recording.calls == [("task_closed",)]


def test_runner_announces_a_run_after_its_record_begins(repo, tmp_path):
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} -c pass"}})
    recording = RecordingSurface()
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))
    request = runner.RunRequest("planner", "intake", "hello", {}, task_workspace(ctx))

    runner.AgentRunner(ctx.store, ctx.config, launcher=launcher, surface=recording).run(request)

    assert recording.calls == [("run_started", "intake-1", "planner", "starting")]


def test_runner_without_a_surface_launches_as_before(repo):
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} -c pass"}})
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))
    request = runner.RunRequest("planner", "intake", "hello", {}, task_workspace(ctx))
    assert runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(request).exit_code == 0


def test_open_context_builds_the_surface_from_the_config(repo, monkeypatch):
    ctx = make_context(repo)
    built = []
    monkeypatch.setattr(surface, "build_surface", lambda cfg: built.append(cfg) or RecordingSurface())
    opened = engine.open_context(repo, ctx.store)
    assert isinstance(opened.surface, RecordingSurface) and built == [opened.config]


def test_the_surface_built_without_a_client_talks_to_the_private_sakura_socket(repo, tmp_path, monkeypatch):
    monkeypatch.setattr(TmuxClient, "available", lambda self: True)
    built = build_surface(config.load(repo, home=tmp_path / "nohome"))
    assert isinstance(built, TmuxSurface) and built._client.socket == "sakura"


@pytest.mark.parametrize("kind, status", [("report", "report_ready"), ("failed", "failed")])
def test_advance_settles_the_surface_once_the_task_reaches_a_final_state(repo, monkeypatch, kind, status):
    recording = RecordingSurface()
    ctx = dataclasses.replace(make_context(repo, runner=FakeRunner()), surface=recording)
    ctx.store.update_state(lambda state: state.update(phase="report", status="running"))
    def handler(context):
        if kind == "report":
            context.store.update_state(lambda state: state.update(status="report_ready"))
            return engine.PhaseOutcome("next")
        return engine.PhaseOutcome("failed", "broken")

    monkeypatch.setitem(engine._HANDLERS, "report", handler)
    monkeypatch.delitem(engine._INPUTS, "report", raising=False)
    engine.advance(ctx)
    assert recording.calls == [("task_opened",), ("task_settled", status)]


@pytest.mark.parametrize("outcome", [engine.PhaseOutcome("blocked", "x"), engine.PhaseOutcome("await", gate="spec"),
                                     engine.PhaseOutcome("stopped", "x")])
def test_advance_keeps_the_surface_while_the_person_still_has_a_step(repo, monkeypatch, outcome):
    recording = RecordingSurface()
    ctx = dataclasses.replace(make_context(repo, runner=FakeRunner()), surface=recording)
    ctx.store.update_state(lambda state: state.update(phase="report", status="running"))
    monkeypatch.setitem(engine._HANDLERS, "report", lambda context: outcome)
    monkeypatch.delitem(engine._INPUTS, "report", raising=False)
    engine.advance(ctx)
    assert recording.calls == [("task_opened",)]


@needs_tmux
def test_settled_task_closes_only_its_own_ledger_window_on_its_own_server(repo):
    ctx = make_context(repo)
    with private_tmux() as client:
        driver = surface_for(client)
        driver.run_started(ctx.store, "build-1", "builder")
        session = session_of(ctx)
        client.new_window(session, "engine", ["sleep", "600"], repo, {})
        client.ensure_session("bystander", repo, "events", ["sleep", "600"], {}, ("@sakura-task", "other:task"))

        driver.task_settled(ctx.store)

        assert window_names(client, session) == ["build-1-builder", "engine"]
        assert window_names(client, "bystander") == ["events"]
