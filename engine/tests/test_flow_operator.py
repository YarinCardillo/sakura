import os
import subprocess
import sys
from types import SimpleNamespace

import pytest

from sakura.flow import engine, process, runner
from sakura.flow.process import ProcessOutcome
from flow_support import MonitorLauncher, make_context, task_workspace

SLEEPER = [sys.executable, "-c", "import time; time.sleep(60)"]


class ScriptedMonitor:
    """Monitor double: replays outcomes, records prompts, touches the log."""

    def __init__(self, outcomes):
        self.outcomes = list(outcomes)
        self.prompts = []

    def __call__(self, argv, **kwargs):
        self.prompts.append(kwargs["stdin_text"])
        kwargs["log_path"].write_text("")
        kwargs["on_start"](os.getpid())
        return self.outcomes.pop(0)


def _request(ctx, phase="plan"):
    return runner.RunRequest("planner", phase, "do $thing", {"thing": "it"}, task_workspace(ctx), {})


def _runner(ctx, monitor):
    return runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))


def _ctx(repo, **kwargs):
    return make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} -c pass"}}, **kwargs)


OK = ProcessOutcome(0, False, False, False, 1)
STALL = ProcessOutcome(None, True, False, False, 1)
TIMEOUT = ProcessOutcome(None, False, True, False, 1)


def test_steer_is_appended_once_and_moved(repo):
    ctx = _ctx(repo)
    ctx.store.steer_file.write_text("prefer short names\n")
    monitor = ScriptedMonitor([OK, OK])
    agent = _runner(ctx, monitor)
    agent.run(_request(ctx))
    agent.run(_request(ctx))
    assert "Operator steer:\nprefer short names" in monitor.prompts[0]
    assert "steer" not in monitor.prompts[1]
    assert not ctx.store.steer_file.exists()
    assert (ctx.store.runs_dir / "steer-1.txt").read_text().strip() == "prefer short names"
    assert "steer delivered" in ctx.store.ledger.read_text()


def test_blank_steer_is_not_delivered(repo):
    ctx = _ctx(repo)
    ctx.store.steer_file.write_text("  \n")
    monitor = ScriptedMonitor([OK])
    _runner(ctx, monitor).run(_request(ctx))
    assert "Operator steer" not in monitor.prompts[0]


@pytest.mark.parametrize("first", [STALL, TIMEOUT])
def test_stalled_run_restarts_once_with_same_prompt(repo, first):
    ctx = _ctx(repo)
    ctx.store.steer_file.write_text("steer me\n")
    monitor = ScriptedMonitor([first, OK])
    result = _runner(ctx, monitor).run(_request(ctx))
    assert result.exit_code == 0 and not result.stalled
    assert monitor.prompts[0] == monitor.prompts[1]
    assert ctx.store.ledger.read_text().count("steer delivered") == 1
    assert "stall restart plan" in ctx.store.ledger.read_text()


def test_second_stall_returns_stalled_result(repo):
    ctx = _ctx(repo)
    monitor = ScriptedMonitor([STALL, STALL])
    result = _runner(ctx, monitor).run(_request(ctx))
    assert result.stalled and len(monitor.prompts) == 2
    assert ctx.store.ledger.read_text().count("stall restart") == 1


def test_stall_reason_names_phase_and_keeps_log_path():
    from pathlib import Path
    from sakura.flow.phases import runs

    stalled = runs.failure_outcome(SimpleNamespace(exit_code=None, stalled=True, timed_out=False, stopped=False, log_path=Path("r.log")), "planner", "plan")
    assert stalled.reason == "stalled: plan; see r.log"
    timed = runs.failure_outcome(SimpleNamespace(exit_code=None, stalled=True, timed_out=True, stopped=False, log_path=Path("r.log")), "planner", "plan")
    assert timed.reason == "timed out: plan; see r.log" and timed.event == "escalation"


def test_timed_out_run_result_flags_timeout(repo):
    ctx = _ctx(repo)
    result = _runner(ctx, ScriptedMonitor([TIMEOUT, TIMEOUT])).run(_request(ctx))
    assert result.stalled and result.timed_out


def test_stopped_run_is_not_restarted(repo):
    ctx = _ctx(repo)
    monitor = ScriptedMonitor([ProcessOutcome(None, False, False, True, 1)])
    result = _runner(ctx, monitor).run(_request(ctx))
    assert result.stopped and len(monitor.prompts) == 1


def test_stalled_failure_escalates():
    from pathlib import Path
    from sakura.flow.phases import runs

    result = SimpleNamespace(exit_code=None, stalled=True, stopped=False, log_path=Path("r.log"))
    outcome = runs.failure_outcome(result, "planner", "plan")
    assert (outcome.kind, outcome.reason, outcome.event) == ("blocked", "stalled: plan; see r.log", "escalation")


def test_engine_notifies_with_outcome_event(repo):
    seen = []
    ctx = make_context(repo)
    ctx = engine.FlowContext(ctx.repo, ctx.store, ctx.config, ctx.runner,
                             type("N", (), {"notify": lambda self, *a: seen.append(a)})(), ctx.steps)
    engine._stop_at(ctx, "plan", engine.PhaseOutcome("blocked", "stalled: plan", event="escalation"))
    assert seen[0][0] == "escalation"


def test_stop_file_makes_engine_stop_between_phases(repo):
    ctx = make_context(repo)
    ctx.store.stop_file.write_text("now operator\n")
    assert engine.advance(ctx) == 5
    assert ctx.store.read_state()["status"] == "stopped"


def _record_active(ctx, proc):
    command = process.command_of(proc.pid)
    ctx.store.update_state(lambda state: state.update(active_run={"pid": proc.pid, "command": command}))


def test_resume_kills_stale_run_by_exact_pid(repo):
    ctx = make_context(repo)
    proc = subprocess.Popen(SLEEPER)
    try:
        _record_active(ctx, proc)
        engine.kill_stale_run(ctx)
        proc.wait(timeout=10)
        assert not process.is_alive(proc.pid)
        assert ctx.store.read_state()["active_run"] is None
        assert f"killed stale run {proc.pid}" in ctx.store.ledger.read_text()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


def test_resume_does_not_kill_reused_pid(repo, monkeypatch):
    ctx = make_context(repo)
    with subprocess.Popen([sys.executable, "-c", "import signal; signal.pause()"]) as child:
        monkeypatch.setattr(process, "_process_table", lambda: {child.pid: (os.getpid(), "a different live process")})
        try:
            ctx.store.update_state(lambda state: state.update(active_run={"pid": child.pid, "command": "old process"}))
            engine.kill_stale_run(ctx)
            assert child.poll() is None
            assert ctx.store.read_state()["active_run"] is None
        finally:
            if child.poll() is None:
                child.kill()


def test_wall_clock_gap_is_ledgered(tmp_path):
    gate = tmp_path / "gap-gate"
    os.mkfifo(gate)
    descriptor = os.open(gate, os.O_RDWR)
    ticks = iter((0, 0, 100, 100))
    gaps, polls = [], []

    def poll():
        polls.append(True)
        if len(polls) == 3:
            os.write(descriptor, b"x")

    try:
        outcome = process.monitored_run(
            [sys.executable, "-c", f"with open({str(gate)!r}, 'rb') as gate: gate.read(1)"],
            cwd=tmp_path, env={}, log_path=tmp_path / "log", budget_seconds=60, idle_seconds=60,
            stop_file=None, on_start=lambda pid: None, on_gap=gaps.append, on_poll=poll,
            wall_clock=lambda: next(ticks, 100), own_group=False,
        )
    finally:
        os.close(descriptor)
    assert outcome.exit_code == 0 and gaps == [100]


def test_no_gap_is_ledgered_for_steady_clock(tmp_path):
    gaps = []
    process.monitored_run(
        [sys.executable, "-c", "import time; time.sleep(0.5)"], cwd=tmp_path, env=dict(os.environ),
        log_path=tmp_path / "log", budget_seconds=60, idle_seconds=60, stop_file=None, on_start=lambda pid: None,
        on_gap=gaps.append)
    assert gaps == []


def test_runner_ledgers_wall_clock_gap(repo):
    ctx = _ctx(repo)

    def monitor(argv, **kwargs):
        kwargs["log_path"].write_text("")
        kwargs["on_gap"](125)
        return OK

    _runner(ctx, monitor).run(_request(ctx))
    assert "wall-clock gap of 125 s" in ctx.store.ledger.read_text()


def test_hold_awake_runs_caffeinate_on_darwin_and_kills_it(monkeypatch, tmp_path):
    fake = tmp_path / "caffeinate"
    fake.write_text(f"#!{sys.executable}\nimport signal\nsignal.pause()\n")
    fake.chmod(0o755)
    starts = {}
    popen = process.subprocess.Popen

    def spawn(argv, **kwargs):
        child = popen(argv, **kwargs)
        starts[child.pid] = (os.getpid(), " ".join(argv))
        return child

    monkeypatch.setattr(process, "_pinned", lambda _name: str(fake))
    monkeypatch.setattr(process, "_process_table", starts.copy)
    monkeypatch.setattr(process, "subprocess", SimpleNamespace(Popen=spawn, DEVNULL=subprocess.DEVNULL))
    monkeypatch.setattr(process.sys, "platform", "darwin")
    with process.hold_awake(4242) as pid:
        assert pid is not None and process.is_alive(pid)
        assert starts[pid][1].endswith("caffeinate -i -w 4242")
    assert not process.is_alive(pid)


def test_hold_awake_kills_caffeinate_when_body_raises(monkeypatch, tmp_path):
    fake = tmp_path / "caffeinate"
    fake.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(60)\n")
    fake.chmod(0o755)
    real_path = process.programs.path
    monkeypatch.setattr(process.programs, "path",
                        lambda name, location=None: str(fake) if name == "caffeinate" else real_path(name, location))
    monkeypatch.setattr(process.sys, "platform", "darwin")
    held = []
    with pytest.raises(RuntimeError):
        with process.hold_awake(1) as pid:
            held.append(pid)
            raise RuntimeError("boom")
    assert not process.is_alive(held[0])


def test_hold_awake_is_a_noop_off_darwin(monkeypatch):
    monkeypatch.setattr(process.sys, "platform", "linux")
    with process.hold_awake(1) as pid:
        assert pid is None


def test_hold_awake_is_a_noop_without_caffeinate(monkeypatch, tmp_path):
    def missing(name, location=None):
        raise process.programs.ProgramMissing(f"no {name} on PATH")

    monkeypatch.setattr(process.sys, "platform", "darwin")
    monkeypatch.setattr(process.programs, "path", missing)
    with process.hold_awake(1) as pid:
        assert pid is None


def test_resume_kills_every_concurrent_run(repo):
    ctx = make_context(repo)
    procs = [subprocess.Popen(SLEEPER), subprocess.Popen(SLEEPER)]
    try:
        runs = [{"pid": proc.pid, "command": process.command_of(proc.pid)} for proc in procs]
        ctx.store.update_state(lambda state: state.update(active_run=runs[-1], active_runs=runs))
        engine.kill_stale_run(ctx)
        for proc in procs:
            proc.wait(timeout=10)
        state = ctx.store.read_state()
        assert state["active_run"] is None and state["active_runs"] == []
    finally:
        for proc in procs:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
