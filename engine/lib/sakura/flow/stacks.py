"""Track and stop application stacks after their configured idle window."""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import os
import time
from pathlib import Path
from typing import Iterator

from sakura.flow import process, repository_commands

MAX_TOUCH_INTERVAL_SECONDS = 30


@contextlib.contextmanager
def lifecycle_lock(store, stack_id: str) -> Iterator[None]:
    """Serialize touches and stop decisions for one stack across processes."""
    lock_dir = store.root / "stack-locks"
    lock_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    lock_name = hashlib.sha256(stack_id.encode()).hexdigest() + ".lock"
    descriptor = os.open(lock_dir / lock_name, os.O_CREAT | os.O_RDWR, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(descriptor, fcntl.LOCK_UN)
        os.close(descriptor)


def register(ctx: FlowContext, stack_id: str, pid: int, command: str, workspace: Path, log: Path,
             port: int, nonce: str = "") -> None:
    """Record a started app process, the nonce of its group and the paths needed to stop it later."""
    entry = {"pid": pid, "command": command, "workspace": str(workspace), "log": str(log),
             "port": port, "last_used": time.time(), "nonce": nonce}

    def update(state: dict) -> None:
        state.setdefault("stacks", {})[stack_id] = entry
        active = state.setdefault("active_stacks", [])
        if not any(item.get("pid") == pid for item in active):
            active.append({"pid": pid, "command": command})

    with lifecycle_lock(ctx.store, stack_id):
        ctx.store.update_state(update)


def update_command(ctx: FlowContext, stack_id: str, pid: int, command: str) -> None:
    """Record the command line `pid` runs now, for a started app that replaced its own image (the framework Python's
    `bin/python3` starts `Python.app` in its place): stopping it compares the command it runs with this one."""
    def update(state: dict) -> None:
        entry = state.get("stacks", {}).get(stack_id)
        if entry is not None and entry.get("pid") == pid:
            entry["command"] = command
        for item in state.get("active_stacks", []):
            if item.get("pid") == pid:
                item["command"] = command

    with lifecycle_lock(ctx.store, stack_id):
        ctx.store.update_state(update)


def touch(ctx: FlowContext, stack_id: str) -> None:
    """Refresh a registered stack's last-used time."""
    touch_store(ctx.store, stack_id)


def touch_store(store, stack_id: str) -> None:
    """Refresh a registered stack through its task store."""
    def update(state: dict) -> None:
        if stack_id in state.get("stacks", {}):
            state["stacks"][stack_id]["last_used"] = time.time()

    with lifecycle_lock(store, stack_id):
        store.update_state(update)


def touch_if_due(ctx: FlowContext, stack_id: str) -> None:
    """Refresh a stack at least twice per idle window without writing on every poll."""
    entry = ctx.store.read_state().get("stacks", {}).get(stack_id)
    if not entry:
        return
    interval = min(MAX_TOUCH_INTERVAL_SECONDS, max(0.2, ctx.config.values["load"]["idle_stack_seconds"] / 2))
    if time.time() - entry["last_used"] >= interval:
        touch(ctx, stack_id)


def forget(ctx: FlowContext, stack_id: str) -> None:
    """Remove a stopped stack from engine state without deleting its files."""

    def update(state: dict) -> None:
        entry = state.get("stacks", {}).pop(stack_id, None)
        if entry:
            state["active_stacks"] = [item for item in state.get("active_stacks", [])
                                      if item.get("pid") != entry["pid"]]

    with lifecycle_lock(ctx.store, stack_id):
        _forget_locked(ctx.store, stack_id, update)


def _forget_locked(store, stack_id: str, update=None) -> None:
    if update is None:
        def update(state: dict) -> None:
            entry = state.get("stacks", {}).pop(stack_id, None)
            if entry:
                state["active_stacks"] = [item for item in state.get("active_stacks", [])
                                          if item.get("pid") != entry["pid"]]
    store.update_state(update)


def sweep(ctx: FlowContext, now: float | None = None) -> list[str]:
    """Stop stacks idle beyond the configured threshold, preserving their worktrees and logs."""
    return sweep_store(ctx.store, ctx.config, now)


def sweep_store(store, config, now: float | None = None) -> list[str]:
    """Sweep a task's stacks using its store and effective configuration."""
    current = time.time() if now is None else now
    cutoff = config.values["load"]["idle_stack_seconds"]
    stopped = []
    for stack_id, entry in list(store.read_state().get("stacks", {}).items()):
        with lifecycle_lock(store, stack_id):
            current_entry = store.read_state().get("stacks", {}).get(stack_id)
            if current_entry != entry:
                continue
            idle = current - current_entry["last_used"]
            if idle <= cutoff:
                continue
            _stop_entry(store, config, stack_id, current_entry)
            _forget_locked(store, stack_id)
            store.append_ledger("stack", f"stack {stack_id} stopped after {int(idle)} s idle")
            stopped.append(stack_id)
    return stopped


def stop_registered(ctx: FlowContext, stack_id: str) -> bool:
    """Stop one registered stack and remove it only after shutdown succeeds."""
    with lifecycle_lock(ctx.store, stack_id):
        entry = ctx.store.read_state().get("stacks", {}).get(stack_id)
        if entry is None:
            return False
        _stop_entry(ctx.store, ctx.config, stack_id, entry)
        _forget_locked(ctx.store, stack_id)
        return True


def _stop_entry(store, config, stack_id: str, entry: dict) -> None:
    stop = config.values["app"]["stop"]
    if stop:
        app_log = Path(entry["log"])
        stop_log = app_log.with_suffix(".stop.log")
        if app_log.parent.name == "runs":
            stop_log = app_log.parent / "operations" / f"{app_log.stem}.stop.log"
        launch = repository_commands.plan(store, config, stop, Path(entry["workspace"]), "app-stop")
        env = {**launch.env, "SAKURA_FLOW_STACK": stack_id}
        if "port" in entry:
            env["SAKURA_FLOW_PORT"] = str(entry["port"])
        outcome = process.monitored_run(launch.argv, cwd=Path(entry["workspace"]), env=env,
                                        log_path=stop_log, budget_seconds=120,
                                        idle_seconds=120, stop_file=None, on_start=lambda pid: None)
        if outcome.exit_code != 0 or outcome.stalled or outcome.timed_out or outcome.stopped:
            raise RuntimeError(f"app.stop failed for stack {stack_id} (exit {outcome.exit_code})")
    pid = int(entry["pid"])
    if process.is_alive(pid) and not process.kill_tree(pid, str(entry["command"])):
        raise RuntimeError(f"stack process {pid} no longer matches its recorded command")
    if entry.get("nonce"):
        process.end_group(pid, entry["nonce"])
