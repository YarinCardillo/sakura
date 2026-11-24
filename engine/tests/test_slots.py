import os
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace
from pathlib import Path

import pytest

from sakura import slots


def test_slot_count_uses_environment_then_default():
    assert slots.slot_count({"SAKURA_HEAVY_SLOTS": "2"}, 12) == 2
    assert slots.slot_count({}, 8) == 2
    assert slots.slot_count({}, 2) == 1


def test_critical_waiter_precedes_normal_waiters(tmp_path, monkeypatch):
    queue = slots.SlotQueue(tmp_path, 1)
    labels = ("normal", "critical-1", "critical-2")
    inserted = {label: threading.Event() for label in labels}
    acquired = []
    publish = queue._new_ticket

    def publish_ticket(label, critical):
        ticket = publish(label, critical)
        if label in inserted:
            inserted[label].set()
        return ticket

    def wait_for(label):
        with queue.acquire(label, label != "normal", 10):
            acquired.append(label)

    monkeypatch.setattr(queue, "_new_ticket", publish_ticket)
    with ThreadPoolExecutor(max_workers=3) as workers:
        with queue.acquire("holder", False, 10):
            futures = []
            for label in labels:
                futures.append(workers.submit(wait_for, label))
                assert inserted[label].wait(10), label
            assert not acquired
        for future in futures:
            future.result(timeout=10)
    assert acquired == ["critical-1", "critical-2", "normal"]


def test_dead_waiter_ticket_is_removed(tmp_path, monkeypatch):
    queue = slots.SlotQueue(tmp_path, 1)
    dead_pid = 42
    monkeypatch.setattr(slots, "_pid_alive", lambda pid: pid == os.getpid())
    stale = tmp_path / "queue" / f"1-{dead_pid}.json"
    stale.parent.mkdir(parents=True)
    stale.write_text(f'{{"label":"dead","critical":false,"since":0,"pid":{dead_pid}}}')
    assert slots._read_waiter(stale).pid == dead_pid
    with queue.acquire("live", False, 0):
        assert not stale.exists()


def test_lease_release_frees_slot_and_snapshot_lists_holder(tmp_path):
    queue = slots.SlotQueue(tmp_path, 1)
    lease = queue.acquire("compile", False, 1)
    snapshot = queue.snapshot()
    assert snapshot.total == 1
    assert len(snapshot.in_use) == 1
    assert snapshot.in_use[0].label == "compile"
    lease.release()
    assert queue.snapshot().in_use == []


def test_timeout_removes_waiter_ticket(tmp_path, monkeypatch):
    clock = iter((0, 0, 1))
    monkeypatch.setattr(slots, "time", SimpleNamespace(
        monotonic=lambda: next(clock), sleep=lambda _: None,
        time=lambda: 123, time_ns=iter((1, 2)).__next__,
    ))
    queue = slots.SlotQueue(tmp_path, 1)
    with queue.acquire("holder", False, None):
        with pytest.raises(slots.SlotTimeout):
            queue.acquire("waiter", False, 0.02)
    assert queue.snapshot().waiting == []


def test_dead_holder_releases_slot_and_stale_ticket_is_cleaned(tmp_path):
    script = ("from sakura.slots import SlotQueue; import os; "
              f"SlotQueue({str(tmp_path)!r}, 1).acquire('dead-holder', False, 1); os._exit(0)")
    env = {**os.environ, "PYTHONPATH": str(Path(__file__).parents[1] / "lib")}
    child = subprocess.run([sys.executable, "-c", script], env=env, timeout=10, check=True)
    assert child.returncode == 0
    queue = slots.SlotQueue(tmp_path, 1)
    assert not queue.snapshot().in_use
    with queue.acquire("next", False, 0):
        assert queue.snapshot().waiting == []
