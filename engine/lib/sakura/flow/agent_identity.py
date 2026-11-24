"""Which piece of work a run is a round of: the facts the app uses to group runs into agents.

An agent is every run that serves the same step, the same phase (spec, contract, plan, build, review) and the same plan
task. A resume of a run (`<run>.<n>`) belongs to the agent of its root run. Rounds count from 1 in start order.
"""

from __future__ import annotations

from sakura.flow import plan, run_records
from sakura.flow.store import TaskStore

TASK_ENV = "SAKURA_FLOW_BUILD_TASK"


def identities(store: TaskStore, state: dict) -> dict[str, dict]:
    """Run name -> `agent`, `round`, `task_id` and `task_title` for every run recorded in `state`.

    `task_id` and `task_title` are empty for a run that serves no task of the plan (and for every run while the plan is
    not written or cannot be read: the titles are the plan's own)."""
    titles = _plan_titles(store)
    records = sorted(state.get("runs", {}).values(), key=lambda run: (run["started_at"], run["name"]))
    rounds: dict[str, int] = {}
    found = {}
    for record in records:
        origin = _origin(state, record)
        key = _key(origin)
        rounds[key] = rounds.get(key, 0) + 1
        task = (origin.get("env") or {}).get(TASK_ENV, "")
        found[record["name"]] = {"agent": key, "round": rounds[key], "task_id": task if task in titles else "",
                                 "task_title": titles.get(task, "")}
    return found


def _origin(state: dict, record: dict) -> dict:
    """The record whose step, phase and task a run has: the root run of its resumes, else the run itself."""
    root = state.get("runs", {}).get(record["name"].split(".")[0])
    return root or record


def _key(origin: dict) -> str:
    step = origin.get("step") or run_records.STEP_OF_ROLE.get(origin.get("role", ""), "")
    task = (origin.get("env") or {}).get(TASK_ENV, "")
    return ":".join((step, origin.get("phase", ""), task))


def _plan_titles(store: TaskStore) -> dict[str, str]:
    try:
        return {task.id: task.title for task in plan.load(store, None)}
    except plan.PlanError:
        return {}
