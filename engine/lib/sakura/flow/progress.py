"""Lane progress: how much of a task's plan and contract the engine's own records show as done.

Total is the plan's build tasks plus the contract's rows. A build task is done when its state is `merged` or it ended
as "needs no change"; a row is done only when its proof holds for this contract and the engine's recorded head
(`completion.row_proven`, the same test delivery applies). Nothing here reads agent text.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from sakura.flow import (
    build_status,
    completion,
    config,
    contract,
    final_head,
    plan,
    store,
)
from sakura.flow.config import PresetSteps


@dataclass(frozen=True)
class Progress:
    """`done` of `total` plan tasks and contract rows, and the phase the task is in. The counts of each kind are kept
    apart for a reader that counts within one step; they are not part of what two progresses compare on."""

    done: int
    total: int
    phase: str
    tasks_done: int = field(default=0, compare=False)
    tasks_total: int = field(default=0, compare=False)
    rows_done: int = field(default=0, compare=False)
    rows_total: int = field(default=0, compare=False)


def of(state: dict, tasks: list[plan.BuildTask], rows: list[contract.Row], steps: PresetSteps,
       root: Path | None) -> Progress:
    """The progress of the task whose state is `state`; `root` is its task folder, for the receipt log check."""
    entries = state.get("build") or {}
    tasks_done = sum(1 for task in tasks if _task_done(entries.get(task.id) or {}))
    digest, head = contract.rows_digest(rows), final_head.recorded(state)
    rows_done = sum(1 for row in rows if completion.row_proven(state, row, steps, digest, head, root))
    return Progress(tasks_done + rows_done, len(tasks) + len(rows), str(state.get("phase") or ""),
                    tasks_done, len(tasks), rows_done, len(rows))


def of_task(task: store.TaskStore, state: dict, cfg: config.FlowConfig | None) -> tuple[Progress, list[str]]:
    """The engine's progress of the task; with its plan or contract unreadable, what is known and the problem.

    Rows and plan tasks count as soon as their files exist, as `flow status` counts them; a digest is checked only
    once the engine has recorded one.
    """
    phase = str(state.get("phase") or "")
    if cfg is None:
        return Progress(0, 0, phase), []
    try:
        tasks = plan.load(task, state.get("plan_sha256") or None) if task.plan_json.exists() else []
        rows = contract.load(task, state.get("contract_sha256") or None) if task.contract.exists() else []
    except (plan.PlanError, contract.ContractError) as error:
        return Progress(0, 0, phase), [f"problem: {error}"]
    return of(state, tasks, rows, config.steps(cfg.values), task.root), []


def _task_done(entry: dict) -> bool:
    if entry.get("status") == build_status.MERGED:
        return True
    return entry.get("status") == build_status.APPROVED and bool(entry.get("no_change"))
