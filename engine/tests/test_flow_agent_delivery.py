"""No message accepted for an agent is lost or stranded: it is delivered live, or held until a worker continues the agent."""

import os
import threading
from types import SimpleNamespace

import pytest

from flow_run_support import add_run, make_task
from flow_support import make_context, task_workspace
from sakura import repository
from sakura.flow import (agent_messages, agent_resume, agent_steer, budget_guard, config, inbox, process, run_records,
                         run_view, runner, store)
from test_flow_agent_resume import FakeContext, finished_builder


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    monkeypatch.setattr(inbox, "LIVE_WAIT_SECONDS", 0.2)
    created = make_task(repo, tmp_path)
    created.spawned = []
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held, run, begun=False: created.spawned.append((run, begun)))
    return created


def fresh(task):
    return store.TaskStore(task.repo, task.task_id)


def send(task, words="go on", run="build-1"):
    cfg = config.load(task.repo, overrides={"roles": {"code_reviewer": "codex"}})
    return agent_steer.send(fresh(task), cfg, repository.common_dir(task.repo), run, words, "cli:person")


# -- a live message the run never replayed

class DyingLauncher:
    """A launcher whose run takes a steer into its channel and ends without ever replaying it."""

    host = "local"

    def __init__(self, store_):
        self.store = store_

    def run(self, request, on_start):
        on_start({"pid": os.getpid(), "command": process.command_of(os.getpid())})
        request.live.attach(open(os.devnull, "wb"), request.stdin_text)
        inbox.enqueue(self.store, "steer", "a word the run never read", "", "cli:person", run=request.log_path.stem)
        request.on_poll()
        return process.ProcessOutcome(0, False, False, False, os.getpid())

    def is_alive(self, identity):
        return False

    def kill(self, identity):
        return None


def test_Shutdown_ALiveSteerAcceptedButNeverReplayed_IsHeldAndAWorkerIsScheduled(repo, tmp_path, monkeypatch):
    spawned = []
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held, run, begun=False: spawned.append(run))
    ctx = make_context(repo, overrides={"roles": {"builder": "claude:sonnet"}})
    agents = runner.AgentRunner(ctx.store, ctx.config, launcher=DyingLauncher(ctx.store))
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)

    agents.run(runner.RunRequest("builder", "build", "prompt", {}, task_workspace(ctx)))

    state = ctx.store.read_state()
    assert [(entry["run"], entry["text"]) for entry in state["agent_queue"]] == [("build-1", "a word the run never read")]
    assert spawned == ["build-1"], "something will deliver it"


# -- the worker and the sender cannot miss each other

def test_Worker_AMessageHeldAsTheWorkerFindsNothingLeft_IsStillDelivered(task, monkeypatch):
    finished_builder(task)
    requests, threads = [], []
    context = FakeContext(fresh(task), requests)
    task_store = context.store

    def wake(held, run, begun=False):
        thread = threading.Thread(target=agent_resume.work, args=(context, run))
        threads.append(thread)
        thread.start()
        thread.join(2)  # a worker that finds the place taken has given up by the time the sender goes on

    monkeypatch.setattr(agent_resume, "spawn_worker", wake)
    sender_started = []
    real_held_for = agent_messages.held_for

    def held_for(state, runs):
        found = real_held_for(state, runs)
        if not found and not sender_started:  # the worker is about to conclude that nothing is left
            sender_started.append(threading.Thread(target=agent_resume.hold_for, args=(task_store, "build-1", "late", "cli:person")))
            sender_started[0].start()
            sender_started[0].join(2)  # a sender that is not held back finishes (and fails to wake anyone) here
        return found

    monkeypatch.setattr(agent_messages, "held_for", held_for)

    agent_resume.work(context, "build-1")
    sender_started[0].join(10)
    for thread in threads:
        thread.join(10)

    assert [request.template for request in requests] == ["late"]
    assert not agent_messages.held_for(task_store.read_state(), {"build-1", "build-1.2"})


# -- a start that fails keeps the message and the conversation

def pending_of(task, name):
    return fresh(task).read_state()["runs"][name].get("pending_text")


def test_Start_ThePendingMessageAndItsSessionAreInTheRecordUntilTheRunLaunches(task):
    finished_builder(task)

    outcome = send(task, "the message")

    record = fresh(task).read_state()["runs"][outcome.run]
    assert record["pending_text"] == "the message" and record["session_id"] == finished_session(task)
    assert task.spawned == [("build-1.2", True)]


def finished_session(task):
    return fresh(task).read_state()["runs"]["build-1"]["session_id"]


def test_Start_ALaunchThatSucceeds_ClearsThePendingMessage(task):
    finished_builder(task)
    outcome = send(task, "the message")
    context = FakeContext(fresh(task), [])

    agent_resume.run_begun(context, outcome.run)

    assert pending_of(task, outcome.run) is None


def test_Start_TheGuardRefusesAtLaunch_TheMessageIsHeldAgainAndTheChainStaysResumable(task, monkeypatch):
    finished_builder(task)
    outcome = send(task, "the first message")
    decision = SimpleNamespace(reason="The account is at its reserve.", action="wait")
    context = FakeContext(fresh(task), [])
    context.runner = SimpleNamespace(run=lambda request: (_ for _ in ()).throw(budget_guard.BudgetWait(decision)))

    agent_resume.run_begun(context, outcome.run)

    state = fresh(task).read_state()
    assert state["runs"][outcome.run]["state"] == "aborted"
    assert [entry["text"] for entry in state["agent_queue"]] == ["the first message"]
    again = send(task, "the second message")
    assert (again.outcome, again.run) == ("resumed", "build-1.3")
    assert fresh(task).read_state()["runs"]["build-1.3"]["pending_text"] == "the first message\n\nthe second message"


def test_Start_AWorkerKilledBeforeItLaunched_LeavesTheMessageForTheNextResume(task, monkeypatch):
    finished_builder(task)
    first = send(task, "never launched")
    monkeypatch.setattr(run_view, "STARTING_GRACE_SECONDS", -1)  # the grace of a start that never launched is over

    again = send(task, "and then this")

    assert (again.outcome, again.run) == ("resumed", "build-1.3")
    new = fresh(task).read_state()["runs"]["build-1.3"]
    assert new["pending_text"] == "never launched\n\nand then this" and new["session_id"] == finished_session(task)
    assert pending_of(task, first.run) is None, "taken over by the new run, once"


def test_Start_ProcessCannotBeSpawned_IsRefusedInWordsAndNothingIsLost(task, monkeypatch):
    finished_builder(task)

    def fail(held, run, begun=False):
        raise OSError("no more processes")

    monkeypatch.setattr(agent_resume, "spawn_worker", fail)

    outcome = send(task, "keep me")

    assert outcome.outcome == "refused" and "could not" in outcome.words.lower()
    state = fresh(task).read_state()
    assert [entry["text"] for entry in state["agent_queue"]] == ["keep me"]
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held, run, begun=False: None)
    assert send(task, "now").outcome == "resumed", "the conversation can still be continued"


def test_Worker_AFailedLaunchOfTheOnlyMessageIsNotRetriedInALoop(task):
    finished_builder(task)
    agent_messages.hold(task, "build-1", "once", "cli:person")
    decision = SimpleNamespace(reason="reserve", action="wait")
    context = FakeContext(fresh(task), [])
    context.runner = SimpleNamespace(run=lambda request: (_ for _ in ()).throw(budget_guard.BudgetWait(decision)))

    agent_resume.work(context, "build-1")  # returns instead of trying again and again

    assert [entry["text"] for entry in fresh(task).read_state()["agent_queue"]] == ["once"]


# -- nothing waiting is thrown away

def test_Queue_PastTheLimit_IsRefusedInWordsAndNothingWaitingIsDropped(task, monkeypatch):
    monkeypatch.setattr(agent_messages, "QUEUE_LIMIT", 12)
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    outcomes = [send(task, f"message {number}", "review-1") for number in range(agent_messages.QUEUE_LIMIT + 1)]

    assert [outcome.outcome for outcome in outcomes[:-1]] == ["queued"] * agent_messages.QUEUE_LIMIT
    assert outcomes[-1].outcome == "refused" and "waiting" in outcomes[-1].words
    held = [entry["text"] for entry in fresh(task).read_state()["agent_queue"]]
    assert held == [f"message {number}" for number in range(agent_messages.QUEUE_LIMIT)]


def test_Queue_MessagesTheEngineRequeuesAreNeverTrimmed(task, monkeypatch):
    monkeypatch.setattr(agent_messages, "QUEUE_LIMIT", 12)
    for number in range(agent_messages.QUEUE_LIMIT + 5):
        inbox.queue_again(fresh(task), f"id-{number}", {"id": f"id-{number}", "text": f"late {number}", "actor": "cli:person",
                                                         "kind": "steer", inbox.FOR_RUN: "build-1"})

    assert len(fresh(task).read_state()["agent_queue"]) == agent_messages.QUEUE_LIMIT + 5


def test_Start_AnErrorOfTheRunnerWhileLaunching_HoldsTheMessageAgainAndSaysSo(task):
    finished_builder(task)
    name = send(task, "keep me").run
    context = FakeContext(fresh(task), [])

    def broken(request):
        raise run_records.RunRecordError(f"run {name} is not recorded")

    context.runner = SimpleNamespace(run=broken)

    assert agent_resume.run_begun(context, name) is False

    state = fresh(task).read_state()
    assert state["runs"][name]["state"] == "aborted"
    assert [entry["text"] for entry in state["agent_queue"]] == ["keep me"]
    assert f"{name} could not continue" in task.ledger.read_text()
