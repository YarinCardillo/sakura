"""The only places usage is read from: authenticated flow run records and lane panes' own sessions."""

from __future__ import annotations

import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path

from sakura import repository
from sakura.flow import run_records, store
from sakura.lanes.status import LaneRow
from sakura.usage import claude, codex, jsonl
from sakura.usage.model import Sample, UsageEvent

TASK_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,47}")


@dataclass(frozen=True)
class Source:
    """What one agent (a flow run or a lane) left behind: timestamped usage and limit samples."""

    item: tuple[str, ...]
    agent: str
    provider: str
    key: tuple
    events: list[UsageEvent]
    samples: list[Sample]


def collect(repos: Iterable[Path], lane_rows: Sequence[LaneRow], sessions_dir: Path, *,
            read_state: store.ReadState | None = None) -> list[Source]:
    """Every readable source, each session once: flow runs first, then lanes whose session is not yet counted."""
    found: dict[tuple, Source] = {}
    candidates = [source for repo in repos for source in _repo_sources(repo, sessions_dir, read_state)]
    candidates += [source for row in lane_rows for source in _lane_sources(row, sessions_dir)]
    for source in candidates:
        found.setdefault(source.key, source)
    return list(found.values())


def _repo_sources(repo: Path, sessions_dir: Path, read_state: store.ReadState | None) -> list[Source]:
    try:
        root = store.flow_root(repo)
        task_ids = sorted(path.name for path in root.iterdir() if path.is_dir() and TASK_ID.fullmatch(path.name))
    except (repository.RepositoryError, OSError):
        return []
    return [source for task_id in task_ids for source in _task_sources(repo, task_id, sessions_dir, read_state)]


def _task_sources(repo: Path, task_id: str, sessions_dir: Path, read_state: store.ReadState | None) -> list[Source]:
    task = store.TaskStore(repo, task_id)
    try:
        state = (read_state(task) if read_state else task.read_state()) if task.exists() else {}
        runs = state.get("runs")
    except store.StateError:
        return []
    if not isinstance(runs, dict):
        return []
    item = ("flow", str(repo), task_id)
    found = (_run_source(task, item, run, sessions_dir) for run in runs.values() if isinstance(run, dict))
    return [source for source in found if source]


def _run_source(task: store.TaskStore, item: tuple, run: dict, sessions_dir: Path) -> Source | None:
    name, ended_at = run.get("name"), jsonl.from_iso(run.get("ended_at"))
    if not isinstance(name, str) or not run_records.RUN_NAME.fullmatch(name) or ended_at is None:
        return None
    if run.get("provider") == claude.PROVIDER:
        log = run_records.log_path(task, name)
        events = claude.run_usage(log, ended_at)
        return Source(item, name, claude.PROVIDER, ("claude-run", *item, name), [events] if events else [],
                      claude.run_samples(log, ended_at))
    return _codex_source(item, name, run.get("session_id"), sessions_dir)


def _lane_sources(row: LaneRow, sessions_dir: Path) -> list[Source]:
    item = ("lane", row.group, row.name)
    if row.tool == claude.PROVIDER and row.transcript and row.session_id:
        events = claude.transcript_events(Path(row.transcript))
        return [Source(item, row.name, claude.PROVIDER, (claude.PROVIDER, row.session_id), events, [])]
    source = _codex_source(item, row.name, row.session_id, sessions_dir) if row.tool == codex.PROVIDER else None
    return [source] if source else []


def _codex_source(item: tuple, agent: str, session_id: object, sessions_dir: Path) -> Source | None:
    rollout = codex.rollout_for(sessions_dir, session_id) if isinstance(session_id, str) else None
    if rollout is None:
        return None
    return Source(item, agent, codex.PROVIDER, (codex.PROVIDER, session_id), codex.session_events(rollout),
                  codex.samples(rollout))
