import fcntl
import threading
from types import SimpleNamespace


def observe_contention(monkeypatch, module, contenders):
    """Report contended exclusive lock attempts for the selected threads."""
    blocked = threading.Semaphore(0)

    def flock(descriptor, operation):
        if operation == fcntl.LOCK_EX and threading.current_thread().name in contenders:
            try:
                return fcntl.flock(descriptor, operation | fcntl.LOCK_NB)
            except BlockingIOError:
                blocked.release()
        return fcntl.flock(descriptor, operation)

    monkeypatch.setattr(module, "fcntl", SimpleNamespace(
        flock=flock, LOCK_EX=fcntl.LOCK_EX, LOCK_NB=fcntl.LOCK_NB, LOCK_UN=fcntl.LOCK_UN,
    ))
    return blocked
