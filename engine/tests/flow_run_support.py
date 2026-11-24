"""Builders for tests of what a run's log says: the lines Claude and Codex write, and recorded runs to hold them."""

from __future__ import annotations

import json
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path

from sakura.flow import process, run_records, run_times, store

SESSION = "9b5c773d-e4a1-4311-bca3-43830f5ff541"
INIT = json.dumps({"type": "system", "subtype": "init", "session_id": SESSION}) + "\n"
START = datetime(2026, 11, 19, 9, 30, 0, tzinfo=timezone.utc)
LIVE_IDENTITY = {"pid": os.getpid(), "command": process.command_of(os.getpid())}  # a process that exists


def iso(seconds: float = 0) -> str:
    """The text of the moment `seconds` after START, as the engine writes times."""
    return (START + timedelta(seconds=seconds)).isoformat(timespec="seconds")


def make_task(repo: Path, tmp_path: Path) -> store.TaskStore:
    """A task store with one recorded workspace outside the repository (`task.workspace`)."""
    workspace = tmp_path / "workspace-0"
    workspace.mkdir()
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo", "status": "running", "workspaces": {"0": {"path": str(workspace)}}})
    task_store.workspace = workspace
    return task_store


def assistant(*blocks) -> str:
    return json.dumps({"type": "assistant", "message": {"role": "assistant", "content": list(blocks)}}) + "\n"


def text(words: str) -> dict:
    return {"type": "text", "text": words}


def tool_use(tool_id: str, name: str = "Bash", **arguments) -> dict:
    return {"type": "tool_use", "id": tool_id, "name": name, "input": arguments}


def tool_result(tool_id: str) -> str:
    return json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tool_id,
                                                                 "content": "ok"}]}}) + "\n"


def message_start() -> str:
    return json.dumps({"type": "stream_event", "parent_tool_use_id": None,
                       "event": {"type": "message_start", "message": {"role": "assistant"}}}) + "\n"


def delta(words: str) -> str:
    return json.dumps({"type": "stream_event", "parent_tool_use_id": None,
                       "event": {"type": "content_block_delta", "index": 0,
                                 "delta": {"type": "text_delta", "text": words}}}) + "\n"


def you(words: str) -> str:
    return json.dumps({"type": "user", "message": {"role": "user", "content": words}}) + "\n"


def codex_item(kind: str, item_type: str, item_id: str = "i1", **fields) -> str:
    return json.dumps({"type": kind, "item": {"id": item_id, "type": item_type, **fields}}) + "\n"


def add_run(task: store.TaskStore, name: str = "build-1", log: str = "", provider: str = "claude",
            role: str = "builder", state: str = "finished", account: str | None = None, resumes: str | None = None,
            cwd: Path | None = None, account_ref: dict | None = None, transcript_root: str | None = None,
            phase: str = "build", env: dict | None = None, step: str = "") -> Path:
    """Record run `name` in `state` (starting, running, finished or aborted) with `log` as its log; returns the log's path."""
    run_records.begin(task, name, role, phase, provider, "sonnet", "low", "local", cwd or task.workspace, env or {},
                      account=account, resumes=resumes, account_ref=account_ref, stored_in=transcript_root, step=step)
    log_path = run_records.log_path(task, name)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text(log)
    if state != "starting":
        run_records.mark_running(task, name, LIVE_IDENTITY)
    if state in ("finished", "aborted"):
        run_records.finish(task, name, 0, aborted=state == "aborted")
    return log_path


def append(log_path: Path, data: str, seconds: float | None = None) -> None:
    """Add `data` to a log and, with `seconds`, mark that the log reached its new size that long after START."""
    with log_path.open("ab") as stream:
        stream.write(data.encode())
    if seconds is not None:
        run_times.record(log_path, log_path.stat().st_size, START + timedelta(seconds=seconds))
