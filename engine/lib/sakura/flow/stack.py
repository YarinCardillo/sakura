"""The application under test: start it, wait until it answers, stop it by exact PID.

Every start is a session of its own on a fresh port (review 14, finding 2): one the port registry gives no other owner
and nothing listens on, held until the task's workspace is removed so no later session of the task gets it again
(review 15), and so no earlier session's cage was granted it; the host's fixed `app.origin` port is used only once
nothing listens there. With `app.start` set the app is ready only once the kernel's socket table shows a listener on
that port and every listener as a process of the app's own process group (the kernel's group: a nonce, which the app
could hand on, decides nothing), whatever answers there; the rows check the same before and after each run
(`flow/baseline.py`), and so does the end of the session.

Readiness is asked of the app's own origin only: `app.ready_url` (the repository's) may choose the path, but its
scheme, host and port must be exactly the origin's (`http://127.0.0.1:<allocated port>`, or the host's `app.origin`),
or the app does not start. The probe itself runs in the checks cage, which reaches loopback on the app's port alone,
and follows no redirect.
"""

from __future__ import annotations

import os
import string
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn
from urllib.parse import urlparse

from sakura.flow import cage, checks, controls, ports, process, repository_commands, stacks
from sakura.flow.engine import FlowContext
from sakura.flow.gitutil import tail
from sakura.flow.workspaces import Workspace

if TYPE_CHECKING:
    from sakura.flow import completion

APP_LIFETIME_SECONDS = 24 * 60 * 60
SESSION_TOKEN_CHARS = 12
READY_POLL_SECONDS = 0.2
PROBE_TIMEOUT_SECONDS = 2
# The caged probe's own start-up (sandbox-exec and the interpreter) on top of its HTTP timeout.
PROBE_STARTUP_SECONDS = 5
# One GET of the ready path, no redirect: exit 0 on a 200, else 1.
PROBE_SCRIPT = ("import http.client, sys\n"
                "try:\n"
                "    connection = http.client.HTTPConnection(sys.argv[1], int(sys.argv[2]), timeout=float(sys.argv[4]))\n"
                "    connection.request('GET', sys.argv[3])\n"
                "    sys.exit(0 if connection.getresponse().status == 200 else 1)\n"
                "except OSError:\n"
                "    sys.exit(1)\n")


class StackError(RuntimeError):
    """Raised when the app does not start or never becomes ready; the message includes the log tail."""


class ReadyProbe:
    """Asks the app's origin whether it is ready, from the checks cage: loopback only on the app's port."""

    def __init__(self, url: str, run_cage: cage.RunCage) -> None:
        self.url = url
        self._cage = run_cage

    def answers(self) -> bool:
        """True when the ready URL answers 200 to one GET; no redirect is followed."""
        parsed = urlparse(self.url)
        target = parsed.path or "/"
        argv = self._cage.wrap([sys.executable, "-c", PROBE_SCRIPT, parsed.hostname or "", str(parsed.port),
                                f"{target}?{parsed.query}" if parsed.query else target, str(PROBE_TIMEOUT_SECONDS)])
        try:
            done = subprocess.run(argv, capture_output=True, check=False, stdin=subprocess.DEVNULL,
                                  timeout=PROBE_TIMEOUT_SECONDS + PROBE_STARTUP_SECONDS,
                                  env={**process.scrubbed_environment(os.environ), **self._cage.env})
        except subprocess.TimeoutExpired:
            return False
        return done.returncode == 0


class AppStack:
    """A running app; use as a context manager to stop it on exit."""

    def __init__(self, ctx: FlowContext, workspace: Workspace, launch: repository_commands.Launch, log: Path,
                 source: Path, ready: ReadyProbe | None, session: tuple[int, str]) -> None:
        self.port, self._port_owner = session
        self.base_url = ctx.config.values["app"]["origin"] or f"http://127.0.0.1:{self.port}"
        self.pid: int | None = None
        self._ctx = ctx
        self._workspace = workspace
        self._source = source
        self._ready = ready
        self._env = launch.env
        self._argv = launch.argv
        self._log = log
        self._command = ""
        self._exit: process.ProcessOutcome | None = None
        self._error: Exception | None = None
        self._warned = False
        self._stop_requested = threading.Event()
        self._thread: threading.Thread | None = None
        self._stack_id = f"{ctx.store.task_id}:{workspace.name}"
        self._slot_context = None
        # The app runs in a group of its own that outlives `app.start` (a start command may daemonize): stopping the
        # stack ends that whole group and every process carrying this nonce.
        self._nonce = process.new_nonce()

    def __enter__(self) -> "AppStack":
        return self

    def __exit__(self, exc_type: object, *_exc: object) -> None:
        """Stop the app; a session that ended without its own listener on the port proves nothing (StackError)."""
        problem = ""
        if exc_type is None and self.pid is not None and self._ctx.config.values["app"]["start"]:
            problem = process.listener_problem(self.pid, self.port)
        self.stop()
        if problem:
            raise StackError(f"at the end of the session {problem}; nothing it ran against is proved")

    def launch(self) -> None:
        """Start `app.start` under supervision and wait for the ready URL to answer 200."""
        start = self._ctx.config.values["app"]["start"]
        if not start:
            return
        started = threading.Event()
        self._thread = threading.Thread(target=self._supervise, args=(start, started), daemon=True)
        self._thread.start()
        if not started.wait(self._ctx.config.values["app"]["ready_seconds"]) or self.pid is None:
            self._fail(f"app supervisor did not start the process: {self._error!r}")
        self._wait_ready()
        self._release_slot()

    def stop(self) -> None:
        """Kill the app by exact PID, run `app.stop` when configured, then end what is left of the app's group."""
        self._stop_requested.set()
        try:
            self._kill_started_process()
            if self._thread is not None:
                self._thread.join()
            self._kill_started_process()
            stacks.stop_registered(self._ctx, self._stack_id)
        finally:
            if self.pid is not None:
                process.end_group(self.pid, self._nonce)
            self._release_slot()

    def _kill_started_process(self) -> None:
        if self.pid is None or not process.is_alive(self.pid):
            return
        if not self._command or not process.kill_tree(self.pid, self._command):
            raise StackError(f"stack process {self.pid} no longer matches its recorded command")

    def _supervise(self, start: str, started: threading.Event) -> None:
        def remember(pid: int) -> None:
            self.pid = pid
            self._command = process.command_of(pid)
            stacks.register(self._ctx, self._stack_id, pid, self._command, self._workspace.path, self._log,
                            self.port, self._nonce)
            started.set()

        try:
            self._slot_context = process.heavy_slot(
                self._ctx.config, "app.start", timeout_seconds=self._ctx.config.values["app"]["ready_seconds"],
                stop_file=self._ctx.store.stop_file)
            self._slot_context.__enter__()
            self._exit = process.monitored_run(
                self._argv, cwd=self._source, env=self._env, log_path=self._log,
                budget_seconds=APP_LIFETIME_SECONDS, idle_seconds=APP_LIFETIME_SECONDS,
                stop_file=None, on_start=remember, keep_group=True, nonce=self._nonce,
                on_poll=lambda: (self._touch_while_starting(), stacks.sweep(self._ctx)))
        except Exception as error:  # reported to the caller as StackError
            self._error = error
        finally:
            started.set()

    def _touch_while_starting(self) -> None:
        if self._slot_context is not None and not self._stop_requested.is_set():
            stacks.touch_if_due(self._ctx, self._stack_id)

    def _wait_ready(self) -> None:
        app = self._ctx.config.values["app"]
        deadline = time.monotonic() + app["ready_seconds"]
        while time.monotonic() < deadline:
            self._check_supervisor()
            # Whose the listener is, before the probe asks it and again once it answered.
            if self._listens_alone() and (self._ready is None or self._ready.answers()) and self._listens_alone():
                self._command = process.command_of(self.pid) if self._exit is None else ""
                if self._command:
                    stacks.update_command(self._ctx, self._stack_id, self.pid, self._command)
                return
            time.sleep(READY_POLL_SECONDS)
        where = f"at {self._ready.url}" if self._ready is not None else ""
        if not process.listeners(self.port):
            where = f"with no listener of its own on port {self.port}"
        self._fail(f"app not ready within {app['ready_seconds']} seconds {where}".rstrip())

    def _listens_alone(self) -> bool:
        """Whether the app's port has a listener and every listener on it is a process of the app's own group, read
        from the kernel's tables (never from what answers, nor from a nonce a survivor could carry); False while
        none listens yet. Fails the start on any other listener."""
        listening = process.listeners(self.port)
        if listening is None:
            self._fail(f"cannot read which process listens on port {self.port}")
        if not listening:
            return False
        own = process.group_members(self.pid)
        foreign = [pid for pid in listening if own is None or pid not in own]
        if foreign:
            self._fail(f"port {self.port} is answered by process {', '.join(map(str, foreign))}, not this app's; "
                       "nothing is proved against it")
        return True

    def _check_supervisor(self) -> None:
        if self._error is not None:
            self._fail(f"app supervision failed: {self._error!r}")
        if self._exit is None:
            return
        if self._exit.exit_code != 0:
            self._fail(f"app exited early (exit {self._exit.exit_code})")
        if not self._warned and not self._ctx.config.values["app"]["stop"]:
            self._warned = True
            self._ctx.store.append_ledger("stack", "app.start exited 0 and app.stop is not set: the app may still be running")

    def _fail(self, headline: str) -> NoReturn:
        """Stop whatever is still running of the app, then raise StackError with the log tail."""
        message = self._failure(headline)
        self._command = process.command_of(self.pid) if self.pid is not None else ""
        try:
            self.stop()
        except Exception as error:
            message += f"; stack cleanup failed: {error}"
        raise StackError(message)

    def _failure(self, headline: str) -> str:
        return f"{headline}; log {self._log}:\n{tail(self._log)}"

    def _release_slot(self) -> None:
        if self._slot_context is not None:
            self._slot_context.__exit__(None, None, None)
            self._slot_context = None


def start(ctx: FlowContext, workspace: Workspace, checkout: "completion.Checkout | None" = None) -> AppStack:
    """Start the app of `workspace` on a fresh port of its own session (`AppStack.port`, see the module text) and
    return once it is ready.

    With `checkout` (the completion checkout, `flow/completion.py`) the app runs from that read-only checkout of the
    revision under test, writing only its build folder; the stack is still registered as `workspace`'s.
    Raises StackError (with the log tail) when the port is not free, when it exits non-zero, when another process
    listens on its port, or when it is not ready within `app.ready_seconds`. A start command that exits 0 counts as
    daemonized once its listener is its own; `app.stop` is then the only way to stop it.
    """
    session = _session_port(ctx, workspace)
    try:
        app = _prepared(ctx, workspace, checkout, session)
    except BaseException:
        if session[1]:
            ports.release(session[1])
        raise
    app.launch()
    return app


def _prepared(ctx: FlowContext, workspace: Workspace, checkout: "completion.Checkout | None",
              session: tuple[int, str]) -> AppStack:
    port = session[0]
    stack_id = f"{ctx.store.task_id}:{workspace.name}"
    origin = ctx.config.values["app"]["origin"] or f"http://127.0.0.1:{port}"
    host = urlparse(origin).hostname
    source = checkout.source if checkout else workspace.path
    launch = _launch(ctx, port, source, checkout.build if checkout else None)
    env = {**launch.env, "SAKURA_FLOW_HOST": host or "127.0.0.1", "SAKURA_FLOW_PORT": str(port),
           "SAKURA_FLOW_BASE_URL": origin, "SAKURA_FLOW_STACK": stack_id}
    ready = _ready_probe(ctx, workspace, env, origin, source, checkout.build if checkout else None)
    log = ctx.store.operations_dir / f"app-{workspace.name}.log"
    log.unlink(missing_ok=True)
    return AppStack(ctx, workspace, repository_commands.Launch(launch.argv, env), log, source, ready, session)


def _session_port(ctx: FlowContext, workspace: Workspace) -> tuple[int, str]:
    """The port this session's app listens on and its owner in the port registry ('' for the host's fixed origin):
    a fresh port no owner holds and nothing listens on, or the `app.origin` port once nothing listens there."""
    origin = ctx.config.values["app"]["origin"]
    if origin:
        port = urlparse(origin).port
        if process.listeners(port) != [] or not ports.binds_reusing(port):
            raise StackError(f"port {port} is in use by another process (a server an earlier run left?); the app "
                             "does not start and nothing is proved against it")
        return port, ""
    owner = f"{ctx.repo.name}/{ctx.store.task_id}/{workspace.name}/app-{process.new_nonce()[:SESSION_TOKEN_CHARS]}"
    try:
        return ports.allocate(owner, size=1).start, owner
    except ports.PortsExhausted as error:
        raise StackError(f"no fresh port for the app: {error}") from error


def ready_url(template: str, env: dict[str, str], origin: str) -> str:
    """`app.ready_url` with the run's variables in, refused (StackError) unless its scheme, host and port are exactly
    the app's origin and it carries no user part; '' when none is configured."""
    if not template:
        return ""
    url = string.Template(template).safe_substitute(env)
    parsed, expected = urlparse(url), urlparse(origin)
    try:
        port = parsed.port
    except ValueError:
        port = None
    if (parsed.scheme, parsed.hostname, port) != (expected.scheme, expected.hostname, expected.port) \
            or "@" in parsed.netloc:
        raise StackError(f"app.ready_url must use the app's origin {origin} (only its path is the repository's); "
                         f"got {controls.escaped(url)}")
    return url


def _ready_probe(ctx: FlowContext, workspace: Workspace, env: dict[str, str], origin: str, source: Path,
                 build_dir: Path | None) -> ReadyProbe | None:
    url = ready_url(ctx.config.values["app"]["ready_url"], env, origin)
    if not url or not ctx.config.values["app"]["start"]:
        return None
    try:
        run_cage = checks.checks_cage(ctx.store, source, f"ready-{workspace.name}",
                                      cage.load_policy(ctx.config.values["knobs"]["cage"]), (urlparse(origin).port,),
                                      build_dir)
    except cage.CageError as error:
        raise StackError(f"app readiness probe not caged: {error}") from error
    return ReadyProbe(url, run_cage)


def _launch(ctx: FlowContext, port: int, source: Path, build_dir: Path | None) -> repository_commands.Launch:
    """How `app.start` runs: a repository's one approved and caged, the host's as it is; nothing without `app.start`."""
    start = ctx.config.values["app"]["start"]
    if not start:
        return repository_commands.Launch([], process.child_environment(os.environ))
    try:
        return repository_commands.plan(ctx.store, ctx.config, start, source, "app-start", port, build_dir)
    except (repository_commands.CommandNotApproved, cage.CageError) as error:
        raise StackError(f"app.start not run: {error}") from error
