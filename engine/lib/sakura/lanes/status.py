"""Lane status: one row per lane window sakura marked, from tmux, git, the pane record and gh."""

from __future__ import annotations

import dataclasses
import json
import os
import shlex
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from sakura import programs
from sakura.flow.process import child_environment
from sakura.flow.render import strip_controls
from sakura.flow.tmux import TmuxClient, TmuxError
from sakura.lanes import identity, lane, transcripts
from sakura.lanes.config import LaneGroup
from sakura.lanes.errors import LaneError
from sakura.lanes.transcripts import Sources
from sakura.lanes.worktrees import git

STATUS_TIMEOUT_SECONDS = 10
GH_TIMEOUT_SECONDS = 15
UNKNOWN = "unknown"
KNOWN_TOOLS = ("claude", "codex")
_BRAILLE = range(0x2800, 0x2900)
_WAITING_MARK = "✳"
_COLUMNS = (("LANE", "name"), ("STATE", "state"), ("TOOL", "tool"), ("MODEL", "model"), ("STACK", "stack"),
            ("BRANCH", "branch"), ("PR", "pr"), ("LAST COMMIT", "last_commit"))

Gh = Callable[[list[str], Path], subprocess.CompletedProcess]


@dataclass(frozen=True)
class LaneRow:
    """What `lane status` knows about one lane; "" means not known. `window_activity` is epoch seconds."""

    group: str
    name: str
    worktree: str
    branch: str
    last_commit: str
    stack: str
    tool: str
    state: str
    model: str
    session_id: str
    pr: str
    transcript: str = ""
    window_activity: str = ""


def agent_state(title: str) -> str:
    """Return `working` for a title that starts with a braille spinner, `waiting` for ✳, else `unknown`."""
    if title and ord(title[0]) in _BRAILLE:
        return "working"
    return "waiting" if title.startswith(_WAITING_MARK) else UNKNOWN


def run_gh(argv: list[str], cwd: Path) -> subprocess.CompletedProcess:
    """Run `gh <argv>` in `cwd` without a shell and without input."""
    gh = programs.path("gh", cwd)
    return subprocess.run([gh, *argv], cwd=cwd, capture_output=True, text=True, errors="replace",
                          stdin=subprocess.DEVNULL, check=False, timeout=GH_TIMEOUT_SECONDS,
                          env=programs.environment("gh", gh))


def collect(group: LaneGroup, client: TmuxClient, gh: Gh, sources: Sources | None = None) -> list[LaneRow]:
    """Return one row per window that sakura marked as a lane of `group`; no other window is read."""
    sources = sources or Sources()
    rows = []
    for window in lane.marked_windows(client, group):
        name = window.options[lane.LANE_MARKER_KEY].removeprefix(f"{group.name}/")
        try:
            lane_id = identity.resolve(group, name)
        except identity.LaneNameError:
            continue
        rows.append(_clean(_row(group, lane_id, window, client, gh, sources)))
    return rows


def render(rows: list[LaneRow]) -> str:
    """Return the rows as an aligned text table."""
    table = [[title for title, _ in _COLUMNS]]
    table += [[getattr(row, field) or "-" for _, field in _COLUMNS] for row in rows]
    widths = [max(len(line[column]) for line in table) for column in range(len(_COLUMNS))]
    return "\n".join("  ".join(cell.ljust(width) for cell, width in zip(line, widths)).rstrip() for line in table)


def render_json(rows: list[LaneRow]) -> str:
    """Return the rows as a JSON list."""
    return json.dumps([dataclasses.asdict(row) for row in rows], indent=2)


def _row(group, lane_id, window, client, gh, sources) -> LaneRow:
    worktree = lane_id.worktree
    known = worktree.is_dir()
    return LaneRow(
        group=group.name, name=lane_id.name, worktree=str(worktree),
        branch=_git_line(worktree, "branch", "--show-current") if known else "",
        last_commit=_git_line(worktree, "log", "-1", "--format=%h %s") if known else "",
        stack=_stack_state(group, worktree) if known else UNKNOWN,
        pr=_pull_request(gh, group, lane_id.branch),
        window_activity=str(window.activity) if window.activity is not None else "",
        **_agent_facts(window, worktree, client, sources))


def _agent_facts(window, worktree: Path, client: TmuxClient, sources: Sources) -> dict[str, str]:
    facts = {"tool": "", "state": UNKNOWN, "model": "", "session_id": "", "transcript": ""}
    panes = lane.agent_panes(window)
    if len(panes) != 1:
        return facts
    pane = panes[0].id
    try:
        facts["state"] = agent_state(client.pane_title(pane))
    except TmuxError:
        return facts
    record = transcripts.read_record(client, pane, sources)
    if transcripts.is_current(record, worktree):
        facts["tool"] = record["tool"] if record.get("tool") in KNOWN_TOOLS else ""
        facts["session_id"] = _session_id(record)
        path = transcripts.locate(record, worktree, sources)
        facts["model"] = transcripts.model(path) if path else ""
        facts["transcript"] = str(path) if path else ""
    return facts


def _session_id(record: dict) -> str:
    session = record.get("session_id")
    return session if isinstance(session, str) and transcripts.SESSION_ID.fullmatch(session) else ""


def _git_line(worktree: Path, *args: str) -> str:
    try:
        return git(worktree, *args, check=False).stdout.strip()
    except LaneError:
        return ""


def _stack_state(group: LaneGroup, worktree: Path) -> str:
    """`up` when the group's status command exits 0, `down` when it exits otherwise, else unknown."""
    try:
        done = subprocess.run(shlex.split(group.status), cwd=worktree, env=child_environment(os.environ),
                              capture_output=True, stdin=subprocess.DEVNULL, check=False, timeout=STATUS_TIMEOUT_SECONDS)
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return UNKNOWN
    return "up" if done.returncode == 0 else "down"


def _pull_request(gh: Gh, group: LaneGroup, branch: str) -> str:
    try:
        done = gh(["pr", "view", branch, "--json", "number,state"], group.repo)
        data = json.loads(done.stdout) if done.returncode == 0 else None
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return ""
    if not isinstance(data, dict) or not isinstance(data.get("number"), int) or not isinstance(data.get("state"), str):
        return ""
    return f"#{data['number']} {data['state']}"


def _clean(row: LaneRow) -> LaneRow:
    return dataclasses.replace(row, **{field.name: strip_controls(getattr(row, field.name))
                                       for field in dataclasses.fields(row)})
