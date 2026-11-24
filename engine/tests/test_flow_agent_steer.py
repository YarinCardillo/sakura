"""`sakura flow steer <task> --run <run> <text> --json`: a message to one agent, and what came of it."""

import json
import time
from types import SimpleNamespace

import pytest

from flow_run_support import INIT, add_run, make_task
from sakura.flow import agent_resume, agent_steer, budget_guard, cli, exits, inbox, live_input, store

OUTCOME_KEYS = {"outcome", "run", "words"}


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(inbox, "LIVE_WAIT_SECONDS", 0.5)
    created = make_task(repo, tmp_path)
    created.spawned = []
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held_store, run, begun=False: created.spawned.append((run, begun)))
    return created


def state_of(task):
    """The task state as another process left it: a store of its own reads it."""
    return store.TaskStore(task.repo, "demo").read_state()


def steer(capsys, run="build-1", text="hello", *extra):
    capsys.readouterr()
    assert cli.main(["steer", "--actor=cli:person", "demo", "--run", run, text, "--json", *extra]) == exits.OK
    return json.loads(capsys.readouterr().out)


class Taker:
    """Polls one role's channel and records what that role was sent."""

    def __init__(self, store, repo, run, tmp):
        self.store, self.repo, self.run = store, repo, run
        self.channel = live_input.LiveChannel()
        self.sink = tmp / f"sink-{run}"
        self.channel.attach(self.sink.open("wb"), "the prompt")

    def poll(self):
        inbox.take_live(self.store, self.repo, self.channel, self.run, "builder")

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.channel.finish()
        self.channel._writer.join(timeout=10)
        assert not self.channel._writer.is_alive()

    def received(self):
        return self.sink.read_text()


def take_on_poll(monkeypatch, *takers):
    elapsed = 0

    def poll(seconds):
        nonlocal elapsed
        elapsed += seconds
        for taker in takers:
            taker.poll()

    monkeypatch.setattr(inbox, "time", SimpleNamespace(
        monotonic=lambda: elapsed, time_ns=time.time_ns, sleep=poll,
    ))


def test_Steer_ToALiveRun_IsDeliveredToThatRunOnly(task, repo, tmp_path, capsys, monkeypatch):
    add_run(task, "build-1", INIT, state="running")
    add_run(task, "build-2", INIT, state="running")
    inbox.mark_live(task, "build-1", "builder")
    inbox.mark_live(task, "build-2", "builder")
    with Taker(task, repo, "build-1", tmp_path) as first, Taker(task, repo, "build-2", tmp_path) as second:
        take_on_poll(monkeypatch, first, second)
        outcome = steer(capsys, "build-2", "use short names")

    assert set(outcome) == OUTCOME_KEYS and outcome["outcome"] == "delivered" and outcome["run"] == "build-2"
    assert outcome["words"] and "use short names" in second.received() and "use short names" not in first.received()


def test_Steer_ToARunningRunWithoutALiveChannel_IsQueuedForWhenItFinishes(task, capsys):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")

    outcome = steer(capsys, "review-1", "explain finding 2")

    assert set(outcome) == OUTCOME_KEYS and (outcome["outcome"], outcome["run"]) == ("queued", "review-1")
    assert "code reviewer" in outcome["words"] and "finishes" in outcome["words"]
    held = state_of(task)["agent_queue"]
    assert [(entry["run"], entry["text"], entry["actor"]) for entry in held] == [("review-1", "explain finding 2", "cli:person")]
    assert not state_of(task).get("steer_pending") and not list(task.inbox_dir.glob("*.json"))
    assert task.spawned == [("review-1", False)], "a worker waits for the run to end and then continues it"


def test_Steer_ToALiveRunThatDoesNotTakeItInTime_IsWithdrawnAndQueued(task, capsys):
    add_run(task, "build-1", INIT, state="running")
    inbox.mark_live(task, "build-1", "builder")

    outcome = steer(capsys, "build-1", "hello")

    assert outcome["outcome"] == "queued" and outcome["run"] == "build-1"
    assert not list(task.inbox_dir.glob("*.json")), "no copy is left for a later role to read"
    assert [entry["text"] for entry in state_of(task)["agent_queue"]] == ["hello"]


def test_Steer_ToARunThatCannotBeWrittenTo_IsRefusedInWords(task, capsys):
    add_run(task, "plan-1", "", provider="exec", role="planner")

    outcome = steer(capsys, "plan-1")

    assert set(outcome) == OUTCOME_KEYS and outcome["outcome"] == "refused" and outcome["run"] == "plan-1"
    assert "not an agent" in outcome["words"]
    assert not state_of(task).get("agent_queue")


def test_Steer_ToAnUnknownRun_IsRefusedInWords(task, capsys):
    outcome = steer(capsys, "build-9")

    assert outcome["outcome"] == "refused" and "no run build-9" in outcome["words"].lower()


def test_Steer_WithoutRun_BehavesAsBeforeAndWritesNoJson(task, capsys):
    capsys.readouterr()

    assert cli.main(["steer", "--actor=cli:person", "demo", "hello"]) == exits.OK

    assert capsys.readouterr().out.startswith("queued: steer for demo")
    assert not state_of(task).get("agent_queue")


def test_Steer_BlankTextToARun_IsAUsageError(task, capsys):
    add_run(task, "build-1", INIT, state="running")

    assert cli.main(["steer", "demo", "--run", "build-1", "   ", "--json"]) == exits.USAGE


# -- the lane's own long-lived store stays valid while a chat writes

def test_Steer_ResumingAFinishedAgentWhileTheLanesStoreIsAlive_DoesNotTripTheLanesTamperCheck(task, capsys, monkeypatch):
    from flow_run_support import assistant, text
    from sakura import repository

    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held, run, begun=False: None)
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    add_run(task, "build-1", INIT + assistant(text("Done.")))
    from test_flow_agent_resume import put_transcript
    put_transcript(task.workspace)
    task.update_state(lambda state: state.update(phase="review"))  # the lane's engine has written, and keeps its store

    outcome = steer(capsys, "build-1", "go on")

    assert outcome["outcome"] == "resumed", outcome["words"]
    state = task.read_state()  # the lane's own store: must not raise "changed outside the engine"
    assert "build-1.2" in state["runs"]
    task.update_state(lambda current: current.update(phase="evaluate"))
    assert "build-1.2" in task.read_state()["runs"], "the lane's next write keeps the continuation"
    assert repository.common_dir(task.repo)


def test_AgentWorker_DeclaresItselfASideWriter(task, monkeypatch):
    seen = []
    monkeypatch.setattr(cli.engine, "open_context", lambda repo, opened: seen.append(opened) or SimpleNamespace(store=opened))
    monkeypatch.setattr(agent_resume, "run_begun", lambda ctx, name: True)
    monkeypatch.setattr(agent_resume, "work", lambda ctx, run: None)

    assert cli.main(["agent-worker", "demo", "build-1", "--begun"]) == exits.OK
    assert seen[0].is_side_writer


def test_AgentSteer_DeclaresItselfASideWriter(task, capsys, monkeypatch):
    seen = []
    real = agent_steer.send
    monkeypatch.setattr(agent_steer, "send", lambda opened, *args, **kwargs: (seen.append(opened), real(opened, *args, **kwargs))[1])

    steer(capsys, "build-9")

    assert seen[0].is_side_writer


# -- the actor of a steer from the app

def steer_as(capsys, actor_args, run="review-1", text="explain"):
    capsys.readouterr()
    assert cli.main(["steer", *actor_args, "demo", "--run", run, text, "--json"]) == exits.OK
    return json.loads(capsys.readouterr().out)


def test_Steer_TheActorFlagOfTheAppIsKeptOnTheHeldMessageAndInTheLedger(task, capsys):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")

    outcome = steer_as(capsys, ["--actor=app:me"])

    assert outcome["outcome"] == "queued"
    assert [entry["actor"] for entry in state_of(task)["agent_queue"]] == ["app:me"]
    assert "steer to review-1 queued actor=app:me" in task.ledger.read_text()


def test_Steer_TheActorVariableOfTheAppIsUsedWhenThereIsNoFlag(task, capsys, monkeypatch):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    monkeypatch.setenv("SAKURA_ACTOR", "app:me")

    steer_as(capsys, [])

    assert [entry["actor"] for entry in state_of(task)["agent_queue"]] == ["app:me"]
    assert "actor=app:me" in task.ledger.read_text()


def test_Steer_WithoutAnActorTheCommandLineUserIsTheActor(task, capsys, monkeypatch):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    monkeypatch.delenv("SAKURA_ACTOR", raising=False)

    steer_as(capsys, [])

    assert state_of(task)["agent_queue"][0]["actor"].startswith("cli:")


def test_Steer_ALiveSteerFromTheAppIsLedgeredWithTheApp(task, capsys, tmp_path, repo, monkeypatch):
    add_run(task, "build-1", INIT, state="running")
    inbox.mark_live(task, "build-1", "builder")
    with Taker(task, repo, "build-1", tmp_path) as taker:
        take_on_poll(monkeypatch, taker)
        outcome = steer_as(capsys, ["--actor=app:me"], "build-1", "hello")

    assert outcome["outcome"] == "delivered"
    assert "steer delivered live" in task.ledger.read_text() and "actor=app:me" in task.ledger.read_text()
