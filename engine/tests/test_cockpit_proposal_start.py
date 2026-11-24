import contextlib
import io
import subprocess
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime

import pytest
from sakura.actors import Actor
from sakura.cockpit import api, snapshot
from sakura.cockpit.controller import Controller, Done, Refused
from sakura.flow import cli, exits, store
from test_cockpit_digest_reading import _published

DESKTOP = Actor("app", "owner")
PHONE = Actor("overlay", "owner", "phone")


@pytest.fixture
def world(repo, monkeypatch):
    _published()
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: None)
    monkeypatch.setattr(cli, "_detach_resume", lambda *args, **kwargs: exits.OK)

    def fresh():
        return snapshot.collect([repo], datetime.now(UTC))

    def run(argv, *, cwd, env, **kwargs):
        assert cwd == repo and argv[1:3] == ["flow", "start"]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = cli.main(argv[2:])
        return subprocess.CompletedProcess(argv, code, output.getvalue(), "")

    controller = Controller(b"test", fresh, fresh, run=run)
    [proposal] = [wait for wait in fresh().waits if wait.title == "Check PR 12"]
    return proposal, controller, fresh


def _act(world, key, actor=DESKTOP, **given):
    proposal, controller, _ = world
    fields = {"key": key, "target": proposal.id, **{f"input.{name}": value for name, value in given.items()}}
    return controller.perform(fields, actor, proof_field="confirm", proof_source=actor.source)


def _lane(result):
    assert isinstance(result, Done), result
    assert result.result.exit_code == 0, result.result.output
    return result.result.output


def test_start_opens_one_lane(repo, world):
    lane = _lane(_act(world, "start", op="inbox-1", project=str(repo), title="Inspect CI"))
    assert [task.task_id for task in store.task_stores(repo)] == [lane]
    assert store.TaskStore(repo, lane).read_state()["title"] == "Inspect CI"
    current = world[2]()
    assert world[0].id not in {wait.id for wait in current.waits}
    [entry] = current.decided
    assert (entry["answer"], entry["lane"], entry["place"]) == ("started", lane, "desktop")
    reading = next(item.facts["reading"] for item in current.items if item.kind == "digest")
    assert (reading["asks"][0]["state"], reading["asks"][0]["id"]) == ("decided", None)


def test_same_op_twice_one_lane(repo, world):
    first = _lane(_act(world, "start", op="same"))
    assert _lane(_act(world, "start", PHONE, op="same")) == first
    assert len(store.task_stores(repo)) == 1
    assert len(world[2]().decided) == 1


@pytest.mark.parametrize("second_op", ["shared", "another-client-op"])
def test_concurrent_start_one_lane(repo, world, second_op):
    with ThreadPoolExecutor(2) as callers:
        first = callers.submit(_act, world, "start", DESKTOP, op="shared")
        second = callers.submit(_act, world, "start", PHONE, op=second_op)
        assert _lane(first.result()) == _lane(second.result())
    assert len(store.task_stores(repo)) == 1
    assert len(world[2]().decided) == 1


def test_dismiss_resolves_without_lane(repo, world):
    result = _act(world, "dismiss", PHONE)
    assert isinstance(result, Done) and result.result.exit_code == 0
    assert store.task_stores(repo) == []
    assert world[0].id not in {wait.id for wait in world[2]().waits}
    [entry] = world[2]().decided
    assert (entry["answer"], entry["place"], entry["lane"]) == ("dismissed", "phone", None)


def test_start_refused_without_project_says_so(world):
    proposal = world[0]

    def fresh():
        return snapshot.collect([], datetime.now(UTC))

    result = _act((proposal, Controller(b"test", fresh, fresh), fresh), "start", op="no-project")
    assert isinstance(result, Refused)
    assert "project" in result.reason.lower()
    assert fresh().decided == []


def test_snapshot_offers_shared_proposal_inputs(world):
    row = next(row for row in api.snapshot_document(world[2]())["waits"] if row["id"] == world[0].id)
    assert row["proposal"]["op"]
    assert row["proposal"]["project"] == str(world[2]().projects[0])
    [start] = [action for action in row["actions"] if action["key"] == "start"]
    assert start["needs"] == ["op"] and start["optional"] == ["project", "preset", "account", "title"]


def test_retry_after_uncertain_start_reuses_the_created_lane(repo, world, monkeypatch):
    def detach(cfg, task, actor, **kwargs):
        return exits.FAILED

    monkeypatch.setattr(cli, "_detach_resume", detach)
    result = _act(world, "start", op="retry")
    assert isinstance(result, Done) and result.result.exit_code == exits.FAILED
    [created] = store.task_stores(repo)
    monkeypatch.setattr(cli, "_detach_resume", lambda *args, **kwargs: exits.OK)
    assert _lane(_act(world, "start", PHONE, op="retry")) == created.task_id
    assert len(store.task_stores(repo)) == 1


def test_phone_start_uses_project_preset_and_account(repo, world):
    from sakura.flow import config

    chosen = config.load(repo).values["preset"]
    assert chosen != "solo"
    lane = _lane(_act(world, "start", PHONE, op="phone-preset", preset="solo", account="account-override"))
    overrides = store.TaskStore(repo, lane).read_state()["overrides"]
    assert "preset" not in overrides and "preset" not in overrides.get("relax", [])
    assert "budget.account" not in overrides and "account-override" not in str(overrides)


@pytest.mark.parametrize("key", ["start", "dismiss"])
def test_proposal_changed_after_lookup_cannot_be_resolved(repo, world, monkeypatch, key):
    from sakura.cockpit import proposal_actions
    from sakura.outside import store as outside_store
    from test_cockpit_digest_reading import NOW

    proposal = world[0]
    root = outside_store.path()
    lane = outside_store.read(root, proposal.task)
    original = getattr(proposal_actions, key)

    def race(*args, **kwargs):
        outside_store.put_row(root, lane.agent, lane.id, "check-pr", title="Replacement", steps=["New work"], now=NOW)
        return original(*args, **kwargs)

    monkeypatch.setattr(proposal_actions, key, race)
    result = _act(world, key, **({"op": "stale"} if key == "start" else {}))
    assert isinstance(result, Refused) and "no longer waiting" in result.reason
    assert not store.task_stores(repo) and not world[2]().decided


@pytest.mark.parametrize(("key", "kind", "given"), [
    ("k", "owner_row", {"verdict": "ok"}),
    ("r", "question", {"choice": "yes"}),
])
def test_legacy_answers_do_not_resolve_triage(world, key, kind, given):
    wait = next(wait for wait in world[2]().waits if wait.kind == kind)
    result = _act((wait, world[1], world[2]), key, **given)
    assert isinstance(result, Refused)
    assert wait.id in {current.id for current in world[2]().waits}


def test_failed_dismissal_write_leaves_proposal_open_and_done_empty(world, monkeypatch):
    from sakura.cockpit import proposal_actions

    def fail(*args):
        raise OSError("disk full")

    with monkeypatch.context() as patch:
        patch.setattr(proposal_actions.write, "replace_atomically", fail)
        result = _act(world, "dismiss")
    assert isinstance(result, Done) and result.result.exit_code == exits.FAILED
    assert world[0].id in {wait.id for wait in world[2]().waits}
    assert world[2]().decided == []
    assert _act(world, "dismiss").result.exit_code == exits.OK
    assert len(world[2]().decided) == 1


def test_proposal_done_does_not_depend_on_the_decided_log(world, monkeypatch):
    from sakura.cockpit import decided

    def fail(*args, **kwargs):
        raise OSError("log unavailable")

    monkeypatch.setattr(decided.appendlog, "append", fail)
    assert _act(world, "dismiss").result.exit_code == exits.OK
    [answer] = world[2]().decided
    assert (answer["id"], answer["answer"], answer["actor"]) == (world[0].id, "dismissed", "app:owner")


def test_terminal_proposal_stays_resolved_if_the_digest_file_is_unavailable(world):
    from sakura.digest import config

    assert _act(world, "dismiss").result.exit_code == exits.OK
    (config.digest_folder() / "2026-11-17.json").unlink()
    current = world[2]()
    assert world[0].id not in {wait.id for wait in current.waits}
    assert [entry["id"] for entry in current.decided] == [world[0].id]


def test_proposal_source_and_operator_brief_are_stored_separately(repo, world):
    lane = _lane(_act(world, "start", op="source-context", title="Inspect CI"))
    task = store.TaskStore(repo, lane)
    proposal = world[0]
    assert task.input.read_text() == "Inspect CI\n"
    context = task.read_state()["proposal_context"]
    assert context == {"source": proposal.source, "author": proposal.author, "title": proposal.title,
                       "detail": proposal.detail}


def test_untracked_l_keeps_created_id_and_does_not_reserve_a_track(repo, world):
    from sakura import engine_home

    _, controller, _ = world
    result = controller.perform_allowed("L", ("project", str(repo)), {"text": "Ordinary lane"}, DESKTOP, {"L"})
    lane = _lane(result)
    assert [task.task_id for task in store.task_stores(repo)] == [lane]
    assert lane == "ordinary-lane"
    assert not (engine_home.root() / "tracks" / "tracks.db").exists()
    assert world[2]().decided == []


def test_legacy_pending_proposal_start_keeps_its_marker(repo, world):
    from sakura.flow import tasks

    lane = _lane(_act(world, "start", op="legacy"))
    task = store.TaskStore(repo, lane)
    assert task.read_state()[tasks.INBOX_START_KEY] == lane
    assert cli.main(["start", "--detach", "--new", f"--inbox-op={lane}", "--message=Same legacy path"]) == exits.OK
    assert [item.task_id for item in store.task_stores(repo)] == [lane]
    assert len(world[2]().decided) == 1
