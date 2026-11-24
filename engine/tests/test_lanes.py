from __future__ import annotations

import json
import subprocess
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest
from conftest import git as fixture_git
from test_lanes_identity import make_group
from tmux_support import TMUX_PROGRAM, needs_tmux, private_tmux

from sakura.lanes import cli, identity, lane, worktrees


SESSION = "lanes-test"
GIT_IDENTITY = ["-c", "commit.gpgsign=false", "-c", "user.name=t", "-c", "user.email=t@example.com"]


def git(cwd: Path, *args: str) -> str:
    return fixture_git(cwd, *GIT_IDENTITY, *args)


def write_script(path: Path, log: Path) -> Path:
    path.write_text(f'#!/bin/sh\necho "$PWD" >> {log}\n')
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
    setup_log, down_log = tmp_path / "setup.log", tmp_path / "down.log"
    group = make_group(
        tmp_path, repo=repo, session=SESSION, branch="feat/{name}", base="main", agent="sleep 300", up="sleep 300",
        worktree_root=tmp_path / "trees", setup=(str(write_script(tmp_path / "setup.sh", setup_log)),),
        down=str(write_script(tmp_path / "down.sh", down_log)))
    with private_tmux() as client:
        yield SimpleNamespace(repo=repo, group=group, client=client, tmp=tmp_path,
                              setup_log=setup_log, down_log=down_log)


def start_session(world) -> None:
    world.client.ensure_session(SESSION, world.tmp, "base", ["sleep", "300"], {}, ("@sakura-test", "1"))


def lane_window(world, name: str):
    return next((w for w in world.client.windows(SESSION) if w.name == name), None)


def open_lane(world, name: str = "alpha", from_ref: str | None = None):
    lane_id = identity.resolve(world.group, name)
    lane.open(lane_id, world.group, from_ref, world.client)
    return lane_id


def calls(log: Path) -> int:
    return len(log.read_text().splitlines()) if log.exists() else 0


def push_lane(world, lane_id) -> None:
    remote = world.tmp / "remote.git"
    subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
    git(world.repo, "remote", "add", "origin", str(remote))
    git(lane_id.worktree, "push", "-q", "-u", "origin", lane_id.branch)


def commit_in(lane_id, name: str = "work") -> None:
    (lane_id.worktree / name).write_text("w\n")
    git(lane_id.worktree, "add", name)
    git(lane_id.worktree, "commit", "-q", "-m", name)


@needs_tmux
def test_open_creates_marked_split_window(world):
    start_session(world)
    lane_id = open_lane(world)
    window = lane_window(world, "alpha")
    assert window.options == {"@sakura-lane": "work/alpha"}
    assert sorted(pane.marker for pane in window.panes) == ["agent", "stack"]
    assert lane_id.worktree.is_dir() and git(lane_id.worktree, "branch", "--show-current") == "feat/alpha"
    assert world.setup_log.read_text().strip() == str(lane_id.worktree)


@needs_tmux
def test_open_twice_creates_nothing(world):
    start_session(world)
    open_lane(world)
    first = lane_window(world, "alpha")
    world.client.select_window(next(w.id for w in world.client.windows(SESSION) if w.name == "base"))
    open_lane(world)
    windows = [w for w in world.client.windows(SESSION) if w.name == "alpha"]
    assert [w.id for w in windows] == [first.id] and len(windows[0].panes) == 2
    assert calls(world.setup_log) == 1
    current = subprocess.run([TMUX_PROGRAM, "-L", world.client.socket, "-f", "/dev/null", "display-message", "-p", "-t",
                              f"={SESSION}:", "#{window_id}"], capture_output=True, text=True, check=True)
    assert current.stdout.strip() == first.id


@needs_tmux
def test_open_outside_tmux(world, monkeypatch, capsys):
    monkeypatch.delenv("TMUX", raising=False)
    assert not world.client.has_session(SESSION)
    code = cli.main(["open", "alpha"], client=world.client, config_path=write_group_config(world),
                    events_path=world.tmp / "events.jsonl")
    assert code == 0
    assert lane_window(world, "alpha").options == {"@sakura-lane": "work/alpha"}
    assert "attach-session" in capsys.readouterr().out


@needs_tmux
def test_open_refuses_unmarked_window_with_same_name(world):
    start_session(world)
    world.client.new_window(SESSION, "alpha", ["sleep", "300"], world.tmp, {})
    with pytest.raises(lane.LaneConflict, match="alpha"):
        open_lane(world)
    assert not (world.tmp / "trees" / "alpha").exists() and calls(world.setup_log) == 0


@needs_tmux
def test_open_refuses_option_like_from_ref_and_creates_nothing(world):
    start_session(world)
    with pytest.raises(lane.LaneError, match="from"):
        open_lane(world, from_ref="--detach")
    assert lane_window(world, "alpha") is None and not (world.tmp / "trees" / "alpha").exists()


@needs_tmux
def test_open_rolls_back_a_new_worktree_when_setup_fails(world):
    start_session(world)
    group = make_group(world.tmp, repo=world.repo, session=SESSION, base="main", agent="sleep 300", up="sleep 300",
                       worktree_root=world.tmp / "trees", setup=("false",), down="true")
    with pytest.raises(lane.LaneError, match="setup"):
        lane.open(identity.resolve(group, "alpha"), group, None, world.client)
    assert not (world.tmp / "trees" / "alpha").exists() and lane_window(world, "alpha") is None
    assert git(world.repo, "branch", "--list", "feat/alpha") == ""


def open_concurrently(world, count: int = 2) -> list[BaseException]:
    lane_id = identity.resolve(world.group, "alpha")
    barrier, failures = threading.Barrier(count), []

    def run() -> None:
        barrier.wait()
        try:
            lane.open(lane_id, world.group, None, world.client)
        except BaseException as error:  # noqa: BLE001 - the test reports whatever the open raised
            failures.append(error)

    threads = [threading.Thread(target=run) for _ in range(count)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return failures


@needs_tmux
@pytest.mark.parametrize("session_exists", [True, False])
def test_open_concurrent_same_lane_makes_one_marked_window_and_runs_setup_once(world, session_exists):
    if session_exists:
        start_session(world)
    assert open_concurrently(world) == []
    windows = [w for w in world.client.windows(SESSION) if w.name == "alpha"]
    assert len(windows) == 1 and windows[0].options == {"@sakura-lane": "work/alpha"}
    assert sorted(pane.marker for pane in windows[0].panes) == ["agent", "stack"]
    assert calls(world.setup_log) == 1


@needs_tmux
def test_open_after_the_window_is_gone_does_not_rerun_setup(world):
    start_session(world)
    open_lane(world)
    world.client.kill_window(lane_window(world, "alpha").id)
    open_lane(world)
    assert lane_window(world, "alpha") is not None and calls(world.setup_log) == 1


@needs_tmux
def test_open_reruns_setup_when_a_worktree_exists_without_the_setup_marker(world):
    start_session(world)
    lane_id = identity.resolve(world.group, "alpha")
    worktrees.create(lane_id, world.group, None)
    open_lane(world)
    assert calls(world.setup_log) == 1 and lane_window(world, "alpha") is not None


@needs_tmux
def test_open_after_an_interrupted_setup_reruns_setup(world, monkeypatch):
    start_session(world)
    lane_id = identity.resolve(world.group, "alpha")
    real = worktrees.run_command

    def killed(command, cwd, what):
        raise KeyboardInterrupt

    monkeypatch.setattr(worktrees, "run_command", killed)
    with pytest.raises(KeyboardInterrupt):
        lane.open(lane_id, world.group, None, world.client)
    assert not lane_id.worktree.exists()
    monkeypatch.setattr(worktrees, "run_command", real)
    open_lane(world)
    assert calls(world.setup_log) == 1 and lane_window(world, "alpha") is not None


@needs_tmux
def test_open_reruns_setup_after_one_that_failed_partway_in_a_kept_worktree(world):
    start_session(world)
    flaky = world.tmp / "flaky.sh"
    flaky.write_text(f'#!/bin/sh\necho x >> {world.setup_log}\n[ -e {world.tmp}/ok ]\n')
    flaky.chmod(0o755)
    group = make_group(world.tmp, repo=world.repo, session=SESSION, base="main", agent="sleep 300", up="sleep 300",
                       worktree_root=world.tmp / "trees", setup=(str(flaky),), down="true")
    lane_id = identity.resolve(group, "alpha")
    worktrees.create(lane_id, group, None)
    with pytest.raises(lane.LaneError, match="setup"):
        lane.open(lane_id, group, None, world.client)
    assert lane_id.worktree.is_dir()
    (world.tmp / "ok").write_text("")
    lane.open(lane_id, group, None, world.client)
    assert calls(world.setup_log) == 2
    lane.open(lane_id, group, None, world.client)
    assert calls(world.setup_log) == 2


@needs_tmux
def test_done_refuses_dirty_and_changes_nothing(world):
    start_session(world)
    lane_id = open_lane(world)
    (lane_id.worktree / "scratch").write_text("unsaved\n")
    with pytest.raises(lane.LaneBusy) as busy:
        lane.done(lane_id, world.group, False, world.client)
    assert any("uncommitted" in reason for reason in busy.value.reasons)
    assert calls(world.down_log) == 0
    assert lane_window(world, "alpha") is not None and lane_id.worktree.is_dir()


@needs_tmux
def test_done_refuses_commits_not_on_base_without_upstream(world):
    start_session(world)
    lane_id = open_lane(world)
    commit_in(lane_id)
    with pytest.raises(lane.LaneBusy) as busy:
        lane.done(lane_id, world.group, False, world.client)
    assert any("main" in reason for reason in busy.value.reasons)
    assert calls(world.down_log) == 0 and lane_window(world, "alpha") is not None


@needs_tmux
def test_done_refuses_commits_not_on_upstream(world):
    start_session(world)
    lane_id = open_lane(world)
    commit_in(lane_id, "one")
    push_lane(world, lane_id)
    lane.done(lane_id, world.group, False, world.client)
    open_lane(world)
    commit_in(lane_id, "two")
    with pytest.raises(lane.LaneBusy) as busy:
        lane.done(lane_id, world.group, False, world.client)
    assert any("origin/feat/alpha" in reason for reason in busy.value.reasons)


@needs_tmux
def test_done_new_lane_without_commits_closes(world):
    start_session(world)
    lane_id = open_lane(world)
    lane.done(lane_id, world.group, False, world.client)
    assert calls(world.down_log) == 1 and lane_window(world, "alpha") is None
    assert lane_id.worktree.is_dir()


@needs_tmux
def test_done_remove_requires_merged(world):
    start_session(world)
    lane_id = open_lane(world)
    commit_in(lane_id)
    push_lane(world, lane_id)
    with pytest.raises(lane.LaneBusy) as busy:
        lane.done(lane_id, world.group, True, world.client)
    assert any("not merged" in reason for reason in busy.value.reasons)
    assert calls(world.down_log) == 0 and lane_id.worktree.is_dir() and lane_window(world, "alpha") is not None
    git(world.repo, "merge", "-q", "--ff-only", lane_id.branch)
    lane.done(lane_id, world.group, True, world.client)
    assert not lane_id.worktree.exists() and lane_window(world, "alpha") is None


@needs_tmux
def test_done_calls_before_kill_while_the_window_still_exists(world):
    start_session(world)
    lane_id = open_lane(world)
    seen = []
    lane.done(lane_id, world.group, False, world.client, before_kill=lambda: seen.append(lane_window(world, "alpha")))
    assert seen[0] is not None and lane_window(world, "alpha") is None


@needs_tmux
def test_done_never_touches_an_unmarked_window(world):
    start_session(world)
    lane_id = open_lane(world)
    world.client.kill_window(lane_window(world, "alpha").id)
    world.client.new_window(SESSION, "alpha", ["sleep", "300"], world.tmp, {})
    lane.done(lane_id, world.group, False, world.client)
    assert lane_window(world, "alpha") is not None


def write_group_config(world) -> Path:
    group = world.group
    path = world.tmp / "lanes.toml"
    path.write_text(
        f'[work]\nrepo = "{group.repo}"\nsession = "{group.session}"\nworktree_root = "{group.worktree_root}"\n'
        f'branch = "feat/{{name}}"\nbase = "main"\nsetup = ["{group.setup[0]}"]\nup = "sleep 300"\n'
        f'down = "{group.down}"\nstatus = "true"\nagent = "sleep 300"\n')
    return path


def events(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines()]


@needs_tmux
def test_every_lane_command_logs_event_with_actor(world, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child: the only process that may carry a viewer's identity
    start_session(world)
    config_path, log = write_group_config(world), world.tmp / "events.jsonl"
    actor = ["--actor", "overlay:alice@example.com:nodeABC"]

    def run(*argv):
        return cli.main([*argv, *actor], client=world.client, config_path=config_path, events_path=log)

    assert run("open", "alpha") == 0
    assert run("open", "../x") != 0
    world.client.new_window(SESSION, "beta", ["sleep", "300"], world.tmp, {})
    assert run("open", "beta") != 0
    (world.tmp / "trees" / "alpha" / "scratch").write_text("x\n")
    assert run("done", "alpha") != 0
    (world.tmp / "trees" / "alpha" / "scratch").unlink()
    assert run("done", "alpha") == 0
    rows = events(log)
    assert [(row["kind"], row["name"], row["outcome"]) for row in rows] == [
        ("open", "alpha", "ok"), ("open", "../x", "refused"), ("open", "beta", "refused"),
        ("done", "alpha", "refused"), ("done", "alpha", "ok")]
    assert {row["actor"] for row in rows} == {"overlay:alice@example.com:nodeABC"}
    assert all(row["group"] == "work" for row in rows[2:]) and "uncommitted" in rows[3]["detail"]


def test_events_strip_controls_and_keep_recent_in_order(tmp_path):
    from sakura.lanes import events as lane_events

    log = tmp_path / "state" / "events.jsonl"
    for number in range(5):
        lane_events.append("open", "g", f"n{number}", "cli:me", "ok", "fine\x1b]2;title\x07", path=log)
    recent = lane_events.recent(3, path=log)
    assert [row["name"] for row in recent] == ["n2", "n3", "n4"]
    assert recent[0]["detail"] == "fine" and "\x1b" not in log.read_text()


def test_sakura_lane_dispatches_and_logs_refusal_under_home(tmp_path, monkeypatch, capsys):
    from sakura import cli as sakura_cli

    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("SAKURA_ACTOR", raising=False)
    assert sakura_cli.main(["lane", "open", "alpha"]) == 2
    assert "sakura lane is shelved" in capsys.readouterr().out
    assert not (tmp_path / ".local/state/sakura/lanes/events.jsonl").exists()
