"""`sakura usage`: weekly consumption from local logs only; reads files and nothing else."""

from __future__ import annotations

import os
import subprocess
from datetime import datetime, timezone
from pathlib import Path

from sakura import repository
from sakura.accounts.providers import ADAPTERS
from sakura.flow.tmux import TmuxClient, TmuxError
from sakura.lanes import config, status
from sakura.lanes.errors import LaneError
from sakura.usage import aggregate, report


def run(as_json: bool) -> int:
    """Print the usage of this repository's flow tasks and of the configured lanes."""
    week = aggregate.week(_repos(), _lane_rows(), codex_sessions(), _utc_now())
    print(report.render_json(week) if as_json else report.render(week))
    return 0


def codex_sessions() -> Path:
    """The directory of Codex session rollouts under CODEX_HOME, else ~/.codex."""
    adapter = ADAPTERS["codex"]
    return Path(os.environ.get(adapter.env_var) or adapter.default_dir).expanduser() / adapter.transcript_subdir


def _utc_now() -> datetime:
    return datetime.now(timezone.utc)


def _repos() -> list[Path]:
    try:
        return [repository.root(Path.cwd())]
    except repository.RepositoryError:
        return []


def _lane_rows() -> list[status.LaneRow]:
    """Lane rows of every configured group; none without a lane config or a tmux server to ask."""
    try:
        groups = config.load()
    except config.LaneConfigError:
        return []
    client = TmuxClient()
    rows: list[status.LaneRow] = []
    for group in groups.values() if client.available() else ():
        try:
            rows += status.collect(group, client, _no_pull_requests)
        except (LaneError, TmuxError):
            continue
    return rows


def _no_pull_requests(argv: list[str], cwd: Path) -> subprocess.CompletedProcess:
    """Usage never asks GitHub; a failed lookup leaves the PR column empty."""
    return subprocess.CompletedProcess(argv, 1, "", "")
