"""Honest status reports for Flow tasks that have not reached the final report gate."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

from sakura.flow import contract
from sakura.flow.store import TaskStore


LOCAL_STATUSES = {"awaiting_approval", "blocked", "failed", "stopped"}


def write(task_store: TaskStore) -> str:
    """Atomically write a local status report without changing task state."""
    state = task_store.read_state()
    rows = contract.load(task_store, state["contract_sha256"]) if state.get("contract_sha256") else []
    text = _render(state, rows, task_store.evidence_dir)
    _atomic_write(task_store.local_report, text)
    return text


def _render(state: dict, rows: list[contract.Row], evidence_dir: Path) -> str:
    task = state["task"]
    reason = " ".join((state.get("reason") or "No recovery reason recorded.").split())
    lines = [
        f"# Local status report: {task}",
        "",
        "**NOT READY:** This is a local status snapshot, not the final comprehension report.",
        "",
        f"- Status: {state['status']} / {state['phase']}",
        f"- Reason: {reason}",
        f"- Base: {state.get('base_branch', 'unknown')} at {state.get('base_commit', 'unknown')}",
        f"- Task branch: {state.get('branch', 'unknown')}",
        f"- Pull request: {state.get('pr', {}).get('url') or 'not opened'}",
        "- Final evaluator verdict: not recorded",
        "",
        "## Contract base evidence",
    ]
    lines.extend(_evidence_lines(rows, evidence_dir))
    lines += ["", "## Publication and review", "", "Canonical code review and publication are pending.",
              "This report makes no evaluator-pass or PR-ready claim.", ""]
    return "\n".join(lines)


def _evidence_lines(rows: list[contract.Row], evidence_dir: Path) -> list[str]:
    if not rows:
        return ["No contract rows are available, so no base evidence can be reported."]
    lines = []
    for row in rows:
        label = "baseline" if row.baseline else "issue-specific"
        expected = "pass" if row.baseline else "fail"
        suffix = row.viewport or "api"
        first_line = _first_line(evidence_dir / "contract-base" / f"{row.id}-{suffix}.log")
        lines.append(f"- {row.id} ({label}, expected to {expected} on base): "
                     f"{first_line or 'no base result recorded'}")
    lines.append("Base results describe contract setup only, not final evaluator results.")
    return lines


def _first_line(path: Path) -> str:
    try:
        with path.open() as stream:
            line = stream.readline().strip()
    except OSError:
        return ""
    return line if line.startswith("exit ") and line[5:].isdigit() else ""


def _atomic_write(target: Path, text: str) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{target.name}.", dir=target.parent)
    try:
        with os.fdopen(descriptor, "w") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
