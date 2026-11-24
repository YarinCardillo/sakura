import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

from sakura import loadinfo, slots


def test_load_snapshot_reports_slots_queue_and_load_average(tmp_path, monkeypatch):
    queue = slots.SlotQueue(tmp_path, 1)
    lease = queue.acquire("pytest -q", False, 1)
    monkeypatch.setattr(loadinfo.os, "getloadavg", lambda: (1.0, 2.0, 3.0))
    snapshot = loadinfo.load_snapshot(queue)
    lease.release()
    assert snapshot == {
        "slots_total": 1,
        "slots_in_use": 1,
        "queue": [],
        "loadavg": [1.0, 2.0, 3.0],
    }


def test_load_snapshot_shows_waiting_command(tmp_path, monkeypatch):
    now = SimpleNamespace(value=123.0)
    monkeypatch.setattr(loadinfo, "time", SimpleNamespace(time=lambda: now.value))
    monkeypatch.setattr(slots, "time", SimpleNamespace(
        time=lambda: now.value, time_ns=iter((1, 2)).__next__,
        monotonic=lambda: 0, sleep=lambda _: released.wait(10),
    ))
    queue = slots.SlotQueue(tmp_path, 1)
    inserted = threading.Event()
    released = threading.Event()
    publish = queue._new_ticket

    def publish_ticket(label, critical):
        ticket = publish(label, critical)
        if label == "npm test":
            inserted.set()
        return ticket

    def wait_for_slot():
        with queue.acquire("npm test", False, 10):
            return True

    monkeypatch.setattr(queue, "_new_ticket", publish_ticket)
    with ThreadPoolExecutor(max_workers=1) as workers:
        with queue.acquire("holder", False, 10):
            waiter = workers.submit(wait_for_slot)
            try:
                assert inserted.wait(10)
                expected = {"label": "npm test", "command": "npm test", "waiting_seconds": 0, "critical": False}
                assert loadinfo.load_snapshot(queue)["queue"] == [expected]
                now.value += 3.75
                assert loadinfo.load_snapshot(queue)["queue"] == [{**expected, "waiting_seconds": 3}]
                assert not waiter.done()
            finally:
                released.set()
        assert waiter.result(timeout=10)
