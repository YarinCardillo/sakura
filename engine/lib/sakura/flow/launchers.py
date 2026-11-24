"""Launchers own the process handle of a role run: where it runs, how it is found again and how it is killed."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import time
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Protocol

from sakura.flow import acp, live_input, process
from sakura.flow.acp import Turn

LOCAL_HOST = "local"
ACP_EXIT_GRACE_SECONDS = 0.3


@dataclass(frozen=True)
class LaunchRequest:
    """Everything a launcher needs to run one role; `on_gap` and `on_poll` are local supervision hooks."""

    argv: list[str]
    cwd: Path
    env: dict
    log_path: Path
    budget_seconds: int
    idle_seconds: int
    stop_file: Path | None
    stdin_text: str = ""
    nice: int | None = None
    on_gap: Callable[[int], None] | None = None
    on_poll: Callable[[], None] | None = None
    # A Claude role the engine talks to while it runs (`flow/live_input.py`); `stdin_text` is then its first message.
    live: live_input.LiveChannel | None = None
    acp: Turn | None = None


class Launcher(Protocol):
    """Starts runs on one host. Its identities are opaque to the engine, which never reads a PID itself."""

    host: str

    def run(self, request: LaunchRequest, on_start: Callable[[dict], None]) -> process.ProcessOutcome:
        """Run to completion; call `on_start(identity)` as soon as the run exists."""

    def is_alive(self, identity: dict) -> bool:
        """Return whether the run behind `identity` is still the same live run."""

    def kill(self, identity: dict) -> None:
        """Terminate the run behind `identity`; a run that is gone or no longer matches is left alone, but what is
        left of a local run's process group is ended by its recorded nonce whatever its leader's state."""


class LocalLauncher:
    """Runs on this machine; identity is the exact PID plus its command line, as a recycled PID would differ, and the
    process group and nonce the engine gave the run (`process.monitored_run`)."""

    host = LOCAL_HOST

    def run(self, request: LaunchRequest, on_start: Callable[[dict], None]) -> process.ProcessOutcome:
        if request.acp is not None:
            return _run_acp(request, request.acp, on_start)
        nonce = process.new_nonce()
        return process.monitored_run(
            request.argv, cwd=request.cwd, env=request.env, log_path=request.log_path,
            budget_seconds=request.budget_seconds, idle_seconds=request.idle_seconds, stop_file=request.stop_file,
            on_start=lambda pid: on_start({"pid": pid, "command": process.command_of(pid), "pgid": pid, "nonce": nonce}),
            stdin_text=request.stdin_text, on_gap=request.on_gap, on_poll=request.on_poll, nice=request.nice,
            nonce=nonce, live=request.live)

    @contextmanager
    def stdio(self, request: LaunchRequest, on_start: Callable[[dict], None]):
        """Own an interactive child and reap its process group even on a failed turn."""
        nonce = process.new_nonce()
        env = {**request.env, process.NONCE_VARIABLE: nonce}
        with subprocess.Popen(request.argv, cwd=request.cwd, env=env, start_new_session=True,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as child:
            try:
                try:
                    command = process.command_of(child.pid)
                except OSError:
                    command = ""
                on_start({"pid": child.pid, "command": command, "pgid": child.pid, "nonce": nonce})
                yield child
            finally:
                with suppress(ProcessLookupError, PermissionError):
                    os.killpg(child.pid, signal.SIGTERM)
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except PermissionError:
                    # macOS refuses a signal to a process group containing only zombies.
                    child.kill()
                child.wait()

    def is_alive(self, identity: dict) -> bool:
        pid, command = identity.get("pid"), identity.get("command")
        if identity.get("transport") == "acp" and process.acp_alive(pid, identity.get("pgid"), identity.get("nonce")):
            return True
        return isinstance(pid, int) and bool(command) and process.command_of(pid) == command

    def kill(self, identity: dict) -> None:
        if self.is_alive(identity):
            process.kill_tree(identity["pid"], identity["command"])
        if identity.get("nonce"):
            process.end_group(identity["pgid"], identity["nonce"])



class _TurnLog:
    def __init__(self, request: LaunchRequest, stream):
        self.request = request
        self.stream = stream
        self.changed = time.monotonic()
        self.wall = time.time()
        self.size = os.fstat(stream.fileno()).st_size
        self.next_poll = self.changed

    def write(self, kind: str, **fields) -> None:
        data = (json.dumps({"type": kind, **fields}) + "\n").encode()
        self.size += len(data)
        if self.size > process.OUTPUT_CAP_BYTES:
            raise acp.AcpError("adapter_output_limit")
        self.stream.write(data)
        self.stream.flush()

    def activity(self) -> None:
        self.changed = time.monotonic()

    def check(self) -> None:
        request = self.request
        if request.stop_file is not None and request.stop_file.exists():
            raise acp.AcpError("stopped")
        if not process._names(request.log_path, os.fstat(self.stream.fileno())):
            raise acp.AcpError("log_replaced")
        now = time.monotonic()
        if now >= self.next_poll:
            self.next_poll = now + acp.POLL_SECONDS
            if request.on_gap is not None:
                self.wall = process._report_gap(self.wall, time.time(), request.on_gap)
            if request.on_poll is not None:
                request.on_poll()
        if now - self.changed >= request.idle_seconds:
            raise acp.AcpError("stalled")


def _run_acp(request: LaunchRequest, turn: acp.Turn, on_start: Callable[[dict], None]) -> process.ProcessOutcome:
    nonce = process.new_nonce()
    env = {**request.env, process.NONCE_VARIABLE: nonce}
    with request.log_path.open("ab") as stream:
        log = _TurnLog(request, stream)
        try:
            log.check()
            child = subprocess.Popen(request.argv, cwd=request.cwd, env=env, start_new_session=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except acp.AcpError as error:
            return _acp_outcome(0, "", str(error), log)
        except OSError:
            return _acp_outcome(0, "", "adapter_could_not_start", log)
        reply, reason = "", ""
        try:
            command = process.command_of(child.pid)
            on_start({"pid": child.pid, "command": command, "pgid": child.pid, "nonce": nonce, "transport": "acp"})
            client = acp.Client(child, request.budget_seconds, checkpoint=log.check)
            session = acp.Session(client, turn, log.write, log.activity)
            reply, reason = _run_session(session, request, log)
        finally:
            left = process.close_acp_child(child, nonce, ACP_EXIT_GRACE_SECONDS)
        if not reason:
            if left:
                reason = "adapter_left_processes"
            elif child.returncode != 0:
                reason = "adapter_bad_exit"
        return _acp_outcome(child.pid, reply, reason, log)


def _run_session(session: acp.Session, request: LaunchRequest, log: _TurnLog) -> tuple[str, str]:
    reply, reason = "", ""
    try:
        reply = session.run(request.cwd)
        log.check()
    except acp.AcpError as error:
        reason = str(error)
    except OSError:
        reason = "adapter_disconnected"
    finally:
        session.shutdown(cancel=bool(reason))
    return reply, reason


def _acp_outcome(pid: int, reply: str, reason: str, log: _TurnLog) -> process.ProcessOutcome:
    if not reason:
        try:
            log.check()
            log.write(acp.FINAL_EVENT, text=reply)
        except acp.AcpError as error:
            reason = str(error)
    if reason:
        with suppress(acp.AcpError):
            if reason != "log_replaced":
                log.write(acp.ERROR_EVENT, reason=reason)
    timed_out = reason == "adapter_timeout"
    stopped = reason == "stopped"
    stalled = reason == "stalled"
    overflow = reason in ("adapter_output_limit", "adapter_frame_limit")
    replaced = reason == "log_replaced"
    left = reason == "adapter_left_processes"
    exit_code = None if stopped or stalled or timed_out or overflow or replaced or left else int(bool(reason))
    return process.ProcessOutcome(exit_code, stalled, timed_out, stopped, pid, overflowed=overflow, log_replaced=replaced,
                                  left_running=left, final=reply if not reason else "", reason=reason)

def for_host(host: str) -> Launcher | None:
    """Return the launcher for `host`, or None when this machine has none."""
    return LocalLauncher() if host == LOCAL_HOST else None


def entry_is_alive(entry: dict) -> bool:
    """Return whether the run in an `active_runs` entry may still be running; a host with no launcher here is unknown, so yes."""
    launcher = for_host(entry.get("host", LOCAL_HOST))
    return launcher is None or launcher.is_alive(entry)
