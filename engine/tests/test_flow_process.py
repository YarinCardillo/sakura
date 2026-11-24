import os
import shutil
import signal
import re
import subprocess
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import cage, completion, engine, logread, process, stacks, store
from sakura.flow.phases import integrate
from flow_support import make_context
from process_support import while_registered


def _run(tmp_path, script, **kwargs):
    pids = []
    options = dict(cwd=tmp_path, env=dict(os.environ), log_path=tmp_path / "run.log", budget_seconds=30,
                   idle_seconds=30, stop_file=None, on_start=pids.append)
    options.update(kwargs)
    return process.monitored_run([sys.executable, "-c", script], **options), pids


def test_monitored_run_reports_exit_and_feeds_stdin(tmp_path):
    outcome, pids = _run(tmp_path, "import sys; print('got', sys.stdin.read())", stdin_text="hello")
    assert outcome.exit_code == 0 and not outcome.stalled and not outcome.timed_out
    assert pids == [outcome.pid]
    assert "got hello" in (tmp_path / "run.log").read_text()


def test_monitored_run_detects_idle_stall(tmp_path, monkeypatch):
    now = SimpleNamespace(value=0)
    terminated = []
    monkeypatch.setattr(process, "time", SimpleNamespace(
        monotonic=lambda: now.value, sleep=lambda _: setattr(now, "value", now.value + 1),
    ))
    def exit_if_past_deadline(child, _is_held):
        if now.value < 2:
            return False
        child.terminate()
        child.wait(timeout=10)
        return True

    monkeypatch.setattr(process, "_has_exited", exit_if_past_deadline)
    monkeypatch.setattr(process, "_stop_run", lambda child, *_: (terminated.append(child.pid), child.terminate()))
    outcome, _ = _run(tmp_path, "import signal; signal.pause()", idle_seconds=1, own_group=False)
    assert outcome.stalled and not outcome.timed_out
    assert terminated == [outcome.pid] and now.value == 1
    assert not process.is_alive(outcome.pid)


def test_monitored_run_enforces_budget(tmp_path):
    script = "import time\nwhile True:\n    print('tick', flush=True)\n    time.sleep(0.1)"
    outcome, _ = _run(tmp_path, script, budget_seconds=1)
    assert outcome.timed_out and not outcome.stalled
    assert not process.is_alive(outcome.pid)


def test_monitored_run_stops_on_stop_file(tmp_path):
    stop = tmp_path / "STOP"
    stop.write_text("")
    outcome, _ = _run(tmp_path, "import time; time.sleep(60)", stop_file=stop)
    assert outcome.stopped and outcome.exit_code is None


def test_kill_tree_refuses_mismatched_command():
    child = subprocess.Popen(["sleep", "30"])
    try:
        assert process.kill_tree(child.pid, "definitely-not-sleep") is False
        assert child.poll() is None
    finally:
        child.kill()
        child.wait()


def test_kill_tree_refuses_a_command_that_is_only_a_prefix_of_the_live_one():
    child = subprocess.Popen(["sleep", "30"])
    try:
        recorded = process.command_of(child.pid)[:-1]
        assert process.kill_tree(child.pid, recorded) is False
        assert child.poll() is None
    finally:
        child.kill()
        child.wait()


def test_kill_tree_kills_children_first_by_exact_pid(monkeypatch):
    parent, child, unrelated = 741, 742, 743
    table = {parent: (1, "fixture parent"), child: (parent, "fixture child"), unrelated: (1, "fixture child")}
    signalled = []

    def kill(pid, sig):
        signalled.append((pid, sig))
        del table[pid]

    monkeypatch.setattr(process, "_process_table", table.copy)
    monkeypatch.setattr(process, "is_alive", table.__contains__)
    monkeypatch.setattr(process, "os", SimpleNamespace(kill=kill))
    assert process.kill_tree(parent, "fixture parent") is True
    assert signalled == [(child, signal.SIGTERM), (parent, signal.SIGTERM)]
    assert set(table) == {unrelated}


def test_resume_kills_full_test_child_left_by_stopped_engine(repo, tracked_child):
    ctx = make_context(repo, own_repository=True, overrides={"tests": {"full": ["true"]}})
    checkout = completion.Checkout("HEAD", repo, cage.new_build_dir())

    def run():
        return integrate._run_command(ctx, checkout, "true", 1, ctx.store.operations_dir / "full.log")

    def inspect():
        active, = ctx.store.read_state()["active_runs"]
        assert active["pid"] == tracked_child.pid and tracked_child.running
        engine.kill_stale_run(ctx)

    try:
        while_registered(tracked_child, run, inspect)
        assert tracked_child.kills == [(tracked_child.pid, tracked_child.command)]
        assert not process.is_alive(tracked_child.pid)
        assert ctx.store.read_state()["active_runs"] == []
    finally:
        shutil.rmtree(checkout.build, ignore_errors=True)


def test_external_child_and_activity_renewal_preserve_live_controller_state(repo, monkeypatch, tracked_child):
    current_time = 100
    monkeypatch.setattr(stacks, "time", SimpleNamespace(time=lambda: current_time))
    ctx = make_context(repo, own_repository=True, overrides={"load": {"idle_stack_seconds": 1}}, state={"stacks": {
        "demo:0": {"pid": 321, "command": "app", "workspace": str(repo), "log": "app.log", "last_used": 98},
        "demo:old": {"pid": 322, "command": "old app", "workspace": str(repo), "log": "old.log", "last_used": 1},
    }})
    external = store.TaskStore(repo, "demo")

    def run():
        return process.external_tracked_run(
            external, [sys.executable, "-c", "pass"], cwd=repo, env={}, log_path=ctx.store.operations_dir / "external.log",
            budget_seconds=5, idle_seconds=3, stop_file=ctx.store.stop_file,
            on_poll=lambda: stacks.touch_store(external, "demo:0"),
        )

    def inspect():
        (record,) = external.external_runs()
        assert record["pid"] == tracked_child.pid
        assert ctx.store.read_state().get("active_runs", []) == []
        ctx.store.update_state(lambda state: state.update(phase="evaluate"))

    assert while_registered(tracked_child, run, inspect).exit_code == 0
    current = ctx.store.read_state()
    assert current["phase"] == "evaluate"
    assert current["stacks"]["demo:0"]["last_used"] == current_time
    assert current["stacks"]["demo:old"]["last_used"] == 1
    assert external.external_runs() == []


def test_parent_resume_kills_registered_external_child(repo, tracked_child):
    ctx = make_context(repo, own_repository=True)
    external = store.TaskStore(repo, ctx.store.task_id)

    def run():
        return process.external_tracked_run(
            external, [sys.executable, "-c", "pass"], cwd=repo, env={},
            log_path=ctx.store.operations_dir / "stale-external.log", budget_seconds=40, idle_seconds=35, stop_file=None,
        )

    def inspect():
        (record,) = external.external_runs()
        assert record["pid"] == tracked_child.pid and tracked_child.running
        engine.kill_stale_run(ctx)

    outcome = while_registered(tracked_child, run, inspect)
    assert outcome.exit_code is None
    assert tracked_child.kills == [(tracked_child.pid, tracked_child.command)]
    assert not process.is_alive(tracked_child.pid)
    assert external.external_runs() == []


# -- child environment: no route to the operator's tmux server ----------------

OWNER_TMUX = {"TMUX": "/private/tmp/tmux-501/default,123,0", "TMUX_PANE": "%3", "TMUX_TMPDIR": "/tmp/owner-tmux"}


@pytest.fixture(autouse=True)
def fresh_private_tmux_dir(monkeypatch):
    """Every test starts without the process-wide private tmux directory, so it makes (and checks) its own."""
    monkeypatch.setattr(process, "_private_tmux_dir", None, raising=False)


def test_child_environment_drops_tmux_identity_and_points_tmux_at_a_private_dir():
    base = {"PATH": "/usr/bin", **OWNER_TMUX}
    env = process.child_environment(base)
    assert "TMUX" not in env and "TMUX_PANE" not in env and env["PATH"] == "/usr/bin"
    private = Path(env["TMUX_TMPDIR"])
    assert private != Path(OWNER_TMUX["TMUX_TMPDIR"])
    assert private.is_dir() and not list(private.iterdir()) and private.stat().st_mode & 0o777 == 0o700
    assert base == {"PATH": "/usr/bin", **OWNER_TMUX}


def test_child_environment_reuses_one_private_dir_for_every_call(monkeypatch):
    made = []
    real = process.tempfile.mkdtemp
    monkeypatch.setattr(process.tempfile, "mkdtemp", lambda **kwargs: made.append(real(**kwargs)) or made[-1])
    first, second = (process.child_environment({})["TMUX_TMPDIR"] for _ in range(2))
    assert first == second == made[0] and len(made) == 1
    assert Path(first).stat().st_mode & 0o777 == 0o700


def test_child_environment_shares_the_private_dir_across_threads():
    results = []
    workers = [threading.Thread(target=lambda: results.append(process.child_environment({})["TMUX_TMPDIR"]))
               for _ in range(8)]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    assert len(set(results)) == 1 and len(results) == 8


def test_no_flow_module_hands_the_raw_environment_to_a_child():
    root = Path(process.__file__).parent
    offenders = [str(path.relative_to(root)) for path in root.rglob("*.py")
                 if re.search(r"dict\(os\.environ\)|\*\*os\.environ", path.read_text())]
    assert offenders == []


# -- a role that floods its output is stopped ----------------------------------------------------------------------

FLOOD = "import sys\nwhile True:\n    sys.stdout.write('x' * 65536)\n    sys.stdout.flush()\n"
MEBIBYTE = 1024 * 1024


def test_a_run_that_floods_its_output_is_terminated_with_the_overflow_status(tmp_path, monkeypatch):
    monkeypatch.setattr(process, "OUTPUT_CAP_BYTES", 2 * MEBIBYTE)
    outcome, _ = _run(tmp_path, FLOOD, idle_seconds=60, budget_seconds=60)
    assert outcome.overflowed and outcome.exit_code is None
    assert not outcome.stalled and not outcome.timed_out and not outcome.stopped
    assert not process.is_alive(outcome.pid)


def test_reading_back_the_log_of_a_flooded_run_stays_bounded(tmp_path, monkeypatch):
    monkeypatch.setattr(process, "OUTPUT_CAP_BYTES", 2 * MEBIBYTE)
    _run(tmp_path, FLOOD, idle_seconds=60, budget_seconds=60)
    assert len(logread.read_log(tmp_path / "run.log", limit=MEBIBYTE).encode()) <= MEBIBYTE


def test_a_run_under_the_cap_is_not_an_overflow(tmp_path, monkeypatch):
    monkeypatch.setattr(process, "OUTPUT_CAP_BYTES", 2 * MEBIBYTE)
    outcome, _ = _run(tmp_path, "print('x' * 1000)")
    assert outcome.exit_code == 0 and not outcome.overflowed


# -- the cap is measured on the open log, not on whatever the path names ---------------------------------------------

def _replace_log_then(after: str) -> str:
    """A child that moves its log aside, puts a fresh file at the old path, then runs `after`."""
    return ("import os, sys\nlog = sys.argv[1]\nos.rename(log, log + '.moved')\nopen(log, 'w').close()\n" + after)


def _run_replacing(tmp_path, after, **kwargs):
    log = tmp_path / "run.log"
    pids = []
    options = dict(cwd=tmp_path, env=dict(os.environ), log_path=log, budget_seconds=60, idle_seconds=60,
                   stop_file=None, on_start=pids.append)
    options.update(kwargs)
    script = _replace_log_then(after)
    return process.monitored_run([sys.executable, "-c", script, str(log)], **options)


def test_a_log_replaced_mid_run_stops_the_run(tmp_path):
    outcome = _run_replacing(tmp_path, "import time\nprint('written to the old inode', flush=True)\ntime.sleep(60)\n")
    assert outcome.log_replaced and outcome.exit_code is None and not outcome.stalled
    assert not process.is_alive(outcome.pid)


def test_the_cap_still_holds_when_the_log_is_moved_aside_and_the_run_floods(tmp_path, monkeypatch):
    monkeypatch.setattr(process, "OUTPUT_CAP_BYTES", 2 * MEBIBYTE)
    steady_flood = "import sys, time\nwhile True:\n    sys.stdout.write('x' * 65536)\n    sys.stdout.flush()\n    time.sleep(0.005)\n"
    outcome = _run_replacing(tmp_path, steady_flood)
    assert (outcome.log_replaced or outcome.overflowed) and outcome.exit_code is None
    assert not process.is_alive(outcome.pid)
    assert (tmp_path / "run.log.moved").stat().st_size < 16 * MEBIBYTE


def test_a_log_deleted_mid_run_stops_the_run(tmp_path):
    log = tmp_path / "run.log"
    script = "import os, sys, time\nos.unlink(sys.argv[1])\nprint('x', flush=True)\ntime.sleep(60)\n"
    outcome = process.monitored_run([sys.executable, "-c", script, str(log)], cwd=tmp_path, env=dict(os.environ),
                                    log_path=log, budget_seconds=60, idle_seconds=60, stop_file=None,
                                    on_start=lambda pid: None)
    assert outcome.log_replaced and not process.is_alive(outcome.pid)
