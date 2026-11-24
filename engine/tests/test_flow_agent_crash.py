"""A process that dies at any point leaves every accepted message deliverable exactly once."""

import os
from types import SimpleNamespace

import pytest

from flow_run_support import INIT, LIVE_IDENTITY, add_run, make_task, you
from flow_support import make_context
from sakura import repository
from sakura.flow import (agent_messages, agent_resume, agent_steer, budget_guard, config, final_head, inbox, process,
                         run_records, run_view, runner, store, task_repository)
from test_flow_agent_resume import FakeContext, finished_builder

HELD, FIRST, LATER = "the held message", "the new message", "the later message"


class Crash(Exception):
    """A process death."""


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    monkeypatch.setattr(inbox, "LIVE_WAIT_SECONDS", 0.2)
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda held, run, begun=False: None)
    return make_task(repo, tmp_path)


def fresh(task):
    return store.TaskStore(task.repo, task.task_id)


def send(task, words, run="build-1"):
    cfg = config.load(task.repo)
    return agent_steer.send(fresh(task), cfg, repository.common_dir(task.repo), run, words, "cli:person")


def occurrences(task, words):
    """How many times `words` is waiting: held in the queue or in a record that never launched."""
    state = fresh(task).read_state()
    held = sum(entry["text"].count(words) for entry in state.get("agent_queue", []))
    pending = sum((record.get("pending_text") or "").count(words) for record in state["runs"].values())
    return held + pending


def crashing(monkeypatch, point, when):
    """Make the `point`-th write of the state die, before it is made or just after."""
    real = store.TaskStore.update_state
    calls = []

    def update_state(self, change):
        calls.append(1)
        if len(calls) == point and when == "before":
            raise Crash
        result = real(self, change)
        if len(calls) == point and when == "after":
            raise Crash
        return result

    monkeypatch.setattr(store.TaskStore, "update_state", update_state)
    return calls


POINTS = [(point, when) for point in range(1, 9) for when in ("before", "after")]


@pytest.mark.parametrize("point,when", POINTS)
def test_Start_AProcessThatDiesAtAnyWrite_LeavesTheHeldMessageDeliverableExactlyOnce(task, monkeypatch, point, when):
    finished_builder(task)
    agent_messages.hold(fresh(task), "build-1", HELD, "cli:person")
    with monkeypatch.context() as patch:
        crashing(patch, point, when)
        try:
            send(task, FIRST)
        except Crash:
            pass

    assert occurrences(task, HELD) == 1, "never lost, never twice"
    assert occurrences(task, FIRST) <= 1
    monkeypatch.setattr(run_view, "STARTING_GRACE_SECONDS", -1)  # a start that never launched has had its time
    outcome = send(task, LATER)
    assert outcome.outcome == "resumed"
    assert occurrences(task, HELD) == 1 and occurrences(task, LATER) == 1


@pytest.mark.parametrize("point,when", [(point, when) for point in range(1, 6) for when in ("before", "after")])
def test_Abandon_AProcessThatDiesAtAnyWrite_LeavesTheMessageDeliverableExactlyOnce(task, monkeypatch, point, when):
    finished_builder(task)
    name = send(task, FIRST).run
    decision = SimpleNamespace(reason="reserve", action="wait")
    context = FakeContext(fresh(task), [])
    context.runner = SimpleNamespace(run=lambda request: (_ for _ in ()).throw(budget_guard.BudgetWait(decision)))
    with monkeypatch.context() as patch:
        crashing(patch, point, when)
        try:
            agent_resume.run_begun(context, name)
        except Crash:
            pass

    assert occurrences(task, FIRST) == 1


def test_Launch_ThePendingMessageStaysUntilTheLogShowsTheRunTookIt(task):
    finished_builder(task)
    name = send(task, FIRST).run
    run_records.mark_running(fresh(task), name, LIVE_IDENTITY)

    assert occurrences(task, FIRST) == 1, "a process that exists has not read it yet"


def test_Launch_AWorkerKilledAfterItStartedButBeforeItReadTheMessage_LeavesItForTheNextResume(task, monkeypatch):
    finished_builder(task)
    name = send(task, FIRST).run
    run_records.mark_running(fresh(task), name, {"pid": 2147480000, "command": "gone"})  # killed: no replay in the log

    outcome = send(task, LATER)

    assert outcome.outcome == "resumed" and occurrences(task, FIRST) == 1


def test_Launch_AMessageTheKilledRunDidReadIsNotSentAgain(task):
    finished_builder(task)
    name = send(task, FIRST).run
    run_records.mark_running(fresh(task), name, {"pid": 2147480000, "command": "gone"})
    run_records.log_path(fresh(task), name).write_text(INIT + you(FIRST))  # the run replayed it, then died

    outcome = send(task, LATER)

    assert outcome.outcome == "resumed" and occurrences(task, FIRST) == 0 and occurrences(task, LATER) == 1


class ReadingLauncher:
    """A launcher whose run reads its first message (it is replayed into the log) and then goes on."""

    host = "local"

    def __init__(self, store_, words):
        self.store, self.words, self.pending_seen = store_, words, []

    def run(self, request, on_start):
        on_start({"pid": os.getpid(), "command": process.command_of(os.getpid())})
        request.log_path.write_text(request.log_path.read_text() + INIT + you(self.words))
        request.on_poll()
        self.pending_seen.append(self.store.read_state()["runs"][request.log_path.stem].get("pending_text"))
        return process.ProcessOutcome(0, False, False, False, os.getpid())

    def is_alive(self, identity):
        return False

    def kill(self, identity):
        return None


def test_Runner_TheSupervisorClearsThePendingMessageOnceTheRunReplaysIt(repo, monkeypatch):
    from flow_support import task_workspace

    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx = make_context(repo, overrides={"roles": {"builder": "claude:sonnet"}})
    workspace = task_workspace(ctx)
    add_run(ctx.store, "build-1", INIT, cwd=workspace)
    run_records.begin(ctx.store, "build-1.2", "builder", "build", "claude", "sonnet", "low", "local", workspace, {},
                      resumes="build-1", session_id="s", pending_text=FIRST)
    launcher = ReadingLauncher(ctx.store, FIRST)

    runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(
        runner.RunRequest("builder", "build", FIRST, {}, workspace, resume=runner.ResumeOf("build-1.2", "s", "sonnet", "low")))

    assert launcher.pending_seen == [None]


# -- a write to a run whose folder was removed meanwhile

def test_Hold_ASenderThatDecidedToQueueJustBeforeTheFolderWasRemoved_IsToldSoAndNothingIsHeld(task, monkeypatch):
    import shutil

    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    shutil.rmtree(task.workspace)
    monkeypatch.setattr(agent_steer.run_view, "listening", lambda *args, **kwargs: ("queue", None))

    outcome = send(task, "too late", "review-1")

    assert outcome.outcome == "refused" and "folder is gone" in outcome.words
    assert not fresh(task).read_state().get("agent_queue")


# -- review copies

@pytest.fixture
def ctx(repo, monkeypatch):
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    return make_context(repo, own_repository=True, overrides={"roles": {"code_reviewer": "codex"}})


def begin_review(ctx, worktree):
    run_records.begin(ctx.store, "review-1", "code_reviewer", "review", "codex", "", "low", "local", worktree, {})
    run_records.mark_running(ctx.store, "review-1", LIVE_IDENTITY)
    run_records.finish(ctx.store, "review-1", 0)


def test_ReviewCopy_ARetainedCopyWithNothingPendingAndNoLiveRun_IsReconciledAway(ctx):
    base = ctx.store.read_state()["base_commit"]
    with final_head.review_copy(ctx, "review-x", base, base) as (repository_path, worktree):
        begin_review(ctx, worktree)
        agent_messages.hold(ctx.store, "review-1", "explain", "cli:person")
    assert worktree.is_dir(), "retained while the message waits"
    agent_messages.drop(ctx.store, {entry["id"] for entry in ctx.store.read_state()["agent_queue"]})  # a worker drained it and died

    agent_resume.reconcile_retained(ctx.store)

    assert not worktree.exists() and not ctx.store.read_state().get(task_repository.RETAINED_KEY)


def test_ReviewCopy_AStaleEngineStartReconcilesRetainedCopies(ctx):
    from sakura.flow import engine

    base = ctx.store.read_state()["base_commit"]
    with final_head.review_copy(ctx, "review-x", base, base) as (repository_path, worktree):
        begin_review(ctx, worktree)
        agent_messages.hold(ctx.store, "review-1", "explain", "cli:person")
    agent_messages.drop(ctx.store, {entry["id"] for entry in ctx.store.read_state()["agent_queue"]})

    engine.kill_stale_run(ctx)

    assert not worktree.exists()


def test_ReviewCopy_ACopyStillNeededIsNotReconciledAway(ctx):
    base = ctx.store.read_state()["base_commit"]
    with final_head.review_copy(ctx, "review-x", base, base) as (repository_path, worktree):
        begin_review(ctx, worktree)
        agent_messages.hold(ctx.store, "review-1", "explain", "cli:person")

    agent_resume.reconcile_retained(ctx.store)

    assert worktree.is_dir()


def test_ReviewCopy_TheDecisionAtTeardownWaitsForASenderHoldingAMessage(ctx):
    import threading

    base = ctx.store.read_state()["base_commit"]
    inside = threading.Event()
    results = []

    def sender():
        inside.wait(5)
        agent_resume.hold_for(ctx.store, "review-1", "explain", "cli:person")
        results.append("held")

    thread = threading.Thread(target=sender)
    thread.start()
    with final_head.review_copy(ctx, "review-x", base, base) as (repository_path, worktree):
        begin_review(ctx, worktree)
        inside.set()
        thread.join(0.3)  # the sender is now at the guard or has held: either way teardown must see a consistent queue
    thread.join(5)

    state = ctx.store.read_state()
    kept = worktree.is_dir()
    held = bool(state.get("agent_queue"))
    assert results == ["held"] and (kept == held), "a held message and its folder are never apart"


# -- the cap holds on the fallback path

def test_Queue_ASteerTheMailboxConsumesWhenTheQueueIsFull_IsRefusedNotAppended(task, monkeypatch):
    monkeypatch.setattr(agent_messages, "QUEUE_LIMIT", 3)
    add_run(task, "build-1", INIT, state="running")
    for number in range(3):
        agent_messages.hold(task, "build-1", f"held {number}", "cli:person")
    path = inbox.enqueue(task, "steer", "the fourth", "", "cli:person", run="build-1")

    inbox.consume(task, task.repo, "runner")

    state = fresh(task).read_state()
    assert [entry["text"] for entry in state["agent_queue"]] == ["held 0", "held 1", "held 2"]
    assert inbox.was_rejected(task, path) and not state.get("steer_pending")


def test_Queue_ASenderWhoseLiveSteerWasRejectedByTheCap_IsToldInWords(task, monkeypatch):
    monkeypatch.setattr(agent_messages, "QUEUE_LIMIT", 3)
    add_run(task, "build-1", INIT, state="running")
    inbox.mark_live(task, "build-1", "builder")
    for number in range(3):
        agent_messages.hold(task, "build-1", f"held {number}", "cli:person")

    outcome = send(task, "the fourth")

    assert outcome.outcome == "refused" and "waiting" in outcome.words


# -- the lane restarting does not end an agent a person is talking to

def test_Engine_TheLanesStartDoesNotKillAChatRunThatIsStillAlive(repo):
    import subprocess

    from flow_support import task_workspace
    from sakura.flow import engine

    ctx = make_context(repo, own_repository=True)
    workspace = task_workspace(ctx)
    add_run(ctx.store, "build-1", INIT, cwd=workspace)
    child = subprocess.Popen(["sleep", "30"])
    try:
        identity = {"pid": child.pid, "command": process.command_of(child.pid)}
        run_records.begin(ctx.store, "build-1.2", "builder", "build", "claude", "sonnet", "low", "local", workspace, {},
                          resumes="build-1")
        run_records.mark_running(ctx.store, "build-1.2", identity)
        entry = {**identity, "run": "build-1.2", "host": "local"}
        ctx.store.update_state(lambda state: state.update(active_runs=[entry], active_run=entry))

        engine.kill_stale_run(ctx)

        assert child.poll() is None, "the chat's own worker supervises it"
        assert ctx.store.read_state()["active_runs"] == [entry]
    finally:
        child.kill()
        child.wait()
