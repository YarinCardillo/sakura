"""One `leeway serve` of the located helper, kept running by the keeper so the readings stay fresh without the Leeway app.

Leeway's readings come from whichever process polls: the app or a `leeway serve`. They share one poll lock, so a second
copy defers and the serve is harmless next to the app. The keeper starts the serve detached (stdin closed, its own process
group), records its PID in the engine home, starts it again when it ends, and stops it by that PID when the keeper stops.

Several keepers (or a keeper restarted by launchd, which leaves the serve running) never run two: the check and the start
happen under a file lock, and a serve that is already running, whoever started it, is kept and left alone. A PID is only
trusted when the process is still the helper's `serve` (a reused PID is never signalled).
"""

from __future__ import annotations

import fcntl
import os
import signal
import subprocess
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home, programs
from sakura.usage import leeway

PID_FILE = "leeway-serve.pid"
LOCK_FILE = "leeway-serve.lock"
LOG_FILE = "leeway-serve.log"
FILE_MODE = 0o600
STOP_WAIT_SECONDS = 5.0
PS_PROGRAM = "/bin/ps"
SERVE_VERB = "serve"


@dataclass(frozen=True)
class Command:
    """What to run: the helper and its verb, and the environment it runs under."""

    argv: tuple[str, ...]
    env: Mapping[str, str]


def helper_command() -> Command | None:
    """`serve` of the helper the engine uses (`leeway.helper`) under the engine's fixed environment; None when none is found."""
    try:
        path = str(leeway.helper())
    except leeway.LeewayNotFound:
        return None
    return Command((path, SERVE_VERB), programs.environment("system", path))


class LeewayServe:
    """Keeps one serve running while the keeper runs. `command` says what to run (None: no helper, nothing to keep)."""

    def __init__(self, command: Callable[[], Command | None]) -> None:
        self._command = command
        self._process: subprocess.Popen | None = None

    def keep(self) -> None:
        """Start the serve unless one runs (this keeper's, another's or one left by a keeper that was restarted).

        Raises OSError when it cannot be started; the caller reports it and tries again at its next look.
        """
        if self._process is not None and self._process.poll() is None:
            return
        self._process = None
        command = self._command()
        if command is None:
            return
        with _locked():
            if _held_pid_runs(command.argv):
                return
            self._process = _start(command)
            _write_pid(self._process.pid)

    def stop(self) -> None:
        """Stop the serve this object started, by its PID; one started by another keeper is left running."""
        process, self._process = self._process, None
        if process is None or process.poll() is not None:
            return
        process.terminate()
        try:
            process.wait(STOP_WAIT_SECONDS)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        with _locked():
            if _read_pid() == process.pid:
                (engine_home.root() / PID_FILE).unlink(missing_ok=True)


def _start(command: Command) -> subprocess.Popen:
    log = os.open(engine_home.root() / LOG_FILE, os.O_WRONLY | os.O_CREAT | os.O_APPEND, FILE_MODE)
    try:
        return subprocess.Popen(list(command.argv), stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                env=dict(command.env), start_new_session=True, close_fds=True)
    finally:
        os.close(log)


class _locked:
    """An exclusive lock on the engine home's serve lock file, held for the check and the start of one keeper at a time."""

    def __enter__(self) -> None:
        path = engine_home.root() / LOCK_FILE
        self._handle = os.fdopen(os.open(path, os.O_RDWR | os.O_CREAT, FILE_MODE), "r+")
        fcntl.flock(self._handle, fcntl.LOCK_EX)

    def __exit__(self, *exc_info) -> None:
        self._handle.close()


def _held_pid_runs(argv: tuple[str, ...]) -> bool:
    pid = _read_pid()
    return pid is not None and _is_running(pid, argv)


def _read_pid() -> int | None:
    try:
        return int((engine_home.root() / PID_FILE).read_text().strip())
    except (FileNotFoundError, ValueError):
        return None


def _write_pid(pid: int) -> None:
    path = Path(engine_home.root() / PID_FILE)
    path.write_text(f"{pid}\n")
    path.chmod(FILE_MODE)


def _is_running(pid: int, argv: tuple[str, ...]) -> bool:
    """Whether `pid` is a live process started as `argv`: a PID that was reused by another program is not."""
    try:
        os.kill(pid, signal.SIG_DFL)
        shown = subprocess.run([PS_PROGRAM, "-p", str(pid), "-o", "command="], capture_output=True, text=True,
                               stdin=subprocess.DEVNULL, timeout=5).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return False
    wanted = " ".join(argv)
    return shown == wanted or shown.endswith(f" {wanted}")  # a script helper shows its interpreter first
