"""`since` and `last_activity_at` of items: from the task state and file times, and, for the shelved tmux lanes,
from the lane window listing."""

import os
import re
import subprocess
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest

from sakura.cockpit import api, items, snapshot
from sakura.flow.tmux import Window
from sakura.lanes import status
from sakura.lanes.lane import LANE_MARKER_KEY
from test_cockpit_snapshot import NIGHT, create_task, item_for, make_item, make_snapshot
from test_lanes import git
from test_lanes_identity import make_group
from test_lanes_status import fake_gh

UTC_ISO = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\+00:00")
HISTORY = [
    {"at": "2026-10-23T10:00:00+00:00", "phase": "intake", "status": "running", "detail": ""},
    {"at": "2026-10-23T11:24:23+00:00", "phase": "plan", "status": "running", "detail": ""},
]


def epoch(text: str) -> float:
    return datetime.fromisoformat(text).timestamp()


def touch(path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()
    os.utime(path, (epoch(text), epoch(text)))


class OneSessionClient:
    """A fake TmuxClient that lists the given windows and answers nothing else."""

    def __init__(self, *windows: Window) -> None:
        self._windows = list(windows)

    def has_session(self, name: str) -> bool:
        return True

    def windows(self, session: str) -> list[Window]:
        return self._windows


def collect_flow(repo):
    return snapshot.collect([repo], NIGHT)


def lane_world(tmp_path, repo, commit_date: str, birth_dir: bool = True):
    group = make_group(tmp_path, repo=repo, session="s", worktree_root=tmp_path / "trees", branch="feat/{name}",
                       base="main", status="true")
    work = group.worktree_root / "alpha"
    git(repo, "worktree", "add", "-q", "-b", "feat/alpha", str(work))
    (work / "NEW").write_text("n\n")
    git(work, "add", "NEW")
    subprocess.run(["git", "-C", str(work), "commit", "-q", "-m", "lane work"], check=True,
                   env={**os.environ, "GIT_COMMITTER_DATE": commit_date, "GIT_AUTHOR_DATE": commit_date})
    return group


def lane_item_of(group, activity):
    """The item of the shelved tmux lane `alpha`: the snapshot no longer lists lanes, the item builder stays."""
    window = Window("@1", "alpha", {LANE_MARKER_KEY: "work/alpha"}, False, [], activity)
    (row,) = status.collect(group, OneSessionClient(window), fake_gh(), None)
    return items.lane_item(row, group, None)


def test_flow_since_is_the_last_history_entry(repo):
    create_task(repo, "demo", history=HISTORY)
    assert item_for(collect_flow(repo), "demo").since == "2026-10-23T11:24:23+00:00"


def test_gate_since_is_the_awaiting_approval_entry(repo):
    gate_entry = {"at": "2026-10-23T12:30:00+00:00", "phase": "spec", "status": "awaiting_approval", "detail": "gate"}
    create_task(repo, "demo", status="awaiting_approval", history=[*HISTORY, gate_entry])
    item = item_for(collect_flow(repo), "demo")
    assert (item.agent_state, item.since) == ("gate", "2026-10-23T12:30:00+00:00")


def test_flow_last_activity_is_the_newest_of_ledger_runs_history(repo):
    task = create_task(repo, "demo", history=HISTORY)
    touch(task.ledger, "2026-10-23T12:00:00+00:00")
    touch(task.runs_dir / "build-1.log", "2026-10-23T13:00:00+00:00")
    touch(task.runs_dir / "plan-1.log", "2026-10-23T11:45:00+00:00")
    assert item_for(collect_flow(repo), "demo").last_activity_at == "2026-10-23T13:00:00+00:00"

    touch(task.ledger, "2026-10-23T14:00:00+00:00")
    assert item_for(collect_flow(repo), "demo").last_activity_at == "2026-10-23T14:00:00+00:00"

    task.write_state({**task.read_state(), "history": [*HISTORY, {**HISTORY[1], "at": "2026-10-23T15:00:00+00:00"}]})
    assert item_for(collect_flow(repo), "demo").last_activity_at == "2026-10-23T15:00:00+00:00"


@pytest.mark.parametrize("activity,expected", [
    (epoch("2026-10-24T09:00:00+00:00"), "2026-10-24T09:00:00+00:00"),
    (epoch("2026-10-20T07:00:00+00:00"), "2026-10-20T08:00:00+00:00"),
])
def test_lane_last_activity_uses_window_activity_and_last_commit(tmp_path, repo, activity, expected):
    """The commit was made at 10:00 +02:00 on the 20th, that is 08:00 UTC; the newer of the two wins."""
    group = lane_world(tmp_path, repo, "2026-10-20T10:00:00+02:00")
    assert lane_item_of(group, int(activity)).last_activity_at == expected


def test_lane_last_activity_without_a_window_activity_is_the_last_commit(tmp_path, repo):
    group = lane_world(tmp_path, repo, "2026-10-20T10:00:00+02:00")
    assert lane_item_of(group, None).last_activity_at == "2026-10-20T08:00:00+00:00"


@pytest.mark.skipif(not hasattr(os.stat_result, "st_birthtime"), reason="the platform keeps no creation time")
def test_lane_since_is_the_creation_of_its_worktree(tmp_path, repo):
    group = lane_world(tmp_path, repo, "2026-10-20T10:00:00+02:00")
    created = datetime.fromtimestamp((group.worktree_root / "alpha").stat().st_birthtime, timezone.utc)
    assert lane_item_of(group, None).since == created.isoformat(timespec="seconds")


def test_unknown_times_are_null(repo, tmp_path):
    create_task(repo, "bare").ledger.unlink()
    unreadable = create_task(repo, "broken")
    unreadable.state_file.write_text("not json")
    flow = collect_flow(repo)
    for task in ("bare", "broken"):
        item = item_for(flow, task)
        assert (item.since, item.last_activity_at) == (None, None)

    group = make_group(tmp_path, repo=repo, session="s", worktree_root=tmp_path / "trees", status="true")
    lane = lane_item_of(group, None)
    assert (lane.since, lane.last_activity_at) == (None, None)


def test_a_history_entry_with_no_readable_time_counts_as_unknown(repo):
    task = create_task(repo, "demo", history=[{"at": "yesterday", "phase": "plan", "status": "running", "detail": ""}])
    task.ledger.unlink()
    item = item_for(collect_flow(repo), "demo")
    assert (item.since, item.last_activity_at) == (None, None)


def test_times_are_utc_iso(tmp_path, repo):
    offset = [{"at": "2026-10-23T13:24:23+02:00", "phase": "plan", "status": "running", "detail": ""}]
    task = create_task(repo, "demo", history=offset)
    touch(task.ledger, "2026-10-23T08:00:00+00:00")
    item = item_for(collect_flow(repo), "demo")
    assert item.since == "2026-10-23T11:24:23+00:00"
    assert UTC_ISO.fullmatch(item.since) and UTC_ISO.fullmatch(item.last_activity_at)

    group = lane_world(tmp_path, repo, "2026-10-20T10:00:00+02:00")
    lane = lane_item_of(group, int(epoch("2026-10-24T09:00:00+00:00")))
    assert UTC_ISO.fullmatch(lane.last_activity_at)


def test_the_api_renders_both_times_as_z_and_null_when_unknown():
    known = make_item(since="2026-10-23T11:24:23+00:00", last_activity_at="2026-10-23T15:00:00+00:00")
    document = api.snapshot_document(make_snapshot(items=[known, make_item()]))["items"]
    assert (document[0]["since"], document[0]["last_activity_at"]) == ("2026-10-23T11:24:23Z", "2026-10-23T15:00:00Z")
    assert (document[1]["since"], document[1]["last_activity_at"]) == (None, None)
