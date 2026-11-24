import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest
from sakura.flow import process


@pytest.fixture
def tracked_child(monkeypatch):
    """Park one child at registration and release it explicitly."""
    child = SimpleNamespace(pid=741, command="fixture child", running=False, kills=[])
    child.registered = threading.Event()
    child.release = threading.Event()
    child.finish = lambda: child.release.set()

    def monitor(_argv, *, on_start, on_poll=None, **kwargs):
        child.running = True
        child.nonce = kwargs.get("nonce", "")
        on_start(child.pid)
        child.registered.set()
        assert child.release.wait(15)
        if on_poll is not None and not child.kills:
            on_poll()
        child.running = False
        return process.ProcessOutcome(None if child.kills else 0, False, False, False, child.pid)

    def kill(pid, command):
        child.kills.append((pid, command))
        if (pid, command) != (child.pid, child.command):
            return False
        child.running = False
        child.finish()
        return True

    monkeypatch.setattr(process, "monitored_run", monitor)
    monkeypatch.setattr(process, "command_of", lambda pid: child.command if pid == child.pid else "")
    monkeypatch.setattr(process, "is_alive", lambda pid: pid == child.pid and child.running)
    monkeypatch.setattr(process, "kill_tree", kill)
    monkeypatch.setattr(process, "end_group", lambda _pid, _nonce: [])
    return child


def while_registered(child, run, assertions):
    """Inspect the registry once the child is held inside its launch."""
    with ThreadPoolExecutor(max_workers=1) as worker:
        outcome = worker.submit(run)
        try:
            assert child.registered.wait(10)
            assertions()
        finally:
            child.finish()
        return outcome.result(timeout=10)
