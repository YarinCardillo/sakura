"""When an item entered its state (`since`) and when it last showed life (`last_activity_at`).

Both come from files the snapshot already reads, the tmux window listing it already makes, and for a lane one
`git log` on its branch. Every figure is a UTC ISO string with `+00:00` (the form the task history uses), or
None when nothing is known; a missing or unreadable source contributes nothing and never raises.
"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path

from sakura.flow import gitutil, store
from sakura.lanes.status import LaneRow

COMMIT_TIME_FORMAT = "--format=%cI"

Times = tuple[str | None, str | None]


def flow_times(task: store.TaskStore, state: dict) -> Times:
    """Return `(since, last_activity_at)` of a flow task.

    `since` is the time of the last history entry; the activity is the newest of that entry, the ledger and
    every file directly in the runs directory.
    """
    entered = _instant(_last_history_time(state))
    return _iso(entered), _iso(_newest(entered, _mtime(task.ledger), *_run_mtimes(task)))


def unknown_times() -> Times:
    """The times of an item whose state cannot be read."""
    return None, None


def lane_times(row: LaneRow, repo: Path) -> Times:
    """Return `(since, last_activity_at)` of a lane.

    `since` is the creation time of the worktree (no lane store records state changes), None where the platform
    keeps no creation time. The activity is the newer of the tmux window activity and the branch's last commit.
    """
    activity = _newest(_epoch(row.window_activity), _commit_time(repo, row.branch))
    return _iso(_created(Path(row.worktree))), _iso(activity)


def _last_history_time(state: dict) -> object:
    history = state.get("history")
    last = history[-1] if isinstance(history, list) and history else None
    return last.get("at") if isinstance(last, dict) else None


def _run_mtimes(task: store.TaskStore) -> list[datetime | None]:
    try:
        return [_mtime(path) for path in task.runs_dir.iterdir() if path.is_file()]
    except OSError:
        return []


def _commit_time(repo: Path, branch: str) -> datetime | None:
    if not branch:
        return None
    try:
        return _instant(gitutil.git(repo, "log", "-1", COMMIT_TIME_FORMAT, f"refs/heads/{branch}", "--").strip())
    except gitutil.GitError:
        return None


def _created(path: Path) -> datetime | None:
    try:
        return _epoch(path.stat().st_birthtime)
    except (OSError, AttributeError):
        return None


def _mtime(path: Path) -> datetime | None:
    try:
        return _epoch(path.stat().st_mtime)
    except OSError:
        return None


def _epoch(seconds: object) -> datetime | None:
    try:
        return datetime.fromtimestamp(float(seconds), timezone.utc)
    except (TypeError, ValueError, OverflowError, OSError):
        return None


def _instant(text: object) -> datetime | None:
    """An ISO 8601 time as an aware UTC moment; a time without a zone is taken as UTC, text that is no time is None."""
    if not isinstance(text, str) or not text:
        return None
    try:
        moment = datetime.fromisoformat(text)
    except ValueError:
        return None
    return moment.astimezone(timezone.utc) if moment.tzinfo else moment.replace(tzinfo=timezone.utc)


def _newest(*moments: datetime | None) -> datetime | None:
    known = [moment for moment in moments if moment]
    return max(known) if known else None


def _iso(moment: datetime | None) -> str | None:
    return moment.isoformat(timespec="seconds") if moment else None
