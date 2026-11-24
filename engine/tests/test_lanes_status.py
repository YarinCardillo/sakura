from __future__ import annotations

import dataclasses
import json
import os
import re
import subprocess
import time
import uuid
from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace

import pytest
from sakura.lanes import identity, lane, status, transcripts
from test_lanes import SESSION, git, start_session
from test_lanes_identity import make_group
from tmux_support import needs_tmux, private_tmux, wait_until

SESSION_ID = "11111111-2222-4333-8444-555555555555"
WORKING = "⠋ working"
WAITING = "✳ idle"


def agent_script(path: Path, title: str, capture: Path) -> Path:
    """A pseudo agent: sets its pane title, then records everything typed into it."""
    path.write_text(f"#!/bin/sh\nprintf '\\033]2;{title}\\007'\nexec cat >> {capture}\n")
    path.chmod(0o755)
    return path


@pytest.fixture
def world(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    (repo / "README").write_text("x\n")
    git(repo, "add", "README")
    git(repo, "commit", "-q", "-m", "init")
    captures = SimpleNamespace(agent=tmp_path / "agent.txt", stack=tmp_path / "stack.txt")
    with private_tmux() as client:
        yield SimpleNamespace(repo=repo, client=client, tmp=tmp_path, captures=captures,
                              sources=transcripts.Sources(records_dir=tmp_path / "records", home=tmp_path / "home"))


def make_world_group(world, title: str = WAITING, status_command: str = "true"):
    script = agent_script(world.tmp / "agent.sh", title, world.captures.agent)
    return make_group(world.tmp, repo=world.repo, session=SESSION, branch="feat/{name}", base="main",
                      agent=str(script), up=f"cat >> {world.captures.stack}", down="true", setup=("true",),
                      status=status_command, worktree_root=world.tmp / "trees")


def open_lane(world, group, name: str = "alpha"):
    start_session(SimpleNamespace(client=world.client, tmp=world.tmp))
    lane_id = identity.resolve(group, name)
    lane.open(lane_id, group, None, world.client)
    return lane_id


def agent_pane(world, name: str = "alpha") -> str:
    window = next(w for w in world.client.windows(SESSION) if w.name == name)
    return next(pane.id for pane in window.panes if pane.marker == "agent")


def wait_for_title(world, pane: str, title: str) -> None:
    assert wait_until(lambda: world.client.pane_title(pane) == title), world.client.pane_title(pane)


def write_record(world, pane: str, worktree: Path, tool: str = "claude", age: float = 0.0) -> Path:
    world.sources.records_dir.mkdir(parents=True, exist_ok=True)
    path = world.sources.records_dir / f"{pane[1:]}.json"
    path.write_text(json.dumps({"tool": tool, "session_id": SESSION_ID, "cwd": str(worktree),
                                "claude_config_dir": "", "permission_mode": "default"}))
    if age:
        past = time.time() - age
        os.utime(path, (past, past))
    return path


def transcript_path(world, worktree: Path) -> Path:
    slug = re.sub(r"[^A-Za-z0-9]", "-", str(worktree))
    return world.sources.home / ".claude" / "projects" / slug / f"{SESSION_ID}.jsonl"


def entry(kind: str, model: str = "", when: float | None = None, text: str = "") -> str:
    moment = datetime.fromtimestamp(time.time() if when is None else when, UTC)
    stamp = moment.isoformat(timespec="milliseconds").replace("+00:00", "Z")
    message = {"role": "assistant", "model": model} if kind == "assistant" else {"role": "user", "content": text}
    return json.dumps({"type": kind, "timestamp": stamp, "message": message}) + "\n"


def write_transcript(world, worktree: Path, *lines: str) -> Path:
    path = transcript_path(world, worktree)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(lines))
    return path


def fake_gh(stdout: str = '{"number": 7, "state": "OPEN"}', code: int = 0):
    calls = []

    def run(argv, cwd):
        calls.append((list(argv), cwd))
        return subprocess.CompletedProcess(argv, code, stdout=stdout, stderr="")

    run.calls = calls
    return run


@pytest.mark.parametrize("title,expected", [
    ("⠋ Fixing the bug", "working"), ("⠀", "working"), ("⣿ x", "working"),
    ("✳ Claude Code", "waiting"), ("Claude Code", "unknown"), ("", "unknown"), (" ✳", "unknown"),
])
def test_agent_state_from_title(title, expected):
    assert status.agent_state(title) == expected


@needs_tmux
def test_rows_only_for_marked_windows(world):
    group = make_world_group(world)
    lane_id = open_lane(world, group)
    world.client.new_window(SESSION, "scratch", ["sleep", "300"], world.tmp, {})
    other = dataclasses.replace(group, name="elsewhere")
    gh = fake_gh()
    rows = status.collect(group, world.client, gh, world.sources)
    assert [row.name for row in rows] == ["alpha"]
    row = rows[0]
    assert (row.group, row.branch, row.worktree) == ("work", "feat/alpha", str(lane_id.worktree))
    assert row.last_commit.endswith("init") and row.stack == "up" and row.pr == "#7 OPEN"
    assert gh.calls == [(["pr", "view", "feat/alpha", "--json", "number,state"], group.repo)]
    assert status.collect(other, world.client, gh, world.sources) == []


@needs_tmux
def test_row_state_stack_and_pr_failures(world):
    group = make_world_group(world, title=WORKING, status_command="false")
    open_lane(world, group)
    wait_for_title(world, agent_pane(world), WORKING)
    row = status.collect(group, world.client, fake_gh(code=1, stdout="boom"), world.sources)[0]
    assert (row.state, row.stack, row.pr) == ("working", "down", "")
    row = status.collect(group, world.client, fake_gh(stdout="not json"), world.sources)[0]
    assert row.pr == ""


@needs_tmux
def test_row_reads_tool_session_and_model_from_current_record(world):
    group = make_world_group(world)
    lane_id = open_lane(world, group)
    pane = agent_pane(world)
    wait_for_title(world, pane, WAITING)
    write_record(world, pane, lane_id.worktree)
    path = write_transcript(world, lane_id.worktree, entry("assistant", "claude-opus-5-5"),
                            entry("assistant", "claude-sonnet-5-5"))
    row = status.collect(group, world.client, fake_gh(), world.sources)[0]
    assert (row.state, row.tool, row.session_id, row.model) == ("waiting", "claude", SESSION_ID, "claude-sonnet-5-5")
    assert row.transcript == str(path)


@needs_tmux
def test_stale_pane_record_after_server_restart_ignored(world):
    group = make_world_group(world)
    lane_id = open_lane(world, group)
    pane = agent_pane(world)
    write_record(world, pane, lane_id.worktree, age=86400)
    write_transcript(world, lane_id.worktree, entry("assistant", "claude-opus-5-5"))
    row = status.collect(group, world.client, fake_gh(), world.sources)[0]
    assert (row.tool, row.session_id, row.model) == ("", "", "")


@needs_tmux
def test_record_for_another_directory_ignored(world):
    group = make_world_group(world)
    open_lane(world, group)
    write_record(world, agent_pane(world), world.tmp / "elsewhere")
    row = status.collect(group, world.client, fake_gh(), world.sources)[0]
    assert (row.tool, row.session_id) == ("", "")


@needs_tmux
def test_hostile_values_reach_no_row_unstripped(world):
    group = make_world_group(world)
    lane_id = open_lane(world, group)
    git(lane_id.worktree, "commit", "-q", "--allow-empty", "-m", "evil \x1b]2;owned\x07 subject")
    row = status.collect(group, world.client, fake_gh(stdout='{"number": 1, "state": "OP\\u001b[31mEN"}'),
                         world.sources)[0]
    text = status.render([row]) + status.render_json([row])
    assert "\x1b" not in text and "\x07" not in text and "evil" in text


def test_transcript_model_is_latest_assistant_and_skips_synthetic(tmp_path):
    path = tmp_path / "t.jsonl"
    path.write_text(entry("assistant", "claude-opus-5-5") + "not json\n" + entry("user", text="hi")
                    + entry("assistant", "claude-sonnet-5-5") + entry("assistant", "<synthetic>"))
    assert transcripts.model(path) == "claude-sonnet-5-5"
    assert transcripts.model(tmp_path / "missing.jsonl") == ""


def test_transcript_model_found_beyond_the_tail_window_is_empty_not_wrong(tmp_path):
    path = tmp_path / "t.jsonl"
    path.write_text(entry("assistant", "claude-opus-5-5") + entry("user", text="x" * (transcripts.TAIL_BYTES * 2)))
    assert transcripts.model(path) == ""


def test_model_change_seen_needs_a_newer_matching_entry(tmp_path):
    now = time.time()
    path = tmp_path / "t.jsonl"
    path.write_text(entry("assistant", "claude-sonnet-5-5", when=now - 60))
    assert not transcripts.model_change_seen(path, "sonnet", now)
    path.write_text(entry("assistant", "claude-sonnet-5-5", when=now + 1))
    assert transcripts.model_change_seen(path, "sonnet", now)
    assert not transcripts.model_change_seen(path, "opus", now)
    ack = "<local-command-stdout>Set model to `Sonnet 5.5`</local-command-stdout>"
    path.write_text(entry("user", text=ack, when=now + 1))
    assert transcripts.model_change_seen(path, "sonnet", now)
    path.write_text(entry("user", text=ack, when=now - 5))
    assert not transcripts.model_change_seen(path, "sonnet", now)


def test_model_change_seen_acknowledgement_must_name_the_requested_model(tmp_path):
    now = time.time()
    path = tmp_path / "t.jsonl"
    sonnet_ack = "<local-command-stdout>Set model to `Sonnet 5.5`</local-command-stdout>"
    path.write_text(entry("user", text=sonnet_ack, when=now + 1))
    assert not transcripts.model_change_seen(path, "opus", now)
    opus_ack = "<local-command-stdout>Set model to Opus 5 (1M context)</local-command-stdout>"
    path.write_text(entry("user", text=opus_ack, when=now + 1))
    assert transcripts.model_change_seen(path, "opus", now)
    assert not transcripts.model_change_seen(path, "sonnet", now)
    path.write_text(entry("user", text=sonnet_ack, when=now + 1))
    assert transcripts.model_change_seen(path, "sonnet-5.5", now)
    assert not transcripts.model_change_seen(path, "sonnet-4-5", now)


def test_locate_requires_current_record_claude_and_a_safe_session_id(tmp_path):
    worktree = tmp_path / "wt"
    worktree.mkdir()
    sources = transcripts.Sources(records_dir=tmp_path / "r", home=tmp_path / "home")
    target = tmp_path / "home" / ".claude" / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(worktree.resolve()))
    target.mkdir(parents=True)
    (target / f"{SESSION_ID}.jsonl").write_text("")
    record = {"tool": "claude", "session_id": SESSION_ID, "cwd": str(worktree), "claude_config_dir": "",
              "recorded_at": 200.0, "server_started_at": 100.0}
    assert transcripts.locate(record, worktree, sources) == target / f"{SESSION_ID}.jsonl"
    assert transcripts.locate({**record, "recorded_at": 99.0}, worktree, sources) is None
    assert transcripts.locate({**record, "cwd": str(tmp_path)}, worktree, sources) is None
    assert transcripts.locate({**record, "tool": "codex"}, worktree, sources) is None
    assert transcripts.locate({**record, "session_id": "../../x"}, worktree, sources) is None
    assert transcripts.locate({**record, "session_id": str(uuid.uuid4())}, worktree, sources) is None
