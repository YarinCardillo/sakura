"""Machine load and heavy-work queue details for status output."""

from __future__ import annotations

import os
import time

from sakura import slots
from sakura.slots import SlotQueue


def load_snapshot(queue: SlotQueue) -> dict:
    """Return slot totals, labeled waiters and the platform load average."""
    snapshot = queue.snapshot()
    now = time.time()
    return {
        "slots_total": snapshot.total,
        "slots_in_use": len(snapshot.in_use),
        "queue": [
            {"label": item.label, "command": item.label, "waiting_seconds": max(0, int(now - item.since)),
             "critical": item.critical}
            for item in snapshot.waiting
        ],
        "loadavg": list(os.getloadavg()),
    }


def current_load(configured_slots: int) -> dict:
    """Return the load snapshot of this machine's shared heavy-work slots; `configured_slots` is `load.slots`."""
    total = slots.slot_count(os.environ, os.cpu_count() or 1, configured_slots)
    return load_snapshot(SlotQueue(slots.default_root(), total))
