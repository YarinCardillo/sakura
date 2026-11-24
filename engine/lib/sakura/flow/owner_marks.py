"""The person's verdict on an `owner` contract row: the only way such a row turns green or red.

The mark is written by `sakura flow mark` under the task lock, with an evidence note (who, when, the head the
person tried, their note) in `evidence/owner/`, and recorded in the task state with the contract digest and the head
it was given at: delivery takes a mark only for the very contract and head it delivers, so a commit made after the
mark (by a surviving process or anyone) leaves the row waiting for the person again. A mark on a row proven by a
check or the evaluator is refused: the engine flips those from its own evidence. Agent text never reaches this module.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

from sakura.flow import checks, contract, final_head
from sakura.flow.config import PresetSteps
from sakura.flow.gitutil import git
from sakura.flow.store import TaskStore

EVIDENCE_DIR = "owner"
# Requests the cage denied, ingested by the runner after each run (see `flow/cage.py`).
REQUESTS_KEY = "cage_requests"
# Each owner row's mark: verdict, contract digest and head, by row key (`checks.row_key`).
MARKS_KEY = "owner_marks"
VERDICTS = {"ok": True, "ko": False}
MAX_NOTE = 2000
# What the engine says while it waits for the person's marks (`flow/phases/pull_request.py`), and what a blocked task
# says once every one is given.
OWNER_ROWS_REASON = "waiting for the person on owner rows"
ALL_MARKED_REASON = "every owner row is marked; resume the task to deliver"


class MarkRefused(ValueError):
    """Raised when a row may not take the person's mark."""


def mark(store: TaskStore, steps: PresetSteps, row_id: str, verdict: str, note: str, actor: str) -> list[contract.Row]:
    """Flip every viewport of owner row `row_id` to `verdict` (`ok` or `ko`); returns the rows marked."""
    if verdict not in VERDICTS:
        raise MarkRefused(f"verdict must be ok or ko; got {verdict!r}")
    state = store.read_state()
    if not state.get("contract_sha256"):
        raise MarkRefused(f"task {store.task_id} has no saved contract yet")
    rows = [row for row in contract.load(store, state["contract_sha256"]) if row.id == row_id]
    if not rows:
        raise MarkRefused(f"no contract row {row_id} in task {store.task_id}")
    # Every row the mark would set is checked, not only the first: a mixed id never lets a mark reach an engine row.
    engine_proven = sorted({contract.prover_of(row, steps) for row in rows} - {contract.OWNER})
    if engine_proven:
        raise MarkRefused(f"row {row_id} is proven by {', '.join(engine_proven)}, not by the person; "
                          "only owner rows take a mark")
    head = _head(state)
    _require_engine_head(head, state)
    digest = contract.rows_digest(contract.load(store, state["contract_sha256"]))
    evidence = _write_evidence(store, row_id, verdict, note[:MAX_NOTE], actor, head)
    for row in rows:
        contract.set_verdict(store, store.read_state(), row.id, row.viewport, evidence, VERDICTS[verdict])
    recorded = {checks.row_key(row): {"verdict": verdict, "contract": digest, "head": head} for row in rows}
    store.update_state(lambda current: current.setdefault(MARKS_KEY, {}).update(recorded))
    store.append_ledger("owner", f"row {row_id} marked {verdict} by {actor}" + (f": {_one_line(note)}" if note else ""))
    return rows


def holds(state: dict, row: contract.Row, rows_digest: str, head: str) -> bool:
    """True when the person marked `row` ok for this very contract and head."""
    entry = state.get(MARKS_KEY, {}).get(checks.row_key(row))
    return bool(head) and isinstance(entry, dict) and entry.get("verdict") == "ok" \
        and entry.get("contract") == rows_digest and entry.get("head") == head


def forget(store: TaskStore) -> None:
    """Drop every recorded mark: the rows they covered must be marked again."""
    store.update_state(lambda state: state.pop(MARKS_KEY, None))


def open_rows(rows: list[contract.Row], steps: PresetSteps, state: dict) -> list[contract.Row]:
    """The owner rows without an ok mark that delivery accepts: for this contract and the engine's head, as the
    delivery check (`completion.row_proven`) reads it. A mark given at another head leaves its row open (review 17,
    finding 7), so the person is still offered the action that unblocks delivery."""
    digest, head = contract.rows_digest(rows), final_head.recorded(state)
    return [row for row in contract.proven_by(rows, steps, contract.OWNER)
            if not (row.passes and holds(state, row, digest, head))]


def blocked_reason(task_id: str, rows: list[contract.Row]) -> str:
    """The reason a task is blocked on the owner rows `rows`, naming the command that marks each."""
    marks = "; ".join(f"sakura flow mark {task_id} {row.id} ok|ko" for row in rows)
    return f"{OWNER_ROWS_REASON}: {marks}"


def is_blocked_on_owner_rows(state: dict) -> bool:
    """Whether the engine stopped the task to wait for the person's marks."""
    return state.get("status") == "blocked" and str(state.get("reason") or "").startswith(OWNER_ROWS_REASON)


def all_marked(store: TaskStore, state: dict, steps: PresetSteps) -> bool:
    """Whether the task is blocked only on owner rows and none is left open: its next step is a resume."""
    if not (is_blocked_on_owner_rows(state) and state.get("contract_sha256")):
        return False
    try:
        return not open_rows(contract.load(store, state["contract_sha256"]), steps, state)
    except contract.ContractError:
        return False


def waiting(store: TaskStore, steps: PresetSteps) -> list[str]:
    """What the task waits for from the person: open owner rows and the command approval. A request the cage denied is
    not here: there is nothing for the person to answer (see `denials`)."""
    items = []
    state = store.read_state()
    task_id = state.get("task", "")
    if state.get("contract_sha256"):
        try:
            rows = contract.load(store, state["contract_sha256"])
        except contract.ContractError as error:
            rows, items = [], [f"contract unreadable: {error}"]
        for row in open_rows(rows, steps, state):
            items.append(f"owner row {row.id}: {row.flow} (sakura flow mark {task_id} {row.id} ok|ko)")
    approval = checks.waiting_line(state, task_id)
    if approval:
        items.append(approval)
    return items


def denials(state: dict) -> list[str]:
    """What the cage refused an agent, oldest first: information for the task log and the report, not an open item."""
    return [f"denied {request.get('tool', '?')} in run {request.get('run', '?')}: "
            f"{request.get('detail', '')}: {request.get('reason', '')}" for request in state.get(REQUESTS_KEY, [])]


def _write_evidence(store: TaskStore, row_id: str, verdict: str, note: str, actor: str, head: str) -> Path:
    folder = store.evidence_dir / EVIDENCE_DIR
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{row_id}.json"
    record = {"row": row_id, "verdict": verdict, "actor": actor, "note": note, "head": head,
              "at": datetime.now(timezone.utc).isoformat(timespec="seconds")}
    path.write_text(json.dumps(record, indent=2) + "\n")
    return path


def workspace_head(state: dict) -> str:
    """The head workspace 0 is at now ('' without a workspace)."""
    return _head(state)


def _require_engine_head(head: str, state: dict) -> None:
    """Refuse a mark while workspace 0 is not at the engine's head: delivery takes a mark only for that head, so a mark
    given elsewhere would look done and unblock nothing (review 17, finding 7)."""
    engine = final_head.recorded(state)
    if head and engine and head != engine:
        raise MarkRefused(f"workspace 0 is at {head[:12]} but the engine's head is {engine[:12]}: a mark would not count "
                          f"for delivery; run `sakura flow restore-head {state.get('task', '<task>')}` (the cockpit offers it "
                          "beside the row), then mark the row")


def _head(state: dict) -> str:
    """The task branch head the person tried, when workspace 0 exists."""
    workspace = state.get("workspaces", {}).get("0", {}).get("path")
    if not workspace or not Path(workspace).is_dir():
        return ""
    return git(Path(workspace), "rev-parse", "HEAD").strip()


def _one_line(text: str) -> str:
    return " ".join(text.split())[:200]
