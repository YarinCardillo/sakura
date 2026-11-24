import json
import os
import shutil
from datetime import datetime, timedelta
from pathlib import Path

import pytest
from sakura import engine_home
from sakura.cockpit import snapshot, text
from sakura.flow import contract, gates, inbox, progress, store, waits
from sakura.lanes import events as lane_events
from sakura.usage import aggregate
from test_lanes import git
from test_usage_aggregate import (
    CLAUDE_FIXTURE_TOTAL,
    CODEX_FIXTURE_TOTAL,
    CODEX_SESSION,
    FIXTURES,
    make_task,
    put_codex_rollout,
    run_record,
)
from test_usage_aggregate import NOW as USAGE_NOW

NIGHT = datetime(2026, 10, 26, 2, 0).astimezone()


class NoTmux:
    """A client for runs without lane groups: any use is a bug."""

    def __getattr__(self, name):
        raise AssertionError(f"tmux was asked for {name}")


def create_task(repo, task_id, **state):
    task = store.TaskStore(repo, task_id)
    base = {"task": task_id, "ref": f"specs/{task_id}.md", "phase": "build", "status": "running", "reason": "",
            "history": [], "approvals": {}, "completed": {}, "active_run": None, "overrides": {}}
    task.create({**base, **state})
    return task


def collect(repos, now=NIGHT, **kwargs):
    return snapshot.collect(repos, now, **kwargs)


def other_repo(tmp_path, name="other"):
    path = tmp_path / name
    git(tmp_path, "init", "-q", "-b", "main", str(path))
    (path / "README").write_text("x\n")
    git(path, "add", "README")
    git(path, "commit", "-q", "-m", "second repo")
    return path


def item_for(found, task_id, repo=None):
    return next(i for i in found.items if i.key[0] == "flow" and i.key[2] == task_id
                and (repo is None or i.key[1] == str(repo)))


def test_snapshot_same_task_id_two_repos_two_items(repo, tmp_path):
    second = other_repo(tmp_path)
    create_task(repo, "demo")
    create_task(second, "demo")

    found = collect([repo, second])

    flow_items = [i for i in found.items if i.kind == "flow"]
    assert sorted(i.key for i in flow_items) == sorted([("flow", str(repo), "demo"), ("flow", str(second), "demo")])
    assert {i.key[1]: i.repo for i in flow_items} == {str(repo): repo, str(second): second}


def test_Snapshot_TaskWithOwnerRow_ListsWaitAndProgress(repo):
    task = create_task(repo, "demo", phase="pr", build={"T1": {"status": "merged"}, "T2": {"status": "building"}})
    rows = [contract.Row("R1", "journey", "step", "cli", None, "look at it", False, False, "owner"),
            contract.Row("R2", "journey", "step", "cli", None, "true", False, True, "check")]
    contract.save(task, rows)
    task.plan_json.write_text(json.dumps({"tasks": [
        {"id": tid, "title": tid, "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False}
        for tid in ("T1", "T2")]}))

    found = collect([repo])

    assert [(wait.kind, wait.task) for wait in found.waits] == [("owner_row", "demo")]
    assert item_for(found, "demo").progress == progress.Progress(1, 4, "pr")


def test_snapshot_one_repository_through_two_worktrees_is_one_item(repo, tmp_path):
    create_task(repo, "demo")
    linked = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "side", str(linked))

    found = collect([repo, linked])

    assert [i.key for i in found.items] == [("flow", str(repo), "demo")]


def test_snapshot_key_is_the_main_worktree_whichever_worktree_registered_first(repo, tmp_path):
    create_task(repo, "demo")
    linked = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "side", str(linked))

    for order in ([repo, linked], [linked, repo]):
        found = collect(order)
        assert [(i.key, i.repo) for i in found.items] == [(("flow", str(repo), "demo"), repo)]


def test_a_broken_personal_flow_config_does_not_take_the_snapshot_down(repo):
    create_task(repo, "demo")
    broken = engine_home.config_file()
    broken.write_text("[gates]\nmode = \"sideways\"\n")

    found = collect([repo])

    assert found.mode in ("day", "night") and [i.key[2] for i in found.items] == ["demo"]
    assert len(found.problems) == 1
    assert "personal flow config" in found.problems[0] and "mode" in found.problems[0]
    assert "personal flow config" in "\n".join(text.render(found, 120, None))


def test_a_sound_config_has_no_problems(repo):
    assert collect([repo]).problems == []


def test_snapshot_skips_a_repository_that_is_gone(repo, tmp_path):
    create_task(repo, "demo")
    assert [i.key[2] for i in collect([repo, tmp_path / "gone"]).items] == ["demo"]


@pytest.mark.parametrize("state,runs,expected", [
    ({"status": "awaiting_approval"}, {}, "gate"),
    ({"status": "blocked"}, {}, "blocked"),
    ({"status": "failed"}, {}, "blocked"),
    ({"status": "running"}, {"build-1": {"name": "build-1", "state": "running"}}, "working"),
    ({"status": "running"}, {"build-1": {"name": "build-1", "state": "starting"}}, "working"),
    ({"status": "running"}, {"build-1": {"name": "build-1", "state": "finished"}}, "waiting"),
    ({"status": "stopped"}, {}, "stopped"),
    ({"status": "report_ready"}, {}, "unknown"),
    ({"status": "cleaned"}, {}, "cleaned"),
    ({"status": "awaiting_approval"}, {"build-1": {"name": "build-1", "state": "running"}}, "gate"),
])
def test_flow_agent_state(repo, state, runs, expected):
    create_task(repo, "demo", runs=runs, **state)
    assert item_for(collect([repo]), "demo").agent_state == expected


@pytest.mark.parametrize("status", ["report_ready", "cleaned"])
def test_Snapshot_FinishedLane_IsWaitingOnlyWhenAWaitIsListed(repo, status):
    create_task(repo, "demo", status=status, phase="cleanup")

    found = collect([repo])

    assert found.waits == []
    assert item_for(found, "demo").agent_state != "waiting"


def test_flow_item_facts(repo, tmp_path):
    work = tmp_path / "workspace"
    git(repo, "worktree", "add", "-q", "-b", "feat/flow-demo", str(work))
    (work / "NEW").write_text("n\n")
    git(work, "add", "NEW")
    git(work, "commit", "-q", "-m", "work in the workspace")
    task = create_task(repo, "demo", branch="feat/flow-demo", workspaces={"0": {"path": str(work)}},
                       pr={"url": "https://github.com/o/r/pull/5"}, active_stacks=["0"],
                       overrides={"roles": {"code_reviewer": "codex:gpt-5"}})
    inbox.enqueue(task, "steer", "mind the cache", actor="cli:me")
    inbox.enqueue(task, "dial", "strong", actor="cli:me")

    item = item_for(collect([repo]), "demo")

    assert (item.worktree, item.branch, item.pr, item.stack) == (work, "feat/flow-demo", "#5", "up")
    assert item.last_commit.endswith("work in the workspace")
    assert item.models["code_reviewer"] == "codex:gpt-5" and item.models["planner"] == "claude:opus"
    assert item.pending == ["steer (14 chars)", "dial strong"]
    assert item.next_command == "sakura flow resume demo"


def test_flow_item_without_workspace_or_branch_is_blank_not_an_error(repo):
    create_task(repo, "demo")
    item = item_for(collect([repo]), "demo")
    assert (item.worktree, item.last_commit, item.pr, item.stack, item.pending) == (None, "", "", "down", [])


def test_gate_item_offers_approval(repo):
    create_task(repo, "demo", status="awaiting_approval")
    assert item_for(collect([repo]), "demo").next_command == "sakura flow approve demo spec --continue"


def test_opus_gate_item_offers_the_approval_of_that_gate(repo):
    reason = "awaiting approval: sakura flow approve demo opus:T2 (T2 needs opus: 2 failed review rounds)"
    create_task(repo, "demo", status="awaiting_approval", reason=reason)
    assert item_for(collect([repo]), "demo").next_command == "sakura flow approve demo opus:T2 --continue"


def test_budget_gate_item_offers_the_approval_of_that_gate(repo):
    reason = "awaiting approval: sakura flow approve demo budget (about 8% of your week (4-15), 12% of the 5-hour window; x)"
    create_task(repo, "demo", status="awaiting_approval", reason=reason)
    assert item_for(collect([repo]), "demo").next_command == "sakura flow approve demo budget --continue"


def test_budget_wait_gate_item_offers_the_approval_of_that_gate(repo):
    reason = "awaiting approval: sakura flow approve demo budget-wait (week 80% reaches 75%)"
    create_task(repo, "demo", status="awaiting_approval", reason=reason)
    assert item_for(collect([repo]), "demo").next_command == "sakura flow approve demo budget-wait --continue"


def test_Item_TaskAtChecksGate_ApproveTargetsChecks(repo):
    reason = "awaiting approval: sakura flow approve demo checks (read the contract first)"
    create_task(repo, "demo", status="awaiting_approval", reason=reason)
    assert item_for(collect([repo]), "demo").next_command == "sakura flow approve demo checks --continue"


def test_unreadable_task_is_shown_blocked_with_the_reason(repo):
    create_task(repo, "good")
    broken = store.TaskStore(repo, "broken")
    broken.root.mkdir(parents=True)
    broken.state_file.write_text("{not json")
    found = collect([repo])
    assert [i.key[2] for i in found.items] == ["broken", "good"]
    broken = item_for(found, "broken")
    assert broken.agent_state == "blocked" and "cannot read" in broken.pending[0]
    assert broken.next_command == "sakura flow status broken"


def test_snapshot_carries_mode_deferred_and_load(repo):
    gates.default_deferred_dir().mkdir(parents=True)
    entry = {"at": (NIGHT - timedelta(minutes=30)).isoformat(), "repo": "r", "task": "demo",
             "event": "awaiting_approval", "message": "spec ready"}
    (gates.default_deferred_dir() / "1-a.json").write_text(json.dumps(entry))

    found = collect([repo])

    assert found.mode == "night"
    assert found.deferred == [entry]
    assert set(found.load) >= {"slots_total", "slots_in_use", "queue", "loadavg"}


def test_forced_day_overrides_the_clock(repo):
    gates.set_override(gates.default_mode_file(), "day")
    assert collect([repo]).mode == "day"


def test_events_are_the_latest_fifty_across_ledgers_and_the_lane_log(repo, tmp_path):
    second = other_repo(tmp_path)
    old = create_task(repo, "old")
    new = create_task(second, "new")
    for number in range(40):
        old.append_ledger("engine", f"old line {number}")
        new.append_ledger("engine", f"new line {number}")
    os.utime(old.ledger, (1_750_000_000, 1_750_000_000))
    os.utime(new.ledger, (1_760_000_000, 1_760_000_000))
    lane_events.append("open", "work", "alpha", "cli:me", "done")

    found = collect([repo, second])

    assert len(found.events) == 50
    assert found.events[-1]["source"] == "lane" and found.events[-1]["scope"] == "work/alpha"
    flow = [e for e in found.events if e["source"] == "flow"]
    assert flow[-1] == {"source": "flow", "scope": "other/new", "at": "", "text": "[engine] new line 39",
                        "kind": "engine", "words": None}
    assert not any(e["text"].startswith("[engine] old line 0") for e in found.events)


def test_snapshot_usage_is_per_item(repo, tmp_path, monkeypatch):
    sessions = tmp_path / "codex-home" / "sessions"
    put_codex_rollout(sessions)
    task = make_task(repo, "demo", {"plan-1": run_record("plan-1", "claude", "a1"),
                                    "build-1": run_record("build-1", "codex", CODEX_SESSION)})
    shutil.copy(FIXTURES / "claude_run.log", task.runs_dir / "plan-1.log")
    create_task(repo, "idle")
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / "codex-home"))

    found = collect([repo], now=USAGE_NOW)

    assert isinstance(found.usage, aggregate.WeekUsage)
    assert item_for(found, "demo").usage.total == CODEX_FIXTURE_TOTAL + CLAUDE_FIXTURE_TOTAL
    assert item_for(found, "idle").usage is None


def test_Snapshot_NoLaneItemsAndEmptyGroups(repo, tmp_path):
    """A configured lane group is ignored: the old tmux lanes are shelved, `groups` stays in the document."""
    from sakura.cockpit import cli
    from sakura.flow import registry

    create_task(repo, "demo")
    registry.remember(repo)
    lanes = tmp_path / "isolated-home" / ".config" / "sakura" / "lanes.toml"
    lanes.parent.mkdir(parents=True, exist_ok=True)
    lanes.write_text(f'[work]\nrepo = "{repo}"\nsession = "work"\nworktree_root = "{tmp_path / "trees"}"\n'
                     'branch = "feat/{name}"\nbase = "main"\nsetup = []\nup = "true"\ndown = "true"\n'
                     'status = "true"\nagent = "true"\n')

    found = cli._collect()

    assert [item.kind for item in found.items] == ["flow"] and found.groups == [] and found.problems == []


def test_snapshot_a_task_waiting_for_approval_is_a_gate(repo):
    create_task(repo, "demo", status="awaiting_approval")

    assert item_for(collect([repo]), "demo").agent_state == "gate"


def test_snapshot_projects_include_missing_registered_folders(repo, tmp_path):
    missing = tmp_path / "missing"
    found = collect([repo, missing])
    assert found.projects == [repo, missing]
    assert found.missing_projects == {missing}


# -- text.render --------------------------------------------------------------------------------


ESC = "\x1b"


def make_item(**over):
    fields = {"kind": "flow", "key": ("flow", "/r", "demo"), "repo": Path("/r"), "worktree": Path("/w"), "branch": "feat/x",
              "last_commit": "abc1234 subject", "stack": "up", "agent_state": "working", "pr": "#5",
              "models": {"builder": "claude:sonnet"}, "pending": [], "usage": None, "next_command": "sakura flow resume demo"}
    fields.update(over)
    return snapshot.Item(**fields)


def make_snapshot(**over):
    fields = {"mode": "day", "deferred": [], "load": {"slots_total": 4, "slots_in_use": 1, "queue": [], "loadavg": [1.0, 2.0, 3.0]},
              "usage": aggregate.WeekUsage(NIGHT, ()), "groups": [], "items": [], "events": []}
    fields.update(over)
    return snapshot.Snapshot(**fields)


def test_render_width_80_and_strips_hostile_branch_name():
    hostile = f"feat/{ESC}]2;pwned\x07x\nsecond line\t{ESC}[31mred{ESC}[0m" + "y" * 200
    item = make_item(branch=hostile, last_commit=f"{ESC}]52;c;AAAA\x07evil\r\nmore", pr=f"{ESC}[2J#5",
                     models={f"{ESC}[1mrole": f"model{ESC}"}, pending=[f"steer{ESC}[H (3 chars)\n"],
                     next_command="cmd\x00 with\x9b2J")
    found = make_snapshot(items=[item, make_item(key=("flow", "/r", "two"))],
                          events=[{"source": "flow", "scope": "demo", "at": "", "text": f"{ESC}]0;t\x07hi\nthere"}])

    lines = text.render(found, 80, item.key)

    assert lines and all(len(line) <= 80 for line in lines)
    joined = "\n".join(lines)
    assert not any(bad in joined for bad in (ESC, "\x07", "\x00", "\r", "\x9b", "\t", "pwned", "AAAA"))
    assert all("\n" not in line for line in lines)
    assert "feat/" in joined and "hi there" in joined


def test_render_lists_waits_and_progress_as_plain_text():
    wait = waits.Wait("[]", "owner_row", "/r", "repo", "demo", "Check row R1", f"look {ESC}[31mat it\n now", None)
    item = make_item(progress=progress.Progress(2, 5, "build"))

    lines = text.render(make_snapshot(waits=[wait], items=[item]), 100, None)

    assert lines.index("WAITING") < lines.index("ITEMS")
    assert any("owner_row" in line and "repo/demo" in line and "look at it now" in line for line in lines)
    assert any("2 of 5 done" in line for line in lines)
    assert not any(ESC in line for line in lines)


def test_render_marks_only_the_selected_item():
    one, two = make_item(), make_item(key=("flow", "/r", "two"), branch="feat/two")
    lines = text.render(make_snapshot(items=[one, two]), 80, two.key)
    marked = [line for line in lines if line.startswith(">")]
    assert len(marked) == 1 and "two" in marked[0]
    assert not any(line.startswith(">") for line in text.render(make_snapshot(items=[one, two]), 80, None))


def test_render_deferred_header_at_night():
    deferred = [{"at": "2026-10-26T01:30:00+02:00", "repo": "r", "task": "demo", "event": "awaiting_approval",
                 "message": "spec ready"},
                {"at": "2026-10-26T01:40:00+02:00", "repo": "r", "task": "other", "event": "failed",
                 "message": f"{ESC}[31mbuild failed"}]
    lines = text.render(make_snapshot(mode="night", deferred=deferred, items=[make_item()]), 80, None)
    head = "\n".join(lines[:6])
    assert "night" in head and "2 deferred" in head
    assert "demo" in head and "awaiting_approval" in head and "build failed" in head and ESC not in head
    day = "\n".join(text.render(make_snapshot(mode="day", items=[make_item()]), 80, None))
    assert "deferred" not in day and "mode: day" in day


def test_render_shows_group_rows_with_open_lane_hint():
    group = snapshot.GroupRow(name="work", repo=Path("/r"), session="work", lanes=2, problem="",
                              next_command="sakura lane open <name> --group work")
    joined = "\n".join(text.render(make_snapshot(groups=[group]), 80, None))
    assert "work" in joined and "2 lanes" in joined and "sakura lane open" in joined


def test_render_is_pure():
    found = make_snapshot(items=[make_item()])
    assert text.render(found, 80, None) == text.render(found, 80, None)


def test_snapshot_json_matches_the_original_bytes(repo, tmp_path, monkeypatch):
    from datetime import UTC

    from sakura.cockpit import api
    from sakura.flow import yield_person
    from sakura.outside import store as outside_store
    from sakura.outside.records import Agent

    now = datetime(2026, 11, 22, 12, tzinfo=UTC)
    digest_status = api.digest_run.status
    monkeypatch.setattr(api.digest_run, "status", lambda settings, _: digest_status(settings, now, tz=UTC))
    engine_home.config_file().write_text('[gates]\nmode = "day"\n')
    task = create_task(repo, "demo", status="awaiting_approval", phase="spec",
                       reason="awaiting approval: sakura flow approve demo spec",
                       **{yield_person.YIELD_KEY: True})
    os.utime(task.ledger, (now.timestamp(), now.timestamp()))
    inbox.enqueue(task, "pace", "slow", actor="cli:fixture")
    outside_store.open_lane(outside_store.path(), Agent("fixture", repo), "outside-demo",
                            title="Outside demo", about="Fixture lane", now=now)
    monkeypatch.setattr(snapshot, "_load", lambda settings: {"slots_total": 4, "slots_in_use": 0,
                                                            "queue": [], "loadavg": [0.0, 0.0, 0.0]})

    document = api.snapshot_document(collect([repo], now=now))
    assert document.pop("tracks") == []
    assert [action["key"] for action in document.pop("track_actions")] == ["tracknew"]
    for project in document["projects"]:
        assert project.pop("track_choices") == []
    for item in document["items"]:
        if item["kind"] == "flow":
            assert item.pop("track") is None
    actual = api.dumps(document).encode() + b"\n"
    expected = Path(__file__).with_name("fixtures") / "cockpit-snapshot-baseline.json"
    assert actual == expected.read_bytes().replace(b"<fixture>", os.fsencode(tmp_path))


def test_snapshot_authenticates_each_state_once_and_reads_it_again_on_refresh(repo, monkeypatch):
    from sakura.cockpit import api

    task = create_task(repo, "demo")
    original = store.TaskStore._read_state_locked
    reads = []

    def record(opened):
        reads.append(opened.task_id)
        return original(opened)

    monkeypatch.setattr(store.TaskStore, "_read_state_locked", record)
    first = api.snapshot_document(collect([repo]))
    assert reads == ["demo"]
    assert first["items"][0]["agent_state"] == "waiting"
    assert first["items"][0]["pace"] is not None

    task.state_file.write_text(task.state_file.read_text() + "\n")
    reads.clear()
    again = api.snapshot_document(collect([repo]))
    assert reads == ["demo"]
    assert again["items"][0]["agent_state"] == "blocked"
    assert again["items"][0]["pace"] is None


def test_snapshot_reads_each_outside_history_once(repo, monkeypatch):
    from sakura.outside import store as outside_store
    from sakura.outside.records import Agent

    root = outside_store.path()
    outside_store.open_lane(root, Agent("fixture", repo), "outside-demo", title="Outside demo", now=NIGHT)
    reads = []
    record, log = outside_store.read, outside_store.log

    def read_record(folder, lane):
        reads.append(("record", lane))
        return record(folder, lane)

    def read_log(folder, lane, **kwargs):
        reads.append(("log", lane))
        return log(folder, lane, **kwargs)

    monkeypatch.setattr(outside_store, "read", read_record)
    monkeypatch.setattr(outside_store, "log", read_log)
    found = collect([repo])
    assert found.items[0].title == "Outside demo"
    assert found.events[0]["kind"] == "opened"
    assert reads == [("record", "outside-demo"), ("log", "outside-demo")]


def test_snapshot_uses_one_inbox_listing_for_pending_and_pace(repo, monkeypatch):
    from sakura.cockpit import api

    task = create_task(repo, "demo")
    inbox.enqueue(task, "pace", "slow", actor="cli:fixture")
    original = inbox.pending
    reads = []

    def record(opened):
        reads.append(opened.task_id)
        return original(opened)

    monkeypatch.setattr(inbox, "pending", record)
    item = api.snapshot_document(collect([repo]))["items"][0]
    assert (item["pending"], item["pace"], item["pace_pending"]) == (["pace slow"], "slow", True)
    assert reads == ["demo"]


def test_snapshot_reuses_goals_ledgers_and_logs_until_the_files_change(repo, monkeypatch):
    import builtins
    from collections import Counter

    from sakura.outside import store as outside_store
    from sakura.outside.records import Agent

    task = create_task(repo, "demo")
    task.input.write_text("A fixture goal\n")
    task.ledger.write_text("- [operator] start requested actor=cli:fixture\n")
    root = outside_store.path()
    lane = "outside-demo"
    agent = Agent("fixture", repo)
    outside_store.open_lane(root, agent, lane, title="Outside demo", now=NIGHT)

    log = root / lane / outside_store.LOG_FILE
    reads = Counter()
    paths = {task.input, task.ledger, log}

    def track(open_file):
        def logged(path, *args, **kwargs):
            if isinstance(path, (str, Path)) and Path(path) in paths:
                reads[Path(path)] += 1
            return open_file(path, *args, **kwargs)
        return logged

    monkeypatch.setattr(Path, "open", track(Path.open))
    monkeypatch.setattr(builtins, "open", track(builtins.open))
    monkeypatch.setattr(os, "open", track(os.open))

    first, again = collect([repo]), collect([repo])
    assert first.items == again.items and first.events == again.events
    assert reads == Counter({path: 1 for path in paths})

    task.input.write_text("A newer goal\n")
    task.ledger.write_text("- [operator] start requested actor=master:fixture\n")
    outside_store.open_lane(root, agent, lane, title="New outside title", now=NIGHT)
    refreshed = collect([repo])
    row = item_for(refreshed, "demo")
    assert row.facts["goal"] == "A newer goal\n"
    assert row.facts["opened_by"] == "master"
    assert any(event["kind"] == "changed" for event in refreshed.events)
