"""Monitored subprocess runs, the process groups the engine makes for them and exact-PID process termination.

Every caged command and role starts in a new session and process group of its own (its PGID is its PID), with a
per-run nonce in its environment (`NONCE_VARIABLE`), and its output reaches the log through a pipe the engine copies.
When the leader exits, its result counts only once the pipe reached EOF and nothing is left of the group or carrying
the nonce (`ps -E` through the pinned `ps`); the group gets SETTLE_SECONDS to go. Anything left is terminated (the
engine's own group, then each exact PID carrying the nonce) and the run ends as `left_running`. On a stall, a timeout,
a stop, an overflow or a replaced log the engine ends the same group and nonce carriers.

The engine signals a group only when it is confirmed as the run's own: while it holds the run's leader unreaped
(the zombie keeps the PID, so nobody else can lead a group of that id), or, in recovery after the leader is gone,
when a live member of the group carries the nonce. `ps -E` cannot read the environment of the system's own binaries
(`/bin/sleep`, `/bin/sh`: measured), so recovery cannot confirm a group they alone are left in; it still ends every
visible carrier.

What a successful command means: it exited, its output reached EOF, and nothing of its process group or carrying its
nonce is left, so the command and every descendant the engine can trace have ended. Not covered, an accepted
same-user risk: a child that leaves the session, clears its environment and closes its output is invisible to those
checks and can outlive the command, still inside its sandbox. It cannot change what is reviewed, proved, delivered or
reported, except on a project whose host config fixes `app.origin` (review 17, finding 3: a survivor of an earlier
session of the task holds the grant to that one port and can answer proof requests while the live application's socket
is handed over; open, the owner decides): no later step reads what an earlier role could write (review 11). The engine merges the exact commits the
reviewers approved into its own head in its own sealed store (`flow/final_head.py`, `flow/sealed.py`) and tests,
receipts, pushes and reports only that head; a reviewer reads a sealed copy in a repository of its own and the
reporter only copies of the sealed inputs in another (`flow/task_repository.py`); no uncaged helper takes its
repository from the workspace (review 10); and the contract's base run and calibration use a read-only checkout of
the sealed base plus the flow bytes the engine read, which is what it commits (review 12); every application starts
on a port no earlier session of the task held and counts only while every listener on it is in its own process group,
checked around every row (reviews 14 and 15); the engine writes into a checkout only through folders it opened without
following a link (review 14); and every writable folder a later step reuses is pinned from creation and refused once
replaced (review 15). A commit it adds
or a branch it moves is ignored. It can change the live workspaces and the task's repository (what a role working
there later sees), the paths its cage writes and the loopback ports it was given, where nothing later listens. The fix is a kernel boundary whose
membership the engine can count, a dedicated macOS user per run (or a VM), not built. Measured, and so not used as a
detection: `sandbox_check` tells a sandboxed process from an unsandboxed one, but about half of a user's processes
are sandboxed apps, and its path form, which would tell this run's policy apart, answers "denied" for every process.
"""

from __future__ import annotations

import contextlib
import os
import secrets
import select
import signal
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Callable, Iterator, Mapping

from sakura import programs, slots
from sakura.flow import live_input, logread
from sakura.flow.config import FlowConfig
from sakura.flow.surface import UNFORWARDED_VARIABLES

OUTPUT_CAP_BYTES = logread.OUTPUT_CAP_BYTES
KILL_GRACE_SECONDS = 5
POLL_SECONDS = 0.1
GAP_SECONDS = 60
CAFFEINATE = "caffeinate"
PS = "ps"
LSOF = "lsof"
LSOF_TIMEOUT_SECONDS = 10
# `lsof` exits 0 when it listed a socket and 1 when there was none to list.
LSOF_FOUND_OR_NONE = (0, 1)
PS_ARGUMENTS = ["-A", "-o", "pid=,ppid=,command="]
# Every process with its group, state and environment (`-E`), at full width.
PS_GROUP_ARGUMENTS = ["-A", "-E", "-ww", "-o", "pid=,pgid=,stat=,command="]
NONCE_VARIABLE = "SAKURA_RUN_NONCE"
NONCE_BYTES = 16
# How long a run's group may take to go after its leader exited (children closing their output, exiting).
SETTLE_SECONDS = 3
# SIGKILL rounds over the group and the nonce carriers, for a run that keeps forking while it is ended.
KILL_ROUNDS = 3
PIPE_CHUNK_BYTES = 1 << 16
# How a step names a run that ended as `left_running`.
LEFT_RUNNING = "left processes running after it exited (the engine terminated them; the run has no result)"
TMUX_TMPDIR = "TMUX_TMPDIR"
PRIVATE_DIR_PREFIX = "sakura-child-"
# What every unattended child may inherit from the engine: nothing else of the engine's environment reaches it.
CHILD_VARIABLES = ("PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "TMPDIR", "USER", "LOGNAME", "SHELL")
# Per provider CLI: the variable that names its account folder (set by `budget.account`), and the engine home's path,
# which the engine's own hooks inside the role need to find the task (a path, not a secret: the cage denies the
# folder, and trust decisions read the account-derived root). No key or token.
PROVIDER_VARIABLES = {"claude": ("CLAUDE_CONFIG_DIR", "SAKURA_ENGINE_HOME"), "codex": ("CODEX_HOME", "SAKURA_ENGINE_HOME")}
_private_tmux_dir: str | None = None
_private_tmux_dir_lock = threading.Lock()


def child_environment(base: Mapping[str, str]) -> dict[str, str]:
    """Return `base` as an unattended child may see it: no tmux identity, and a tmux that reaches no server.

    `TMUX` and `TMUX_PANE` are dropped, and `TMUX_TMPDIR` points at an empty private (0700) directory
    that this process creates on first use and shares with every child, so a bare `tmux` in the child
    finds no socket of the operator's server.
    """
    env = {key: value for key, value in base.items() if key not in UNFORWARDED_VARIABLES}
    env[TMUX_TMPDIR] = _private_tmux_directory()
    return env


def scrubbed_environment(base: Mapping[str, str], provider: str = "") -> dict[str, str]:
    """The environment of a child that runs code the engine does not trust (a role, a repository's command).

    An explicit allow-list: PATH, HOME, LANG, TERM, TMPDIR and the like, the provider's account-folder variable
    when `provider` names one, and the private tmux directory. Nothing else of `base` is passed: no key, token,
    credential path or proxy setting of the engine.
    """
    allowed = (*CHILD_VARIABLES, *PROVIDER_VARIABLES.get(provider, ()))
    env = {key: base[key] for key in allowed if key in base}
    env[TMUX_TMPDIR] = _private_tmux_directory()
    return env


def _private_tmux_directory() -> str:
    global _private_tmux_dir
    with _private_tmux_dir_lock:
        if _private_tmux_dir is None:
            _private_tmux_dir = tempfile.mkdtemp(prefix=PRIVATE_DIR_PREFIX)
        return _private_tmux_dir


@contextmanager
def heavy_slot(config: FlowConfig, label: str, *, task_id: str = "", timeout_seconds: float | None,
               stop_file: Path | None = None) -> Iterator[slots.SlotLease]:
    """Hold one machine-wide slot for a supervised heavy operation."""
    load = config.values["load"]
    total = slots.slot_count(os.environ, os.cpu_count() or 1, load["slots"])
    queue = slots.SlotQueue(slots.default_root(), total)
    critical = task_id in load["critical"] if task_id else False
    with queue.acquire(label, critical, timeout_seconds, stop_file) as lease:
        yield lease


@dataclass(frozen=True)
class ProcessOutcome:
    """How a monitored run ended; `exit_code` is None when the engine killed it.

    `overflowed` means the run wrote more than OUTPUT_CAP_BYTES and was terminated for it; `log_replaced` means
    the log path stopped naming the file the run writes to (moved, replaced or deleted) and it was terminated;
    `left_running` means its leader exited while processes of its group or carrying its nonce, or a holder of its
    output, were still there, and the engine terminated them: the run has no result.
    """

    exit_code: int | None
    stalled: bool
    timed_out: bool
    stopped: bool
    pid: int
    overflowed: bool = False
    log_replaced: bool = False
    left_running: bool = False
    final: str = ""
    reason: str = ""


class TestCommandInterrupted(RuntimeError):
    """A focused test command ended because of STOP, a stall or its elapsed budget."""

    def __init__(self, outcome: ProcessOutcome) -> None:
        super().__init__("focused test command was interrupted")
        self.outcome = outcome


def monitored_run(
    argv: list[str],
    *,
    cwd: Path,
    env: dict,
    log_path: Path,
    budget_seconds: int,
    idle_seconds: int,
    stop_file: Path | None,
    on_start: Callable[[int], None],
    stdin_text: str = "",
    on_gap: Callable[[int], None] | None = None,
    on_poll: Callable[[], None] | None = None,
    nice: int | None = None,
    wall_clock: Callable[[], float] = time.time,
    log_file: BinaryIO | None = None,
    own_group: bool = True,
    keep_group: bool = False,
    nonce: str = "",
    live: "live_input.LiveChannel | None" = None,
) -> ProcessOutcome:
    """Run `argv`, logging stdout and stderr to `log_path`, until it exits or must be killed.

    With `log_file` (an open file the engine made, `repository_commands.scratch_log`) the output goes to that
    descriptor and `log_path` is only watched, never opened.

    Stall means the log size did not change for `idle_seconds`; timeout means
    wall time above `budget_seconds`; stopped means `stop_file` appeared; overflow means the log grew past
    OUTPUT_CAP_BYTES, so a run that never goes idle cannot fill the disk. The size is read from the open
    descriptor, and the run is terminated when `log_path` no longer names that file, so moving or replacing
    the log cannot hide its growth.
    `on_gap(seconds)` is called when two polls are more than GAP_SECONDS apart
    on `wall_clock` (the machine slept).

    `own_group` (every caged command and role): its own session, group and nonce (`nonce`, else a new one), output
    through a pipe, supervised as the module text says. `own_group=False` is for the engine's own uncaged commands (a
    push, `gh`), whose helpers may outlive them by design (an ssh connection kept open). `keep_group` (the app under
    test) writes the log directly and leaves the group to the caller after the leader's exit (`end_group`).

    With `live` (a Claude role, `flow/live_input.py`) the engine keeps the role's stdin: `stdin_text` is written as the
    first stream-json message and the channel closes stdin when the role has answered everything it took up; the
    caller's `on_poll` feeds and observes the channel. Whatever ends the run closes it.
    """
    if log_file is None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
    nonce = (nonce or new_nonce()) if own_group else ""
    # A supervised run's leader is reaped only once its group is ended: while it is an unreaped child its PID, and
    # so its group id, cannot be given to anyone else, which confirms the group as the run's own.
    is_held = own_group and not keep_group
    with contextlib.nullcontext(log_file) if log_file is not None else log_path.open("ab") as log:
        proc, pump = _start(argv, cwd, env, log, nice, nonce, piped=is_held)
        try:
            on_start(proc.pid)
            if live is not None:
                live.attach(proc.stdin, stdin_text)
            else:
                _feed(proc, stdin_text)
            reason = _watch(proc, log, log_path, budget_seconds, idle_seconds, stop_file, on_gap or _ignore_gap,
                            on_poll or _ignore_poll, wall_clock, OUTPUT_CAP_BYTES, is_held)
            if reason:
                _stop_run(proc, nonce, is_held)
            elif is_held:
                reason = _settled(proc.pid, nonce, pump)
            proc.wait()
        except BaseException:
            if nonce:
                end_group(proc.pid, nonce, is_held=is_held and proc.returncode is None)
            _stop_started_process(proc)
            raise
        finally:
            if live is not None:
                live.finish()
            if pump is not None:
                pump.close()
        exit_code = None if reason else proc.returncode
    return ProcessOutcome(exit_code, reason == "stalled", reason == "timed_out", reason == "stopped", proc.pid,
                          reason == "overflowed", reason == "log_replaced", reason == "left_running")


def new_nonce() -> str:
    """A fresh run nonce: what marks every process of one run in its environment."""
    return secrets.token_hex(NONCE_BYTES)


def end_group(pgid: int, nonce: str, is_held: bool = False) -> list[int] | None:
    """Terminate what is left of a run the engine started as group `pgid` with `nonce`: each exact PID carrying the
    nonce and, only when the group is confirmed as the run's own, the whole group; SIGTERM, then SIGKILL after
    KILL_GRACE_SECONDS. Confirmed means the caller holds the run's leader unreaped for the whole call (`is_held`), or
    a live member of the group carries the nonce (`ps -E` cannot read the environment of the system's own
    binaries, such as `/bin/sleep`, so they never count as carriers). Nothing here reaps a child.

    Returns the PIDs found, members of an unconfirmed group included (empty when nothing was left), or None when the
    process table could not be read, and then nothing is signalled: a caller treats both as something left.
    """
    census = _census(pgid, nonce, is_held)
    if census is None or not census.left:
        return None if census is None else []
    _signal_census(pgid, census, signal.SIGTERM)
    deadline = time.monotonic() + KILL_GRACE_SECONDS
    while time.monotonic() < deadline:
        current = _census(pgid, nonce, is_held)
        if current is None or not current.targets:
            break
        time.sleep(POLL_SECONDS)
    for _ in range(KILL_ROUNDS):
        current = _census(pgid, nonce, is_held)
        if current is None or not current.targets:
            break
        _signal_census(pgid, current, signal.SIGKILL)
        time.sleep(POLL_SECONDS)
    return census.left


def _stop_run(proc: subprocess.Popen, nonce: str, is_held: bool) -> None:
    """End a run the engine stopped (stall, timeout, stop, overflow, replaced log). A held run: its whole group, with
    the leader still unreaped, then the descendants that left the group, by exact PID; otherwise its process tree,
    then the nonce carriers."""
    if not is_held:
        kill_tree(proc.pid, command_of(proc.pid))
        proc.wait()
        if nonce:
            end_group(proc.pid, nonce)
        return
    descendants = [pid for pid in tree_of(proc.pid) if pid != proc.pid]
    end_group(proc.pid, nonce, is_held=True)
    _end_exact(descendants)


def _end_exact(pids: list[int]) -> None:
    """SIGTERM each of `pids` still alive (none of them a child of this process), SIGKILL after KILL_GRACE_SECONDS."""
    _signal_all([pid for pid in pids if is_alive(pid)], signal.SIGTERM)
    deadline = time.monotonic() + KILL_GRACE_SECONDS
    while time.monotonic() < deadline and any(is_alive(pid) for pid in pids):
        time.sleep(POLL_SECONDS)
    _signal_all([pid for pid in pids if is_alive(pid)], signal.SIGKILL)


def _start(argv: list[str], cwd: Path, env: dict, log, nice: int | None, nonce: str,
           piped: bool) -> tuple[subprocess.Popen, "_Pump | None"]:
    """Start `argv`: with a nonce in a new session (so a group) of its own, carrying it; `piped`: output through a
    pipe the engine copies into `log`, whose EOF says that nothing holds it any more."""
    preexec_fn = (lambda: os.nice(nice)) if nice is not None and hasattr(os, "nice") else None
    options = dict(cwd=cwd, env={**env, NONCE_VARIABLE: nonce} if nonce else env, stdin=subprocess.PIPE,
                   stderr=subprocess.STDOUT, preexec_fn=preexec_fn, start_new_session=bool(nonce))
    if not piped:
        return subprocess.Popen(argv, stdout=log, **options), None
    source, sink = os.pipe()
    try:
        proc = subprocess.Popen(argv, stdout=sink, **options)
    except BaseException:
        os.close(source)
        raise
    finally:
        os.close(sink)
    return proc, _Pump(source, log.fileno(), OUTPUT_CAP_BYTES)


def _settled(pgid: int, nonce: str, pump: "_Pump") -> str:
    """How a held run ends after its leader exited by itself (still unreaped): `left_running` when its group, a nonce
    carrier or a holder of its output was still there after SETTLE_SECONDS (those processes are terminated first),
    else ''."""
    deadline = time.monotonic() + SETTLE_SECONDS
    while time.monotonic() < deadline:
        census = _census(pgid, nonce, is_held=True)
        if pump.eof.is_set() and census is not None and not census.left:
            break
        time.sleep(POLL_SECONDS)
    left = end_group(pgid, nonce, is_held=True)
    if left != []:
        pump.eof.wait(KILL_GRACE_SECONDS)  # the copy takes in what the ended processes wrote
        return "left_running"
    if not pump.eof.is_set():
        return "left_running"  # a process outside the group and without the nonce still holds the output
    return ""


def _signal_census(pgid: int, census: "_Census", sig: signal.Signals) -> None:
    """Signal the nonce carriers, and the group only when the census confirmed it as the run's own."""
    if census.is_confirmed and pgid > 1 and pgid != os.getpgrp():
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(pgid, sig)
    _signal_all(census.carriers, sig)


@dataclass(frozen=True)
class _Census:
    """What is left of one run: the live processes carrying its nonce, the live members of its group, and whether the
    group is confirmed as the run's own: the engine holds the run's leader unreaped, or a member carries the nonce
    (a group id is not given out again while the group has a member)."""

    carriers: tuple[int, ...]
    members: tuple[int, ...]
    is_confirmed: bool

    @property
    def left(self) -> list[int]:
        """Everything that counts as left of the run, unconfirmed members included (the step fails on them)."""
        return sorted({*self.carriers, *self.members})

    @property
    def targets(self) -> list[int]:
        """What the engine may signal: the carriers, and the members of a confirmed group."""
        return sorted({*self.carriers, *(self.members if self.is_confirmed else ())})


def _census(pgid: int, nonce: str, is_held: bool, timeout: float | None = None) -> _Census | None:
    """The census of group `pgid` and nonce `nonce` from one `ps -E` through the pinned `ps` (zombies and this process
    left out); None when the table cannot be read.

    Unless the leader is held, it is gone: a live process whose PID is the group id and that shows no nonce means the
    id may have been given out again after the group emptied, and its members are not counted as the run's."""
    executable = programs.path(PS)
    options = {} if timeout is None else {"timeout": timeout}
    done = subprocess.run([executable, *PS_GROUP_ARGUMENTS], capture_output=True, text=True, errors="replace",
                          check=False, stdin=subprocess.DEVNULL, env=programs.environment("system", executable), **options)
    if done.returncode:
        return None
    mark = f"{NONCE_VARIABLE}={nonce}"
    rows = []
    for line in done.stdout.splitlines():
        parts = line.split(None, 3)
        if len(parts) < 3 or not (parts[0].isdigit() and parts[1].isdigit()) or parts[2].startswith("Z"):
            continue
        if int(parts[0]) != os.getpid():
            rows.append((int(parts[0]), int(parts[1]), len(parts) == 4 and mark in parts[3]))
    reused = not is_held and any(pid == pgid and not marked for pid, _, marked in rows)
    members = () if reused else tuple(pid for pid, group, _ in rows if group == pgid)
    carriers = tuple(pid for pid, _, marked in rows if marked)
    return _Census(carriers, members, is_held or (not reused and any(pid in carriers for pid in members)))


class _Pump:
    """Copies a run's output pipe into its log until every writer closed it (EOF). At most `cap + 1` bytes reach the
    log, so the watch stops a flood before it fills the disk; the rest is read and dropped."""

    def __init__(self, source: int, log_descriptor: int, cap: int) -> None:
        self.eof = threading.Event()
        self._source, self._log, self._room = source, log_descriptor, cap + 1
        self._closing = threading.Event()
        self._thread = threading.Thread(target=self._copy, daemon=True)
        self._thread.start()

    def close(self) -> None:
        """Stop copying and close the engine's end: whatever still holds the pipe gets an error on its next write."""
        self._closing.set()
        self._thread.join()
        os.close(self._source)

    def _copy(self) -> None:
        while not self._closing.is_set():
            ready, _, _ = select.select([self._source], [], [], POLL_SECONDS)
            if not ready:
                continue
            chunk = os.read(self._source, PIPE_CHUNK_BYTES)
            if not chunk:
                self.eof.set()
                return
            kept = chunk[:max(self._room, 0)]
            self._room -= len(kept)
            while kept:
                kept = kept[os.write(self._log, kept):]


def tracked_run(ctx, argv: list[str], *, cwd: Path, env: dict, log_path: Path, budget_seconds: int,
                idle_seconds: int, stop_file: Path | None, on_poll: Callable[[], None] | None = None,
                log_file: BinaryIO | None = None) -> ProcessOutcome:
    """Run an engine-owned child with bounded supervision and restart recovery identity: its PID, command line,
    process group and nonce are recorded while it runs."""
    started: list[int] = []
    nonce = new_nonce()

    def record(pid: int) -> None:
        started.append(pid)
        _record_active(ctx.store, pid, nonce)

    try:
        return monitored_run(argv, cwd=cwd, env=env, log_path=log_path, budget_seconds=budget_seconds,
                             idle_seconds=idle_seconds, stop_file=stop_file, on_start=record, on_poll=on_poll,
                             log_file=log_file, nonce=nonce)
    finally:
        for pid in started:
            _clear_active(ctx.store, pid)


def external_tracked_run(task_store, argv: list[str], *, cwd: Path, env: dict, log_path: Path,
                         budget_seconds: int, idle_seconds: int, stop_file: Path | None,
                         on_poll: Callable[[], None] | None = None, log_file: BinaryIO | None = None,
                         own_group: bool = True) -> ProcessOutcome:
    """Run a non-engine child with bounded supervision and separate recovery identity (`own_group` as in
    `monitored_run`): its PID and command line, and with its own group that group and its nonce."""
    registered: list[tuple[int, str]] = []
    nonce = new_nonce() if own_group else ""

    def register(pid: int) -> None:
        command = command_of(pid)
        if not command:
            raise RuntimeError(f"cannot register external child {pid}: command line is unavailable")
        task_store.register_external_run(pid, command, pid if nonce else 0, nonce)
        registered.append((pid, command))

    try:
        return monitored_run(argv, cwd=cwd, env=env, log_path=log_path, budget_seconds=budget_seconds,
                             idle_seconds=idle_seconds, stop_file=stop_file, on_start=register, on_poll=on_poll,
                             log_file=log_file, own_group=own_group, nonce=nonce)
    finally:
        for pid, command in registered:
            task_store.remove_external_run(pid, command)


def _stop_started_process(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    command = command_of(proc.pid)
    if not command or not kill_tree(proc.pid, command):
        proc.terminate()
    try:
        proc.wait(timeout=KILL_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def run_test_command(ctx, command: str, cwd: Path, *, budget_seconds: int, idle_seconds: int) -> tuple[int, str]:
    """Run one checkout test command with flow STOP, timeout and stale-child tracking."""
    log_path = ctx.store.operations_dir / f"focused-{time.time_ns()}.raw"
    log_path.unlink(missing_ok=True)
    from sakura.flow import repository_commands  # imported here: it builds on this module

    launch = repository_commands.plan(ctx.store, ctx.config, command, cwd, "focused")
    scratch = repository_commands.scratch_log(launch.env, log_path)
    try:
        outcome = tracked_run(ctx, launch.argv, cwd=cwd, env=launch.env, log_path=scratch.path,
                              budget_seconds=budget_seconds, idle_seconds=idle_seconds,
                              stop_file=ctx.store.stop_file, log_file=scratch.file)
    finally:
        repository_commands.keep_log(scratch, log_path)
    output = _read_back(log_path)
    print(output, end="", flush=True)
    if (outcome.stopped or outcome.stalled or outcome.timed_out or outcome.overflowed or outcome.log_replaced
            or outcome.left_running):
        raise TestCommandInterrupted(outcome)
    return outcome.exit_code, output


def _read_back(log_path: Path) -> str:
    try:
        return logread.read_log(log_path)
    except FileNotFoundError:
        return ""


def _record_active(store, pid: int, nonce: str) -> None:
    active = {"pid": pid, "command": command_of(pid), "pgid": pid, "nonce": nonce}

    def update(state: dict) -> None:
        state["active_runs"] = [*state.get("active_runs", []), active]
        state["active_run"] = active

    store.update_state(update)


def _clear_active(store, pid: int) -> None:
    def update(state: dict) -> None:
        remaining = [run for run in state.get("active_runs", []) if run["pid"] != pid]
        state["active_runs"] = remaining
        state["active_run"] = remaining[-1] if remaining else None

    store.update_state(update)


def command_of(pid: int) -> str:
    """Return the command line of `pid`, or an empty string when it does not exist."""
    return _process_table().get(pid, (0, ""))[1]


def is_alive(pid: int) -> bool:
    """Return whether `pid` is a running process; reaps it when it is our zombie child."""
    try:
        reaped, _ = os.waitpid(pid, os.WNOHANG)
        return reaped == 0
    except ChildProcessError:
        pass
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def tree_of(pid: int) -> list[int]:
    """`pid` and every process below it, children first."""
    return _descendants_first(pid, _process_table())


def kill_tree(pid: int, expected_command: str) -> bool:
    """Terminate `pid` and its descendants, children first, each by exact PID.

    Refuses (returns False) when the PID's command line is not exactly
    `expected_command`, which guards against a recycled PID. Survivors are
    killed with SIGKILL after KILL_GRACE_SECONDS.
    """
    table = _process_table()
    if pid not in table or not expected_command or table[pid][1] != expected_command:
        return False
    victims = _descendants_first(pid, table)
    _signal_all(victims, signal.SIGTERM)
    deadline = time.monotonic() + KILL_GRACE_SECONDS
    while time.monotonic() < deadline and any(is_alive(victim) for victim in victims):
        time.sleep(POLL_SECONDS)
    _signal_all([victim for victim in victims if is_alive(victim)], signal.SIGKILL)
    return True


def _feed(proc: subprocess.Popen, text: str) -> None:
    def write() -> None:
        try:
            proc.stdin.write(text.encode())
            proc.stdin.close()
        except BrokenPipeError:
            pass

    threading.Thread(target=write, daemon=True).start()


def _watch(proc: subprocess.Popen, log, log_path: Path, budget: int, idle: int, stop_file: Path | None,
           on_gap: Callable[[int], None], on_poll: Callable[[], None], wall_clock: Callable[[], float],
           output_cap: int, is_held: bool = False) -> str:
    started = last_change = time.monotonic()
    last_size, last_wall = -1, wall_clock()
    while not _has_exited(proc, is_held):
        on_poll()
        now = time.monotonic()
        last_wall = _report_gap(last_wall, wall_clock(), on_gap)
        info = os.fstat(log.fileno())
        if not _names(log_path, info):
            return "log_replaced"
        size = info.st_size
        if size != last_size:
            last_size, last_change = size, now
        if size > output_cap:
            return "overflowed"
        if stop_file is not None and stop_file.exists():
            return "stopped"
        if now - started > budget:
            return "timed_out"
        if now - last_change >= idle:
            return "stalled"
        time.sleep(POLL_SECONDS)
    return ""


def _has_exited(proc: subprocess.Popen, is_held: bool) -> bool:
    """Whether the run's leader exited; a held leader is left unreaped (its zombie keeps the PID, so the group id)."""
    if not is_held:
        return proc.poll() is not None
    try:
        return os.waitid(os.P_PID, proc.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
    except ChildProcessError:
        return True


def _names(path: Path, info: os.stat_result) -> bool:
    """Whether `path` still names the file `info` describes."""
    try:
        current = os.stat(path, follow_symlinks=False)
    except FileNotFoundError:
        return False
    return (current.st_dev, current.st_ino) == (info.st_dev, info.st_ino)


def _ignore_gap(seconds: int) -> None:
    return None


def _ignore_poll() -> None:
    return None


def _report_gap(previous: float, current: float, on_gap: Callable[[int], None]) -> float:
    if current - previous > GAP_SECONDS:
        on_gap(int(current - previous))
    return current


@contextlib.contextmanager
def hold_awake(engine_pid: int) -> Iterator[int | None]:
    """Keep macOS awake while `engine_pid` lives; yields the caffeinate PID, or None when unavailable.

    The assertion process is killed by its exact PID on exit, normal or not.
    """
    executable = _pinned(CAFFEINATE) if sys.platform == "darwin" else None
    if executable is None:
        yield None
        return
    proc = subprocess.Popen([executable, "-i", "-w", str(engine_pid)], stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            env=programs.environment("system", executable))
    try:
        yield proc.pid
    finally:
        kill_tree(proc.pid, command_of(proc.pid))
        proc.wait()


def listeners(port: int) -> list[int] | None:
    """The PIDs listening on TCP `port`, read from the kernel's socket table through the pinned `lsof` (never from
    what answers on the port); None when the table cannot be read."""
    executable = programs.path(LSOF)
    try:
        done = subprocess.run([executable, "-nP", "-a", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fp"], capture_output=True,
                              text=True, check=False, stdin=subprocess.DEVNULL, timeout=LSOF_TIMEOUT_SECONDS,
                              env=programs.environment("system", executable))
    except subprocess.TimeoutExpired:
        return None
    if done.returncode not in LSOF_FOUND_OR_NONE:
        return None
    return sorted({int(line[1:]) for line in done.stdout.splitlines() if line.startswith("p") and line[1:].isdigit()})


def group_members(pgid: int) -> set[int] | None:
    """The live members of process group `pgid`, read from the kernel's process table through the pinned `ps` (the
    group of a process, never its command or environment); None when the table cannot be read."""
    executable = programs.path(PS)
    done = subprocess.run([executable, *PS_GROUP_ARGUMENTS], capture_output=True, text=True, errors="replace",
                          check=False, stdin=subprocess.DEVNULL, env=programs.environment("system", executable))
    if done.returncode:
        return None
    members = set()
    for line in done.stdout.splitlines():
        parts = line.split(None, 3)
        if len(parts) >= 3 and parts[0].isdigit() and parts[1].isdigit() and not parts[2].startswith("Z") \
                and int(parts[1]) == pgid:
            members.add(int(parts[0]))
    return members


def listener_problem(pgid: int, port: int) -> str:
    """Why the listeners on TCP `port` are not exactly processes of group `pgid` (review 15: the kernel's group, not
    the nonce, says whose a listener is), or ''."""
    listening = listeners(port)
    if listening is None:
        return f"cannot read which process listens on port {port}"
    if not listening:
        return f"nothing listens on port {port}"
    own = group_members(pgid)
    foreign = [pid for pid in listening if own is None or pid not in own]
    if foreign:
        return f"port {port} is answered by process {', '.join(map(str, foreign))}, not the app's own"
    return ""


def _pinned(name: str) -> str | None:
    """The engine's pinned copy of `name`, or None when there is none (the helper is then skipped)."""
    try:
        return programs.path(name)
    except programs.ProgramError:
        return None


def _process_table() -> dict[int, tuple[int, str]]:
    executable = programs.path(PS)
    done = subprocess.run([executable, *PS_ARGUMENTS], capture_output=True, text=True, check=False,
                          env=programs.environment("system", executable))
    table = {}
    for line in done.stdout.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit():
            table[int(parts[0])] = (int(parts[1]), parts[2])
    return table


def _descendants_first(root: int, table: dict[int, tuple[int, str]]) -> list[int]:
    order, frontier = [root], [root]
    while frontier:
        frontier = [pid for pid, (parent, _) in table.items() if parent in frontier]
        order.extend(frontier)
    return list(reversed(order))


def _signal_all(pids: list[int], sig: signal.Signals) -> None:
    for pid in pids:
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            continue


def acp_alive(pid: int, pgid: int, nonce: str) -> bool:
    """Check an ACP leader by ownership nonce because exec can change its command line."""
    if type(pid) is not int or pid <= 0 or pid != pgid or not isinstance(nonce, str) or not nonce:
        return False
    try:
        census = _census(pgid, nonce, is_held=False, timeout=2)
    except (OSError, programs.ProgramError, subprocess.TimeoutExpired):
        return False
    return census is not None and pid in census.carriers


def close_acp_child(child: subprocess.Popen, nonce: str, grace: float) -> bool:
    """Close ACP pipes and bound teardown of the unreaped group and its nonce carriers."""
    with contextlib.suppress(OSError):
        child.stdin.close()
    deadline = time.monotonic() + grace
    while time.monotonic() < deadline and not _has_exited(child, True):
        time.sleep(min(POLL_SECONDS, max(0, deadline - time.monotonic())))
    exited = _has_exited(child, True)
    try:
        remaining = None
        with contextlib.suppress(OSError, programs.ProgramError, subprocess.TimeoutExpired):
            remaining = _census(child.pid, nonce, is_held=True, timeout=2)
        detached = remaining is None or any(pid != child.pid for pid in remaining.left)
        if remaining is not None and remaining.targets:
            _signal_census(child.pid, remaining, signal.SIGKILL)
        return detached or not exited
    finally:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=2)
        child.stdout.close()
        child.stderr.close()
