"""Private tmux servers for tests: a throwaway socket, no user config, killed on exit."""

from __future__ import annotations

import os
import shutil
import subprocess
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Callable, Iterator

import pytest
from sakura.flow.tmux import TmuxClient

SOCKET_PREFIX = "sakura-test-"
POLL_SECONDS = 0.05
WAIT_SECONDS = 10.0
TMUX_PROGRAM = shutil.which("tmux", path="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
needs_tmux = pytest.mark.skipif(TMUX_PROGRAM is None, reason="tmux is not installed in a system tool directory")


def _isolated_run(argv, **kwargs):
    """Run tmux with `-f /dev/null` after the socket and without the caller's $TMUX and $TMUX_PANE."""
    if TMUX_PROGRAM is None:
        pytest.skip("tmux is not installed in a system tool directory")
    isolated = [TMUX_PROGRAM, *argv[1:3], "-f", "/dev/null", *argv[3:]]
    environment = {key: value for key, value in os.environ.items() if key not in ("TMUX", "TMUX_PANE")}
    return subprocess.run(isolated, env=environment, **kwargs)


def _socket_path(socket: str) -> Path:
    """Where tmux keeps the socket file of `-L socket`; kill-server leaves the file behind."""
    return Path(os.environ.get("TMUX_TMPDIR", "/tmp")) / f"tmux-{os.getuid()}" / socket


@contextmanager
def private_tmux() -> Iterator[TmuxClient]:
    """Yield a TmuxClient bound to a private server; the server is killed on exit."""
    socket = SOCKET_PREFIX + uuid.uuid4().hex
    client = TmuxClient(socket=socket, run=_isolated_run)
    try:
        yield client
    finally:
        _isolated_run(["tmux", "-L", socket, "kill-server"], capture_output=True, check=False)
        _socket_path(socket).unlink(missing_ok=True)


def wait_until(condition: Callable[[], object], seconds: float = WAIT_SECONDS) -> bool:
    """Poll `condition` until it is truthy or the time is up; return the last verdict."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(POLL_SECONDS)
    return bool(condition())
