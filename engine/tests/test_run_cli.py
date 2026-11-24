import contextlib
import os
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest
from sakura import cli, slots
from sakura.cli import main
from sakura.flow import config as flow_config
from sakura.flow import process, stacks
from sakura.flow.store import TaskStore


@pytest.fixture(autouse=True)
def child_process_table(monkeypatch):
    children = {}
    start = process._start

    def record_child(argv, *args, **kwargs):
        child, pump = start(argv, *args, **kwargs)
        children[child.pid] = (os.getpid(), " ".join(argv))
        return child, pump

    monkeypatch.setattr(process, "_start", record_child)
    monkeypatch.setattr(process, "_process_table", lambda: children.copy())


def test_run_without_heavy_is_usage_error():
    assert main(["run", "--", "python3", "-c", "pass"]) == 2


def test_run_heavy_runs_command_and_returns_its_exit_code(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert main(["run", "--heavy", "--", sys.executable, "-c", "raise SystemExit(7)"]) == 7


def test_heavy_commands_queue_behind_free_slots(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("SAKURA_HEAVY_SLOTS", "1")
    first_started = threading.Event()
    second_waiting = threading.Event()
    order = []
    try_slots = slots.SlotQueue._try_slots
    run_child = cli._run_heavy_child

    def try_and_report(queue, ticket):
        label = slots._ticket_label(ticket)
        lease = try_slots(queue, ticket)
        if label == "second":
            second_waiting.set()
        return lease

    def record_launch(argv, *args):
        order.append(argv[-1])
        return run_child(argv, *args)

    def observe_child(child):
        if not first_started.is_set() and order == ["first"]:
            assert child.poll() is None
            first_started.set()

    monkeypatch.setattr(slots.SlotQueue, "_try_slots", try_and_report)
    monkeypatch.setattr(cli, "_run_heavy_child", record_launch)
    _on_watch_poll(monkeypatch, observe_child)
    with _held_child(tmp_path, label="first") as (argv, finish_first), ThreadPoolExecutor(max_workers=2) as workers:
        first = workers.submit(main, argv)
        try:
            assert first_started.wait(10)
            second = workers.submit(main, _heavy_argv("pass", label="second"))
            assert second_waiting.wait(10)
            waiting = slots.SlotQueue(slots.default_root(), 1).snapshot()
            assert [item.label for item in waiting.in_use] == ["first"]
            assert [item.label for item in waiting.waiting] == ["second"]
            assert order == ["first"]
        finally:
            finish_first()
        assert [first.result(timeout=10), second.result(timeout=10)] == [0, 0]
    assert order == ["first", "second"]


def _heavy_argv(script, *, label=None):
    label_args = ["--label", label] if label else []
    return ["run", "--heavy", *label_args, "--", sys.executable, "-c", script, label or "child"]


def test_heavy_command_refreshes_its_flow_stack(repo, tmp_path, monkeypatch):
    from sakura.flow.store import TaskStore

    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("SAKURA_FLOW_STACK", "task:repo")
    task_store = TaskStore(repo, "task")
    task_store.create({"task": "task", "stacks": {"task:repo": {
        "pid": 123, "command": "app", "workspace": str(repo), "log": "app.log", "last_used": 1,
    }}})

    assert main(["run", "--heavy", "--", sys.executable, "-c", "pass"]) == 0

    assert TaskStore(repo, "task").read_state()["stacks"]["task:repo"]["last_used"] > 1


def test_short_heavy_command_refreshes_recent_stack_at_start(repo, tmp_path, monkeypatch):
    from sakura.flow.store import TaskStore

    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("SAKURA_FLOW_TASK", "task")
    monkeypatch.setenv("SAKURA_FLOW_STACK", "task:repo")
    task_store = TaskStore(repo, "task")
    initial = time.time()
    task_store.create({"task": "task", "stacks": {"task:repo": {
        "pid": 123, "command": "app", "workspace": str(repo), "log": "app.log", "last_used": initial,
    }}})

    assert main(["run", "--heavy", "--", sys.executable, "-c", "pass"]) == 0

    assert TaskStore(repo, "task").read_state()["stacks"]["task:repo"]["last_used"] > initial


def test_heavy_command_does_not_recreate_missing_stack(repo, tmp_path, monkeypatch):
    from sakura.flow.store import TaskStore

    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("SAKURA_FLOW_TASK", "task")
    monkeypatch.setenv("SAKURA_FLOW_STACK", "task:missing")
    task_store = TaskStore(repo, "task")
    task_store.create({"task": "task", "stacks": {}})

    assert main(["run", "--heavy", "--", sys.executable, "-c", "pass"]) == 0

    assert TaskStore(repo, "task").read_state()["stacks"] == {}


def test_heavy_command_uses_persisted_flow_overrides(repo, tmp_path, monkeypatch):
    from sakura.flow.store import TaskStore

    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("SAKURA_FLOW_TASK", "task")
    monkeypatch.setenv("SAKURA_FLOW_STACK", "task:repo")
    overrides = {
        "load": {"slots": 1, "idle_stack_seconds": 2},
        "timeouts": {"evaluate": 3, "idle_seconds": 2},
    }
    store = TaskStore(repo, "task")
    store.create({"task": "task", "overrides": overrides, "stacks": {"task:repo": {
        "pid": 123, "command": "app", "workspace": str(repo), "log": "app.log",
        "last_used": time.time() - 1,
    }}})

    observed = {"load_overrides": [], "queue": [], "child": []}
    real_load = flow_config.load
    real_queue = slots.SlotQueue
    real_external_run = process.external_tracked_run

    def load_with_observation(path, **kwargs):
        observed["load_overrides"].append(kwargs.get("overrides"))
        return real_load(path, **kwargs)

    class RecordingQueue:
        def __init__(self, root, total):
            observed["queue"].append({"root": root, "total": total})
            self.queue = real_queue(root, total)

        def acquire(self, label, critical, timeout_seconds, stop_file):
            observed["queue"].append({"timeout": timeout_seconds})
            return self.queue.acquire(label, critical, timeout_seconds, stop_file)

    def external_run(task_store, argv, **kwargs):
        observed["child"].append((kwargs["budget_seconds"], kwargs["idle_seconds"]))
        return real_external_run(task_store, argv, **kwargs)

    monkeypatch.setattr(flow_config, "load", load_with_observation)
    monkeypatch.setattr(slots, "SlotQueue", RecordingQueue)
    monkeypatch.setattr(process, "external_tracked_run", external_run)

    assert main(["run", "--heavy", "--", sys.executable, "-c", "print('finished')"]) == 0

    assert observed["load_overrides"] == [overrides]
    assert observed["queue"] == [
        {"root": slots.default_root(), "total": 1}, {"timeout": 3}]
    assert observed["child"] == [(3, 2)]


def _heavy_task(repo, monkeypatch, *, idle_stack=10, budget=30):
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_FLOW_TASK", "task")
    values = {"load": {"slots": 1, "idle_stack_seconds": idle_stack, "critical": [], "nice": 10},
              "timeouts": {"evaluate": budget, "idle_seconds": 30}}
    monkeypatch.setattr(flow_config, "load", lambda _repo: SimpleNamespace(values=values))
    return TaskStore(repo, "task")


@contextlib.contextmanager
def _held_child(tmp_path, *, label=None):
    gate = tmp_path / "child-gate"
    os.mkfifo(gate)
    descriptor = os.open(gate, os.O_RDWR)
    try:
        script = f"with open({str(gate)!r}, 'rb') as gate: gate.read(1)"
        yield _heavy_argv(script, label=label), lambda: os.write(descriptor, b"x")
    finally:
        os.close(descriptor)


def _on_watch_poll(monkeypatch, callback):
    watch = process._watch

    def observe(proc, log, path, budget, idle, stop_file, on_gap, on_poll, wall_clock, cap, is_held):
        def poll():
            on_poll()
            callback(proc)
        return watch(proc, log, path, budget, idle, stop_file, on_gap, poll, wall_clock, cap, is_held)

    monkeypatch.setattr(process, "_watch", observe)


def test_long_heavy_command_keeps_flow_stack_active(repo, tmp_path, monkeypatch):
    store = _heavy_task(repo, monkeypatch, idle_stack=1)
    monkeypatch.setenv("SAKURA_FLOW_STACK", "task:repo")
    now = SimpleNamespace(value=100.0)
    monkeypatch.setattr(stacks, "time", SimpleNamespace(time=lambda: now.value))
    store.create({"task": "task", "stacks": {"task:repo": {
        "pid": 123, "command": "app", "workspace": str(repo), "log": "app.log", "last_used": 99,
    }}})
    samples = []

    with _held_child(tmp_path) as (argv, release):
        def advance_while_running(child):
            if len(samples) == 2:
                return
            assert child.poll() is None
            samples.append(store.read_state()["stacks"]["task:repo"]["last_used"])
            now.value += 1
            if len(samples) == 2:
                release()
        _on_watch_poll(monkeypatch, advance_while_running)
        assert main(argv) == 0

    assert samples == [100.0, 101.0]
    assert samples[1] > samples[0] + 0.8


def test_heavy_command_stops_when_flow_stop_is_requested(repo, tmp_path, monkeypatch, capsys):
    store = _heavy_task(repo, monkeypatch)
    store.create({"task": "task"})
    with _held_child(tmp_path) as (argv, release):
        def stop_after_start(child):
            assert child.poll() is None
            if store.stop_file.exists():
                release()
                child.wait(timeout=10)
                return
            store.stop_file.write_text("operator stop\n")
        _on_watch_poll(monkeypatch, stop_after_start)
        assert main(argv) == 1

    assert "heavy command stopped" in capsys.readouterr().out
    assert store.external_runs() == []


def test_heavy_command_enforces_flow_elapsed_budget(repo, tmp_path, monkeypatch, capsys):
    _heavy_task(repo, monkeypatch, budget=3)
    now = SimpleNamespace(value=0)
    monkeypatch.setattr(process, "time", SimpleNamespace(
        monotonic=lambda: now.value,
        sleep=lambda _: setattr(now, "value", now.value + 1),
    ))
    with _held_child(tmp_path) as (argv, release):
        def advance_or_release(child):
            assert child.poll() is None
            if now.value >= 5:
                release()
                child.wait(timeout=10)
        _on_watch_poll(monkeypatch, advance_or_release)
        assert main(argv) == 1

    assert "heavy command timed out" in capsys.readouterr().out
