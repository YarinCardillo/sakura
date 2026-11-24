"""Machine-wide file-lock slots for commands that use substantial resources."""

from __future__ import annotations

import fcntl
import json
import os
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

QUEUE_POLL_SECONDS = 0.2
SLOT_DIRECTORY = Path(".cache/sakura/slots")


class SlotTimeout(TimeoutError):
    """A command waited longer than its allowed queue budget."""


class SlotStopped(RuntimeError):
    """The operator stopped a command while it waited for a slot."""


@dataclass(frozen=True)
class SlotUse:
    """A slot currently held by a live process."""

    slot: int
    pid: int
    label: str


@dataclass(frozen=True)
class Waiter:
    """A live command waiting for a slot."""

    sequence: int
    pid: int
    label: str
    critical: bool
    since: float


@dataclass(frozen=True)
class QueueSnapshot:
    """Point-in-time view of the configured slots and their waiters."""

    total: int
    in_use: list[SlotUse]
    waiting: list[Waiter]


class SlotLease:
    """A held file lock, released explicitly or when its process exits."""

    def __init__(self, descriptor: int, ticket: Path) -> None:
        self._descriptor = descriptor
        self._ticket = ticket

    def release(self) -> None:
        """Release the lock and remove this process's queue ticket."""
        if self._descriptor is None:
            return
        fcntl.flock(self._descriptor, fcntl.LOCK_UN)
        os.close(self._descriptor)
        self._descriptor = None
        self._ticket.unlink(missing_ok=True)

    def __enter__(self) -> "SlotLease":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.release()


class SlotQueue:
    """Coordinate heavy commands through shared lock and ticket files."""

    def __init__(self, root: Path, total: int) -> None:
        if total < 1:
            raise ValueError("slot total must be at least 1")
        self.root = Path(root)
        self.total = total
        self.queue_dir = self.root / "queue"

    def acquire(self, label: str, critical: bool, timeout_seconds: float | None,
                stop_file: Path | None = None) -> SlotLease:
        """Wait in priority order, then lock a free slot; timeout removes the ticket."""
        ticket = self._new_ticket(label, critical)
        deadline = None if timeout_seconds is None else time.monotonic() + timeout_seconds
        try:
            while True:
                if stop_file is not None and stop_file.exists():
                    raise SlotStopped(f"stopped while waiting for heavy-work slot: {label}")
                self._remove_dead_tickets()
                if self._is_first(ticket):
                    lease = self._try_slots(ticket)
                    if lease is not None:
                        return lease
                if deadline is not None and time.monotonic() >= deadline:
                    raise SlotTimeout(f"timed out waiting for heavy-work slot: {label}")
                time.sleep(QUEUE_POLL_SECONDS)
        except BaseException:
            ticket.unlink(missing_ok=True)
            raise

    def snapshot(self) -> QueueSnapshot:
        """Return live slot holders and waiters, removing tickets from dead processes."""
        self._remove_dead_tickets()
        in_use = []
        self.root.mkdir(parents=True, exist_ok=True)
        for index in range(self.total):
            path = self.root / f"slot-{index}.lock"
            descriptor = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
            try:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    entry = _read_holder(path)
                    if entry:
                        in_use.append(SlotUse(index, int(entry["pid"]), str(entry["label"])))
                else:
                    fcntl.flock(descriptor, fcntl.LOCK_UN)
            finally:
                os.close(descriptor)
        waiting = sorted((_read_waiter(path) for path in self.queue_dir.glob("*.json")), key=_waiter_order)
        return QueueSnapshot(self.total, in_use, waiting)

    def _new_ticket(self, label: str, critical: bool) -> Path:
        self.root.mkdir(parents=True, exist_ok=True)
        self.queue_dir.mkdir(parents=True, exist_ok=True)
        while True:
            sequence = time.time_ns()
            path = self.queue_dir / f"{sequence}-{os.getpid()}.json"
            temporary = path.with_suffix(".tmp")
            try:
                descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            except FileExistsError:
                continue
            with os.fdopen(descriptor, "w") as stream:
                json.dump({"label": label, "critical": critical, "since": time.time(), "pid": os.getpid()}, stream)
                stream.flush()
                os.fsync(stream.fileno())
            try:
                os.replace(temporary, path)
            except FileExistsError:
                temporary.unlink(missing_ok=True)
                continue
            return path

    def _is_first(self, ticket: Path) -> bool:
        ordered = sorted(self.queue_dir.glob("*.json"), key=lambda path: _waiter_order(_read_waiter(path)))
        return bool(ordered and ordered[0] == ticket)

    def _try_slots(self, ticket: Path) -> SlotLease | None:
        for index in range(self.total):
            path = self.root / f"slot-{index}.lock"
            descriptor = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                os.close(descriptor)
                continue
            os.ftruncate(descriptor, 0)
            os.write(descriptor, json.dumps({"pid": os.getpid(), "label": _ticket_label(ticket)}).encode())
            os.fsync(descriptor)
            ticket.unlink(missing_ok=True)
            return SlotLease(descriptor, ticket)
        return None

    def _remove_dead_tickets(self) -> None:
        for path in self.queue_dir.glob("*.json") if self.queue_dir.exists() else ():
            waiter = _read_waiter(path)
            if not _pid_alive(waiter.pid):
                path.unlink(missing_ok=True)


def slot_count(env: Mapping[str, str], cores: int, configured: int = 0) -> int:
    """Resolve slots from the environment, config, or one third of available cores."""
    value = env.get("SAKURA_HEAVY_SLOTS")
    count = int(value) if value is not None else configured if configured else max(1, cores // 3)
    if count < 1:
        raise ValueError("SAKURA_HEAVY_SLOTS and load.slots must be at least 1")
    return count


def default_root(home: Path | None = None) -> Path:
    """Return the per-user cache directory shared by sakura worktrees."""
    return (home or Path.home()) / SLOT_DIRECTORY


def _read_waiter(path: Path) -> Waiter:
    try:
        data = json.loads(path.read_text())
        sequence = int(path.name.split("-", 1)[0])
        return Waiter(sequence, int(data["pid"]), str(data["label"]), bool(data["critical"]), float(data["since"]))
    except (OSError, ValueError, KeyError, TypeError):
        return Waiter(0, 0, path.name, False, 0)


def _waiter_order(waiter: Waiter) -> tuple[int, int]:
    return (0 if waiter.critical else 1, waiter.sequence)


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _read_holder(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def _ticket_label(path: Path) -> str:
    return str(json.loads(path.read_text())["label"])
