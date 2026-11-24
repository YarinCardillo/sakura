from __future__ import annotations

import json
import time
from pathlib import Path

import pytest
from sakura.lanes import cli
from sakura.lanes import input as lane_input
from sakura.lanes.errors import InputInvalid, InputRefused
from test_lanes import SESSION
from test_lanes_status import (
    SESSION_ID,
    WAITING,
    WORKING,
    agent_pane,
    entry,
    make_world_group,
    open_lane,
    transcript_path,
    wait_for_title,
    world,
    write_record,
    write_transcript,
)
from tmux_support import needs_tmux

assert world  # fixture re-exported from test_lanes_status


def read(path: Path) -> str:
    return path.read_text() if path.exists() else ""


def settled(world, expected: str) -> bool:
    from tmux_support import wait_until

    return wait_until(lambda: read(world.captures.agent) == expected)


def ready_lane(world, title: str = WAITING, tool: str = "claude"):
    group = make_world_group(world, title=title)
    lane_id = open_lane(world, group)
    pane = agent_pane(world)
    wait_for_title(world, pane, title)
    write_record(world, pane, lane_id.worktree, tool=tool)
    return group, lane_id


def say(world, group, lane_id, text, log: Path | None = None):
    return lane_input.say(lane_id, group, text, world.client, sleep=lambda seconds: None,
                          messages_log=log or world.tmp / "messages.log")


@pytest.fixture
def refused_input(world, monkeypatch):
    sent = []
    monkeypatch.setattr(world.client, "paste", lambda *args: sent.append(args))
    monkeypatch.setattr(world.client, "send_enter", lambda *args: sent.append(args))
    return sent


@needs_tmux
def test_say_targets_agent_pane_not_stack(world):
    group, lane_id = ready_lane(world)
    log = world.tmp / "messages.log"
    say(world, group, lane_id, "hello\nworld; echo $HOME", log)
    assert settled(world, "hello\nworld; echo $HOME\n")
    assert read(world.captures.stack) == ""
    lines = log.read_text().splitlines()
    assert lines[0].endswith(f"-> lane work/alpha ({agent_pane(world)})") and lines[0].startswith("=== ")
    assert lines[1:3] == ["hello", "world; echo $HOME"]


@needs_tmux
def test_say_refuses_working(world, refused_input):
    group, lane_id = ready_lane(world, title=WORKING)
    with pytest.raises(InputRefused, match="working"):
        say(world, group, lane_id, "hi")
    assert refused_input == []
    assert read(world.captures.agent) == "" and not (world.tmp / "messages.log").exists()


@needs_tmux
def test_say_refuses_unknown_title_and_missing_lane(world):
    group, lane_id = ready_lane(world, title="plain shell")
    with pytest.raises(InputRefused, match="unknown"):
        say(world, group, lane_id, "hi")
    ghost = lane_id.__class__(**{**lane_id.__dict__, "name": "ghost", "window_marker": "work/ghost"})
    with pytest.raises(InputRefused, match="no open lane"):
        say(world, group, ghost, "hi")


@needs_tmux
def test_say_rejects_nul_and_oversize(world):
    group, lane_id = ready_lane(world)
    hostile = ["a\x00b", "x" * (lane_input.MAX_TEXT + 1), "esc \x1b[201~ rm -rf", "bell \x07", "c1 \x9b", "", "\ud800"]
    for text in hostile:
        with pytest.raises(InputInvalid):
            say(world, group, lane_id, text)
    longest = ("y" * 99 + "\n") * (lane_input.MAX_TEXT // 100)  # short lines: a tty line holds 1024 characters
    assert len(longest) == lane_input.MAX_TEXT
    say(world, group, lane_id, longest)
    assert settled(world, longest + "\n")


@needs_tmux
def test_say_refuses_two_agent_panes(world):
    group, lane_id = ready_lane(world)
    window = next(w for w in world.client.windows(SESSION) if w.name == "alpha")
    world.client.split_window(window.id, ["sleep", "300"], world.tmp, {}, "agent")
    with pytest.raises(InputRefused, match="2 agent panes"):
        say(world, group, lane_id, "hi")


def model(world, group, lane_id, name, sleep=lambda seconds: None):
    return lane_input.model(lane_id, group, name, world.client, sleep=sleep, messages_log=world.tmp / "m.log",
                            sources=world.sources)


@needs_tmux
def test_model_confirmed_from_transcript_fixture(world):
    group, lane_id = ready_lane(world)
    path = write_transcript(world, lane_id.worktree, entry("assistant", "claude-opus-5-5", when=time.time() - 60))

    def claude_reacts(seconds):
        with path.open("a") as handle:
            handle.write(entry("assistant", "claude-sonnet-5-5"))

    assert model(world, group, lane_id, "sonnet", sleep=claude_reacts) == "confirmed"
    assert settled(world, "/model sonnet\n")
    assert "/model sonnet" in read(world.tmp / "m.log")


@needs_tmux
def test_model_not_confirmed_when_the_transcript_stays_silent(world):
    group, lane_id = ready_lane(world)
    write_transcript(world, lane_id.worktree, entry("assistant", "claude-opus-5-5", when=time.time() - 60))
    polls = []
    assert model(world, group, lane_id, "sonnet", sleep=polls.append) == "sent, not confirmed"
    assert len(polls) > 2 and sum(polls) <= lane_input.MODEL_CONFIRM_SECONDS + lane_input.PASTE_SETTLE_SECONDS


@needs_tmux
def test_model_without_a_transcript_is_sent_not_confirmed(world):
    group, lane_id = ready_lane(world)
    assert not transcript_path(world, lane_id.worktree).exists()
    assert model(world, group, lane_id, "sonnet") == "sent, not confirmed"


@needs_tmux
def test_model_refuses_codex_lane(world, refused_input):
    group, lane_id = ready_lane(world, tool="codex")
    with pytest.raises(InputRefused, match="Claude"):
        model(world, group, lane_id, "sonnet")
    assert refused_input == []
    assert read(world.captures.agent) == ""


@needs_tmux
def test_model_refuses_unknown_agent_and_working_and_hostile_names(world, refused_input):
    group, lane_id = ready_lane(world)
    (world.sources.records_dir / f"{agent_pane(world)[1:]}.json").unlink()
    with pytest.raises(InputRefused, match="cannot tell"):
        model(world, group, lane_id, "sonnet")
    write_record(world, agent_pane(world), lane_id.worktree)
    for name in ["sonnet\n/quit", "a b", "x\x1b", "", "-x", "a;b", "$(id)", "a" * 100]:
        with pytest.raises(InputInvalid):
            model(world, group, lane_id, name)
    assert refused_input == []
    assert read(world.captures.agent) == ""


@needs_tmux
def test_say_and_model_log_event_with_actor_on_success_and_refusal(world, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child: the only process that may carry a viewer's identity
    group, lane_id = ready_lane(world)
    write_transcript(world, lane_id.worktree, entry("assistant", "claude-opus-5-5", when=time.time() - 60))
    config_path = world.tmp / "lanes.toml"
    config_path.write_text(
        f'[work]\nrepo = "{group.repo}"\nsession = "{SESSION}"\nworktree_root = "{group.worktree_root}"\n'
        f'branch = "feat/{{name}}"\nbase = "main"\nsetup = ["true"]\nup = "true"\ndown = "true"\n'
        f'status = "true"\nagent = "{group.agent}"\n')
    log = world.tmp / "events.jsonl"
    actor = ["--actor", "overlay:alice@example.com:nodeABC"]
    long_text = "word " * 40

    def run(*argv):
        return cli.main([*argv, *actor], client=world.client, config_path=config_path, events_path=log,
                        sources=world.sources, messages_log=world.tmp / "messages.log", sleep=lambda seconds: None)

    assert run("say", "alpha", long_text) == 0
    assert run("say", "alpha", "bad \x1b[31m text") == 2
    assert run("model", "alpha", "sonnet") == 0
    assert run("model", "alpha", "a b") == 2
    assert run("say", "nolane", "hi") == 3
    rows = [json.loads(line) for line in log.read_text().splitlines()]
    assert [(r["kind"], r["name"], r["outcome"]) for r in rows] == [
        ("say", "alpha", "ok"), ("say", "alpha", "refused"), ("model", "alpha", "ok"),
        ("model", "alpha", "refused"), ("say", "nolane", "refused")]
    assert {r["actor"] for r in rows} == {"overlay:alice@example.com:nodeABC"}
    assert f"{len(long_text)} characters" in rows[0]["detail"] and long_text[:80] in rows[0]["detail"]
    assert long_text[:81] not in rows[0]["detail"]
    assert "\x1b" not in log.read_text() and "control character" in rows[1]["detail"]
    assert "sent, not confirmed" in rows[2]["detail"]


@needs_tmux
def test_status_command_prints_table_and_json(world, capsys):
    group, _ = ready_lane(world)
    config_path = world.tmp / "lanes.toml"
    config_path.write_text(
        f'[work]\nrepo = "{group.repo}"\nsession = "{SESSION}"\nworktree_root = "{group.worktree_root}"\n'
        f'branch = "feat/{{name}}"\nbase = "main"\nsetup = ["true"]\nup = "true"\ndown = "true"\n'
        f'status = "true"\nagent = "{group.agent}"\n')

    def run(*argv):
        return cli.main(list(argv), client=world.client, config_path=config_path, events_path=world.tmp / "e.jsonl",
                        sources=world.sources, gh=lambda argv, cwd: __import__("subprocess").CompletedProcess(
                            argv, 1, stdout="", stderr=""))

    assert run("status") == 0
    assert "alpha" in capsys.readouterr().out
    assert run("status", "--json") == 0
    rows = json.loads(capsys.readouterr().out)
    assert [(row["name"], row["state"], row["tool"], row["session_id"]) for row in rows] == [
        ("alpha", "waiting", "claude", SESSION_ID)]
    assert not (world.tmp / "e.jsonl").exists()
