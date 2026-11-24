import threading
from types import SimpleNamespace

import pytest
from sakura.flow import ports, process, stack


class StackProcess:
    def __init__(self):
        self.pid = 741
        self.command = "fixture app"
        self.port = None
        self.cwd = None
        self.ended = threading.Event()
        self.tick = threading.Event()
        self.poll_returned = threading.Event()
        self.is_running = False
        self.crash = None
        self.foreign = []
        self.may_poll = False

    def monitor(self, _argv, *, cwd, env, on_start, on_poll, **_kwargs):
        if self.crash:
            raise self.crash
        self.port, self.cwd = int(env["SAKURA_FLOW_PORT"]), cwd
        self.is_running = True
        on_start(self.pid)
        assert self.tick.wait(15)
        if self.may_poll:
            on_poll()
            self.poll_returned.set()
        assert self.ended.wait(15)
        return process.ProcessOutcome(0, False, False, False, self.pid)

    def listeners(self, port):
        own = [self.pid] if port == self.port and self.is_running else []
        return [*own, *self.foreign]

    def signal(self, pid, command):
        if (pid, command) != (self.pid, self.command):
            return False
        self.is_running = False
        self.tick.set()
        self.ended.set()
        return True


@pytest.fixture
def stack_process(monkeypatch):
    app = StackProcess()
    monkeypatch.setattr(ports, "is_free", lambda _port: True)
    monkeypatch.setattr(stack.ReadyProbe, "answers", lambda ready: bool(app.listeners(app.port)))
    monkeypatch.setattr(process, "monitored_run", app.monitor)
    monkeypatch.setattr(process, "listeners", app.listeners)
    monkeypatch.setattr(process, "group_members", lambda pid: [app.pid] if pid == app.pid else [])
    monkeypatch.setattr(process, "is_alive", lambda pid: app.is_running and pid == app.pid)
    monkeypatch.setattr(process, "command_of", lambda pid: app.command if pid == app.pid else "")
    monkeypatch.setattr(process, "kill_tree", app.signal)
    monkeypatch.setattr(process, "end_group", lambda pid, nonce: [])
    try:
        yield app
    finally:
        app.tick.set()
        app.ended.set()


@pytest.fixture
def stack_clock(monkeypatch):
    now = SimpleNamespace(value=100.0, pauses=[])
    real_time = stack.time

    def advance(seconds):
        now.pauses.append(seconds)
        now.value += seconds

    monkeypatch.setattr(stack, "time", SimpleNamespace(
        monotonic=lambda: now.value, sleep=advance, time=real_time.time,
    ))
    return now
