"""Past samples of what work cost in weighted tokens, from finished flow runs and merged pull requests."""

from __future__ import annotations

import sqlite3
from collections import defaultdict
from collections.abc import Iterable, Iterator
from datetime import datetime
from pathlib import Path
from typing import NamedTuple

from sakura.budget import routing
from sakura.flow import plan, run_records, store, tasks
from sakura.usage import claude
from sakura.usage.archive import calibration

BUILD = "build"
PULL_REQUEST = "pr"
BUILDER_ROLE = "builder"
TASK_KINDS = (BUILD, PULL_REQUEST)
OTHER = calibration.OTHER

MERGED_PR_TURNS = """
SELECT p.repo, p.number, p.changed_files, p.additions + p.deletions, t.model,
       t.input_tokens, t.cache_write_tokens, t.cache_read_tokens, t.output_tokens
FROM pull_requests p JOIN pr_turns t ON t.repo = p.repo AND t.number = p.number
WHERE p.merged_at IS NOT NULL AND p.changed_files > 0 AND t.provider = 'claude'
"""


class Sample(NamedTuple):
    """What one unit of work cost. `kind` is `build` (a flow build task), `pr` or a flow role name.

    `task_class` is set for flow build tasks only; `model_class` is the class that spent most of the tokens.
    """

    kind: str
    task_class: str | None
    model_class: str
    files: int
    lines: int
    rows: int
    weighted: float


class Run(NamedTuple):
    """A recorded flow run: the repository directory and task it belongs to, its role and Claude session id."""

    repo: str
    task_id: str
    role: str
    session_id: str


def samples(conn: sqlite3.Connection, flow_repos: Iterable[Path] = ()) -> list[Sample]:
    """Merged pull requests of the archive `conn` plus the finished flow runs of `flow_repos`.

    The repositories are injected: the caller passes `registry.repos()`, a test passes its own.
    """
    return [*pr_samples(conn), *flow_samples(flow_repos)]


def pr_samples(conn: sqlite3.Connection) -> list[Sample]:
    """One sample per merged pull request that changed files, with the weighted tokens of its turns."""
    totals: dict[tuple, dict[str, float]] = defaultdict(lambda: defaultdict(float))
    for repo, number, files, lines, model, *counts in conn.execute(MERGED_PR_TURNS):
        totals[(repo, number, files, lines)][calibration.model_class(model)] += calibration.weighted_tokens(*counts)
    return [Sample(PULL_REQUEST, None, _dominant(by_class), files, lines, 0, sum(by_class.values()))
            for (_, _, files, lines), by_class in totals.items()]


def flow_samples(repos: Iterable[Path]) -> list[Sample]:
    """The builder runs of each finished build task, summed, and each other finished role run on its own."""
    found: list[Sample] = []
    for task in _task_stores(repos):
        found.extend(_task_samples(task))
    return found


def flow_runs(repos: Iterable[Path]) -> list[Run]:
    """The Claude runs of the flow tasks of `repos` whose session id is recorded (live runs have none yet)."""
    found: list[Run] = []
    for task in _task_stores(repos):
        try:
            runs = task.read_state().get("runs", {}).values()
        except store.StateError:
            continue
        found.extend(Run(task.repo.name, task.task_id, record["role"], record["session_id"])
                     for record in runs if record.get("provider") == "claude" and record.get("session_id"))
    return found


def class_of(model: str) -> str:
    """`claude-opus-5-5` and the aliases `opus`, `sonnet:high` give `opus`, `sonnet`; anything else `other`."""
    named = calibration.model_class(model)
    if named != OTHER:
        return named
    return next((name for name in routing.MODEL_LADDER if name in model.lower()), OTHER)


def _task_stores(repos: Iterable[Path]) -> Iterator[store.TaskStore]:
    for repo in repos:
        for task_id in tasks.known_tasks(repo):
            yield store.TaskStore(repo, task_id)


def _dominant(by_class: dict[str, float]) -> str:
    return max(by_class, key=lambda name: by_class[name])


def _task_samples(task: store.TaskStore) -> list[Sample]:
    try:
        state = task.read_state()
    except store.StateError:
        return []
    weighted = [(record, _weighted(task, record)) for record in state.get("runs", {}).values()
                if record.get("state") == "finished" and record.get("provider") == "claude"]
    weighted = [(record, value) for record, value in weighted if value is not None]
    builds = _build_samples(task, state, [pair for pair in weighted if pair[0]["role"] == BUILDER_ROLE])
    roles = [Sample(record["role"], None, class_of(record.get("model", "")), 0, 0, 0, value)
             for record, value in weighted if record["role"] != BUILDER_ROLE]
    return builds + roles


def _weighted(task: store.TaskStore, record: dict) -> float | None:
    """Weighted tokens of the run's result event; None when the log is missing or has none."""
    ended = datetime.fromisoformat(record["ended_at"])
    event = claude.run_usage(run_records.log_path(task, record["name"]), ended)
    if event is None:
        return None
    usage = event.usage
    return calibration.weighted_tokens(usage.uncached_input, usage.cache_write, usage.cache_read, usage.output)


def _build_samples(task: store.TaskStore, state: dict, runs: list[tuple[dict, float]]) -> list[Sample]:
    """Builder runs belong to the build task whose workspace they ran in."""
    try:
        by_id = {entry.id: entry for entry in plan.load(task, None)}
    except plan.PlanError:
        return []
    owner = {str(Path(entry["path"]).resolve()): task_id for task_id, build in state.get("build", {}).items()
             for name, entry in state.get("workspaces", {}).items() if build.get("workspace") == name}
    spent: dict[str, dict[str, float]] = defaultdict(lambda: defaultdict(float))
    for record, value in runs:
        task_id = owner.get(record["cwd"])
        if task_id in by_id:
            spent[task_id][class_of(record.get("model", ""))] += value
    return [Sample(BUILD, by_id[task_id].task_class, _dominant(by_class), len(by_id[task_id].files), 0,
                   len(by_id[task_id].rows), sum(by_class.values())) for task_id, by_class in spent.items()]
