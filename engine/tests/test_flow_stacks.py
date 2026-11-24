import copy
import threading

from lock_support import observe_contention
from pathlib import Path

import pytest

from sakura.flow import repository_commands, stacks
from sakura.flow.process import ProcessOutcome


@pytest.fixture(autouse=True)
def uncaged_plan(monkeypatch):
    """These tests fake the store and the config; how `app.stop` is caged is tested in test_flow_repository_commands."""
    monkeypatch.setattr(stacks.repository_commands, "plan", lambda _store, _config, command, *_args, **_kwargs:
                        repository_commands.Launch(["sh", "-c", command], {}))


class Store:
    def __init__(self, root):
        self.root = Path(root)
        self.state = {"stacks": {"app-1": {"pid": 123, "command": "python app.py", "workspace": ".",
                                                   "log": "app.log", "last_used": 1}}}
        self.ledger = []

    def read_state(self):
        return self.state

    def update_state(self, update):
        update(self.state)
        return self.state

    def append_ledger(self, phase, line):
        self.ledger.append((phase, line))


def test_idle_stack_is_stopped_and_kept(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(stacks.process, "is_alive", lambda _pid: True)
    monkeypatch.setattr(stacks.process, "kill_tree", lambda pid, command: calls.append((pid, command)) or True)
    ctx = type("Context", (), {"store": Store(tmp_path), "config": type("Config", (), {"values": {
        "load": {"idle_stack_seconds": 2}, "app": {"stop": ""}}})()})()
    stopped = stacks.sweep(ctx, now=10)
    assert stopped == ["app-1"]
    assert calls == [(123, "python app.py")]
    assert ctx.store.state["stacks"] == {}
    assert ctx.store.ledger == [("stack", "stack app-1 stopped after 9 s idle")]


def test_legacy_top_level_stack_log_remains_readable_and_stop_log_is_separated(tmp_path, monkeypatch):
    log = tmp_path / "runs" / "app-legacy.log"
    log.parent.mkdir()
    log.write_text("legacy app output\n")
    calls = []
    monkeypatch.setattr(stacks.process, "is_alive", lambda _pid: True)
    monkeypatch.setattr(stacks.process, "kill_tree", lambda *_args: True)
    monkeypatch.setattr(stacks.process, "monitored_run", lambda *args, **kwargs:
                        calls.append(kwargs["log_path"]) or ProcessOutcome(0, False, False, False, 0))
    store = Store(tmp_path)
    store.state["stacks"]["app-1"].update(workspace=str(tmp_path), log=str(log))
    ctx = type("Context", (), {"store": store, "config": type("Config", (), {"values": {
        "load": {"idle_stack_seconds": 2}, "app": {"stop": "echo stopped"}, "knobs": {"cage": "default"}, "checks": {"approved": [], "remembered": []}}, "origins": {"app.stop": "personal"}})()})()

    stacks.sweep(ctx, now=10)

    assert log.read_text() == "legacy app output\n"
    assert calls == [tmp_path / "runs" / "operations" / "app-legacy.stop.log"]


def test_sweep_rechecks_activity_after_stale_snapshot(tmp_path, monkeypatch):
    class RefreshingStore(Store):
        def __init__(self, root):
            super().__init__(root)
            self.first_read = True

        def read_state(self):
            snapshot = copy.deepcopy(self.state)
            if self.first_read:
                self.first_read = False
                stacks.touch_store(self, "app-1")
            return snapshot

    stopped = []
    monkeypatch.setattr(stacks.time, "time", lambda: 9)
    monkeypatch.setattr(stacks, "_stop_entry", lambda _store, _config, _stack_id, entry: stopped.append(entry["pid"]))
    store = RefreshingStore(tmp_path)
    config = type("Config", (), {"values": {"load": {"idle_stack_seconds": 2}, "app": {"stop": ""}}})()

    result = stacks.sweep_store(store, config, now=10)

    assert result == []
    assert stopped == []
    assert store.state["stacks"]["app-1"]["last_used"] == 9


def test_two_sweepers_stop_one_stack_once(tmp_path, monkeypatch):
    blocked = observe_contention(monkeypatch, stacks, {"second-sweeper"})
    stop_entered = threading.Event()
    release_stop = threading.Event()
    stop_calls = []

    def stop(_store, _config, _stack_id, entry):
        stop_calls.append(entry["pid"])
        if len(stop_calls) == 1:
            stop_entered.set()
            assert release_stop.wait(timeout=15)

    monkeypatch.setattr(stacks, "_stop_entry", stop)
    store = Store(tmp_path)
    config = type("Config", (), {"values": {"load": {"idle_stack_seconds": 2}, "app": {"stop": ""}}})()
    results = []
    first = threading.Thread(target=lambda: results.append(stacks.sweep_store(store, config, now=10)))
    second = threading.Thread(name="second-sweeper",
                              target=lambda: results.append(stacks.sweep_store(store, config, now=10)))
    first.start()
    assert stop_entered.wait(timeout=5)
    second.start()
    try:
        assert blocked.acquire(timeout=10)
    finally:
        release_stop.set()
    first.join(timeout=5)
    second.join(timeout=5)

    assert not first.is_alive()
    assert not second.is_alive()
    assert stop_calls == [123]
    assert sorted(results) == [[], ["app-1"]]
    assert store.state["stacks"] == {}


def test_idle_stop_receives_saved_stack_identity_and_port(tmp_path, monkeypatch):
    store = Store(tmp_path)
    store.state["stacks"]["app-1"].update(port=41000)
    captured = []
    monkeypatch.setattr(stacks.process, "is_alive", lambda _pid: False)
    monkeypatch.setattr(stacks.process, "monitored_run", lambda *_args, **kwargs: captured.append(kwargs["env"]) or
                        ProcessOutcome(0, False, False, False, 0))
    config = type("Config", (), {"values": {"load": {"idle_stack_seconds": 2}, "app": {"stop": "stop-app"}}, "origins": {"app.stop": "personal"}})()

    stacks.sweep_store(store, config, now=10)

    assert captured[0]["SAKURA_FLOW_STACK"] == "app-1"
    assert captured[0]["SAKURA_FLOW_PORT"] == "41000"


def test_idle_stop_failure_keeps_recovery_record(tmp_path, monkeypatch):
    store = Store(tmp_path)
    monkeypatch.setattr(stacks.process, "is_alive", lambda _pid: False)
    monkeypatch.setattr(stacks.process, "monitored_run", lambda *_args, **_kwargs:
                        ProcessOutcome(7, False, False, False, 0))
    config = type("Config", (), {"values": {"load": {"idle_stack_seconds": 2}, "app": {"stop": "stop-app"}}, "origins": {"app.stop": "personal"}})()

    with pytest.raises(RuntimeError, match="app.stop failed"):
        stacks.sweep_store(store, config, now=10)

    assert "app-1" in store.state["stacks"]
    assert store.ledger == []
