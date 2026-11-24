"""Build plan model: tasks read from plan.json, validated against the contract rows."""

from __future__ import annotations

import hashlib
import json
from collections import Counter
from dataclasses import dataclass

from sakura.budget import routing
from sakura.flow.contract import Row
from sakura.flow.store import TaskStore

SEAM_NEIGHBOUR_COUNT = 2
GOAL_TASK_ID = "T1"
WHOLE_REPOSITORY = "."
GOAL_TITLE = "Build the goal of the lane"
GOAL_TITLE_LIMIT = 100


class PlanError(ValueError):
    """Raised when plan.json is missing or malformed."""


class PlanTampered(PlanError):
    """Raised when the plan files no longer match the digest the engine bound at approval."""


@dataclass(frozen=True)
class BuildTask:
    """One unit of build work; `seam` marks a task that joins the output of other tasks.

    `task_class` (see `sakura.budget.routing`) picks the builder's model; a plan written before classes existed is `integration`.
    """

    id: str
    title: str
    depends_on: list[str]
    rows: list[str]
    files: list[str]
    seam: bool
    owner_line: str = ""
    task_class: str = routing.DEFAULT_TASK_CLASS
    class_reason: str = ""


def digest(store: TaskStore) -> str:
    """Return the sha256 over the bytes of plan.json and plan.md."""
    material = store.plan_json.read_bytes() + b"\0" + store.plan_md.read_bytes()
    return hashlib.sha256(material).hexdigest()


def load(store: TaskStore, expected_sha256: str | None) -> list[BuildTask]:
    """Read plan.json; raises PlanTampered when the files differ from `expected_sha256`, PlanError when malformed."""
    try:
        if expected_sha256 is not None and digest(store) != expected_sha256:
            raise PlanTampered("plan.md or plan.json changed after the plan was approved")
    except OSError as error:
        raise PlanError(f"plan files cannot be read: {error}") from error
    try:
        entries = json.loads(store.plan_json.read_text())["tasks"]
        return [_parse_task(entry) for entry in entries]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise PlanError(f"plan.json is malformed: {error!r}") from error


def write_goal_plan(store: TaskStore, rows: list[Row], spec_text: str) -> None:
    """Write the plan of a lane whose plan step is off: one task that builds the whole goal and owns every row.

    The task is named after the first line of the spec; a spec with no words names it `GOAL_TITLE`.
    """
    title = next((line.strip().lstrip("#").strip() for line in spec_text.splitlines() if line.strip().lstrip("#").strip()),
                 GOAL_TITLE)[:GOAL_TITLE_LIMIT]
    owned = [row.id for row in rows if not row.baseline]
    task = {"id": GOAL_TASK_ID, "title": title, "depends_on": [], "rows": owned, "files": ["."], "seam": False}
    store.plan_json.write_text(json.dumps({"tasks": [task]}))
    store.plan_md.write_text(f"# Plan\n\nThe plan step is off: one task builds the goal.\n\n- {GOAL_TASK_ID}: {title}\n")


def validate(tasks: list[BuildTask], rows: list[Row]) -> list[str]:
    """Return every reason the tasks are not an acceptable plan; empty means valid."""
    known = {task.id for task in tasks}
    errors = _duplicate_ids(tasks)
    for task in tasks:
        errors.extend(_task_problems(task, known, {row.id for row in rows}))
    errors.extend(_uncovered_rows(tasks, rows))
    errors.extend(_seam_problems(tasks))
    return errors + _cycle_errors(tasks)


def order(tasks: list[BuildTask]) -> list[str]:
    """Task ids in plan order, each after its dependencies; tasks in a cycle are left out.

    Dependencies on ids that are not tasks are ignored here; `validate` reports them.
    """
    known = {task.id for task in tasks}
    done: list[str] = []
    pending = list(tasks)
    while True:
        ready = next((task for task in pending if all(dep in done or dep not in known for dep in task.depends_on)), None)
        if ready is None:
            return done
        done.append(ready.id)
        pending.remove(ready)


def _parse_task(entry: dict) -> BuildTask:
    task = BuildTask(entry["id"], entry["title"], entry["depends_on"], entry["rows"], entry["files"], entry["seam"],
                     entry.get("owner_line", ""), entry.get("task_class", routing.DEFAULT_TASK_CLASS),
                     entry.get("class_reason", ""))
    lists = (task.depends_on, task.rows, task.files)
    if not (isinstance(task.id, str) and isinstance(task.title, str) and isinstance(task.seam, bool)
            and isinstance(task.owner_line, str) and isinstance(task.task_class, str)
            and isinstance(task.class_reason, str)):
        raise TypeError(f"task {entry['id']!r} has wrongly typed fields")
    if not all(isinstance(items, list) and all(isinstance(item, str) for item in items) for items in lists):
        raise TypeError(f"task {task.id} lists must hold strings")
    return task


def _duplicate_ids(tasks: list[BuildTask]) -> list[str]:
    counts = Counter(task.id for task in tasks)
    return [f"duplicate task id {task_id}" for task_id, count in sorted(counts.items()) if count > 1]


def _task_problems(task: BuildTask, known_tasks: set[str], known_rows: set[str]) -> list[str]:
    problems = [f"task {task.id} depends on unknown task {dep}" for dep in task.depends_on if dep not in known_tasks]
    problems += [f"task {task.id} references unknown row {row}" for row in task.rows if row not in known_rows]
    if not task.rows:
        problems.append(f"task {task.id} has no rows")
    if not task.files:
        problems.append(f"task {task.id} has no files")
    if task.task_class not in routing.TASK_CLASSES:
        problems.append(f"task {task.id} has unknown task_class {task.task_class!r}; use {', '.join(routing.TASK_CLASSES)}")
    return problems


def _uncovered_rows(tasks: list[BuildTask], rows: list[Row]) -> list[str]:
    covered = {row for task in tasks for row in task.rows}
    return [f"row {row.id} is not covered by any task" for row in rows if not row.baseline and row.id not in covered]


def shares_files(first: BuildTask, second: BuildTask) -> bool:
    """Whether two tasks list a file in common (the whole repository, `.`, shares with every task): they must not run at
    once, since each would create or change it on its own."""
    if set(first.files) & set(second.files):
        return True
    return WHOLE_REPOSITORY in first.files or WHOLE_REPOSITORY in second.files


def _seam_problems(tasks: list[BuildTask]) -> list[str]:
    problems = []
    for task in tasks:
        neighbours = {other.id for other in tasks if other.id != task.id and set(other.files) & set(task.files)}
        if len(neighbours) >= SEAM_NEIGHBOUR_COUNT and not task.seam:
            problems.append(f"task {task.id} touches files of {', '.join(sorted(neighbours))} and must be seam: true")
    return problems


def _cycle_errors(tasks: list[BuildTask]) -> list[str]:
    ordered = order(tasks)
    stuck = sorted(task.id for task in tasks if task.id not in ordered)
    return [f"dependency cycle among tasks: {', '.join(stuck)}"] if stuck else []
