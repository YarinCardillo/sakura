"""Run one program under supervision: its own process group, stdin closed, a working folder of its own, an absolute
deadline and a cap on what it may print, with the whole group ended when any of them is passed.

Both pipes are drained as the program runs. Standard output is kept (up to the cap, it is the program's answer);
standard error is only counted and its last bytes kept: the diagnostics of a program may carry whatever it was
talking to (an address with a token in it), so they are the caller's to keep private and never to publish.
Nothing the program prints touches the disk.
"""

from __future__ import annotations

import os
import select
import shutil
import signal
import subprocess
import tempfile
import time
from collections import deque
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura.digest import schema

READ_CHUNK = 65536
POLL_SECONDS = 0.2
TERMINATE_GRACE_SECONDS = 2
STDERR_TAIL_BYTES = 2000
MAX_STDERR_BYTES = 1_000_000
WORK_FOLDER_PREFIX = "sakura-digest-source-"


@dataclass(frozen=True)
class Execution:
    """How a program ended: `exit_code` is None when it was ended for its time or its output.

    `stderr` is at most the last `STDERR_TAIL_BYTES` of what it wrote there, for the caller's own diagnosis only."""

    exit_code: int | None
    stdout: bytes
    stderr: str
    timed_out: bool = False
    over_limit: bool = False
    start_error: str = ""


def run_program(argv: tuple[str, ...], environment: Mapping[str, str], timeout_seconds: int, *,
                stdin_text: bytes | None = None) -> Execution:
    """Run `argv` in a folder of its own, in its own process group, with stdin closed (or holding only `stdin_text`,
    which the engine's own helpers use for what must not appear in a command line); at most
    `schema.MAX_SOURCE_BYTES` of output, `MAX_STDERR_BYTES` of diagnostics and `timeout_seconds` of time, after
    which its group is ended."""
    folder = Path(tempfile.mkdtemp(prefix=WORK_FOLDER_PREFIX))
    try:
        return _supervised(argv, environment, timeout_seconds, folder, stdin_text)
    except OSError as error:
        return Execution(None, b"", "", start_error=f"cannot run {Path(argv[0]).name}: {error.strerror or 'failed'}")
    finally:
        shutil.rmtree(folder, ignore_errors=True)


class _Output:
    """What has been read from the program's two pipes so far."""

    def __init__(self) -> None:
        self.stdout: list[bytes] = []
        self.stdout_size = 0
        self.stderr_size = 0
        self.stderr_tail: deque[int] = deque(maxlen=STDERR_TAIL_BYTES)

    def add_stdout(self, data: bytes) -> bool:
        self.stdout.append(data)
        self.stdout_size += len(data)
        return self.stdout_size <= schema.MAX_SOURCE_BYTES

    def add_stderr(self, data: bytes) -> bool:
        self.stderr_size += len(data)
        self.stderr_tail.extend(data)
        return self.stderr_size <= MAX_STDERR_BYTES

    def tail(self) -> str:
        return " ".join(bytes(self.stderr_tail).decode("utf-8", errors="replace").split())


def _supervised(argv: tuple[str, ...], environment: Mapping[str, str], timeout_seconds: int, folder: Path,
                stdin_text: bytes | None) -> Execution:
    deadline = time.monotonic() + timeout_seconds
    child = subprocess.Popen(list(argv), cwd=folder, env=dict(environment),
                             stdin=subprocess.DEVNULL if stdin_text is None else subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    output = _Output()
    try:
        _give_input(child, stdin_text)
        reason = _drain(child, output, deadline) or _exit_reason(child, deadline)
    finally:
        _end_group(child)
        child.wait()
        child.stdout.close()
        child.stderr.close()
    ended_early = reason in ("time", "size")
    return Execution(None if ended_early else child.returncode, b"" if reason == "size" else b"".join(output.stdout),
                     output.tail(), reason == "time", reason == "size")


def _give_input(child: subprocess.Popen, stdin_text: bytes | None) -> None:
    """Hand the program its input and close its stdin; it is a short text, far below the pipe's buffer."""
    if stdin_text is None:
        return
    try:
        child.stdin.write(stdin_text)
        child.stdin.close()
    except BrokenPipeError:  # the program ended without reading it
        pass


def _drain(child: subprocess.Popen, output: _Output, deadline: float) -> str:
    """Read both pipes until both close: "" then, or the reason it was stopped early ("time" or "size")."""
    open_pipes = {child.stdout: output.add_stdout, child.stderr: output.add_stderr}
    while open_pipes:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return "time"
        ready, _, _ = select.select(list(open_pipes), [], [], min(remaining, POLL_SECONDS))
        for pipe in ready:
            data = os.read(pipe.fileno(), READ_CHUNK)
            if not data:
                del open_pipes[pipe]
            elif not open_pipes[pipe](data):
                return "size"
    return ""


def _exit_reason(child: subprocess.Popen, deadline: float) -> str:
    """"time" when the program closed its output but did not exit before the deadline, else ""."""
    try:
        child.wait(timeout=max(0.0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        return "time"
    return ""


def _end_group(child: subprocess.Popen) -> None:
    """End the process group the engine started for `child` (its id is the group id): terminate, then kill. A group
    that is already gone is left alone."""
    for signal_number in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(child.pid, signal_number)
        except (ProcessLookupError, PermissionError):
            return
        if _group_gone(child):
            return


def _group_gone(child: subprocess.Popen) -> bool:
    deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
    while time.monotonic() < deadline:
        child.poll()
        try:
            os.killpg(child.pid, 0)
        except (ProcessLookupError, PermissionError):
            return True
        time.sleep(POLL_SECONDS / 4)
    return False
