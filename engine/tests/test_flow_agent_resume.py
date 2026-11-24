"""Writing to a finished agent: its conversation is continued headless in a new run of the same chain."""

import json
import re
import shutil
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from flow_run_support import INIT, SESSION, add_run, assistant, make_task, text
from sakura import repository
from sakura.flow import agent_messages, agent_resume, agent_steer, budget_guard, config, inbox, process, run_records, runner, store
from flow_support import MonitorLauncher, make_context, task_workspace


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda *args, **kwargs: None)
    monkeypatch.setattr(inbox, "LIVE_WAIT_SECONDS", 0.3)
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    return make_task(repo, tmp_path)


def put_transcript(cwd, session=SESSION):
    folder = Path.home() / ".claude" / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(cwd))
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"{session}.jsonl").write_text("{}\n")


def finished_builder(task, name="build-1", **extra):
    path = add_run(task, name, INIT + assistant(text("Done.")), **extra)
    put_transcript(task.workspace)
    return path


def fresh(task):
    return store.TaskStore(task.repo, task.task_id)


def send(task, run="build-1", words="also handle empty input"):
    cfg = config.load(task.repo)
    return agent_steer.send(fresh(task), cfg, repository.common_dir(task.repo), run, words, "cli:person")


# -- the new run's record

def test_Resume_StartsANewRunOfTheSameRoleModelEffortFolderAndAccount(task):
    finished_builder(task, account="work")

    outcome = send(task)

    assert (outcome.outcome, outcome.run) == ("resumed", "build-1.2") and outcome.words
    old, new = (fresh(task).read_state()["runs"][name] for name in ("build-1", "build-1.2"))
    assert new["state"] == "starting" and new["resumes"] == "build-1" and new["account"] == "work"
    for key in ("role", "phase", "provider", "model", "effort", "cwd", "host"):
        assert new[key] == old[key], key


def test_Resume_TheNextResumeOfTheChainIsNumberedOnAndLinksToTheNewestRun(task):
    finished_builder(task)
    send(task)
    run_records.mark_running(fresh(task), "build-1.2", {"pid": 1, "command": "x"})
    run_records.log_path(fresh(task), "build-1.2").write_text(INIT)
    run_records.finish(fresh(task), "build-1.2", 0)

    outcome = send(task, "build-1", "and again")

    assert (outcome.outcome, outcome.run) == ("resumed", "build-1.3")
    assert fresh(task).read_state()["runs"]["build-1.3"]["resumes"] == "build-1.2"


def test_Resume_HandsTheWorkerTheNewRunWhichHoldsThePersonsText(task, monkeypatch):
    finished_builder(task)
    handed = []
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda store, run, begun=False: handed.append((run, begun)))

    send(task, words="please add a test")

    assert handed == [("build-1.2", True)]
    assert fresh(task).read_state()["runs"]["build-1.2"]["pending_text"] == "please add a test"


def test_Resume_MessagesHeldForTheAgentGoWithTheNewOne(task):
    finished_builder(task)
    agent_messages.hold(fresh(task), "build-1", "first, held", "cli:person")
    send(task, words="second")

    assert fresh(task).read_state()["runs"]["build-1.2"]["pending_text"] == "first, held\n\nsecond" and not agent_messages.held_for(fresh(task).read_state(), {"build-1"})


def test_Resume_TwoWritesAtOnce_StartOneRunAndTheOtherIsHeldForIt(task):
    finished_builder(task)
    outcomes, gate = [], threading.Barrier(2)

    def write(words):
        gate.wait()
        outcomes.append(send(task, words=words))

    threads = [threading.Thread(target=write, args=(f"message {number}",)) for number in range(2)]
    [thread.start() for thread in threads]
    [thread.join() for thread in threads]

    assert sorted(outcome.outcome for outcome in outcomes) == ["queued", "resumed"]
    assert sorted(fresh(task).read_state()["runs"]) == ["build-1", "build-1.2"]


# -- refusals, in words

def test_Resume_FolderGone_IsRefused(task):
    finished_builder(task)
    shutil.rmtree(task.workspace)

    outcome = send(task)

    assert outcome.outcome == "refused" and "folder is gone" in outcome.words
    assert sorted(fresh(task).read_state()["runs"]) == ["build-1"]


def test_Resume_TranscriptMissing_IsRefused(task):
    add_run(task, "build-1", INIT + assistant(text("Done.")))

    outcome = send(task)

    assert outcome.outcome == "refused" and "no longer on this Mac" in outcome.words


def test_Resume_TheGuardSaysNo_ComesBackInItsWords(task, monkeypatch):
    finished_builder(task)
    decision = SimpleNamespace(reason="The account is at its reserve until 18:00.", action="wait")

    def refuse(*args, **kwargs):
        raise budget_guard.BudgetWait(decision)

    monkeypatch.setattr(budget_guard, "require", refuse)

    outcome = send(task)

    assert outcome.outcome == "refused" and "reserve until 18:00" in outcome.words
    assert sorted(fresh(task).read_state()["runs"]) == ["build-1"]


def test_Resume_TheRoleNowRunsOnAnotherAgent_IsRefused(task):
    finished_builder(task, role="reviewer")
    fresh(task).update_state(lambda state: state["runs"]["build-1"].update(provider="codex"))
    put_transcript(task.workspace)

    outcome = send(task)

    assert outcome.outcome == "refused"


# -- a run that continues a conversation

def _resumed_run(repo, provider="claude", role="builder", roles_override=None):
    ctx = make_context(repo, overrides={"roles": roles_override or {role: f"{provider}:sonnet"}})
    workspace = task_workspace(ctx)
    return ctx, workspace


def test_Runner_AResumeLaunchesTheContinuationWithTheTextAsItsFirstMessage(repo, monkeypatch):
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx, workspace = _resumed_run(repo)
    run_records.begin(ctx.store, "build-1", "builder", "build", "claude", "haiku", "low", "local", workspace, {})
    run_records.begin(ctx.store, "build-1.2", "builder", "build", "claude", "haiku", "low", "local", workspace, {},
                      resumes="build-1")
    seen = {}

    def monitor(argv, **kwargs):
        seen.update(kwargs, argv=argv)
        return process.ProcessOutcome(0, False, False, False, 1)

    request = runner.RunRequest("builder", "build", "tell me $HOME costs", {}, workspace,
                                resume=runner.ResumeOf("build-1.2", SESSION, "haiku", "low", ""))
    result = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)

    assert result.log_path.name == "build-1.2.log"
    assert seen["argv"][1:4] == ["-p", "--resume", SESSION] and seen["argv"][seen["argv"].index("--model") + 1] == "haiku"
    assert seen["stdin_text"] == "tell me $HOME costs", "the person's words as written: no template, no board note"
    runs = ctx.store.read_state()["runs"]
    assert runs["build-1.2"]["state"] == "finished" and runs["build-1.2"]["resumes"] == "build-1"
    assert runs["build-1"]["state"] == "starting", "the other runs and the pending steers of the lane are left alone"


def test_Runner_AHungResumeIsNotRestartedUnderANewRecord(repo, monkeypatch):
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx, workspace = _resumed_run(repo)
    run_records.begin(ctx.store, "build-1.2", "builder", "build", "claude", "haiku", "low", "local", workspace, {},
                      resumes="build-1")
    launches = []

    def monitor(argv, **kwargs):
        launches.append(argv)
        return process.ProcessOutcome(None, True, False, False, 1)

    request = runner.RunRequest("builder", "build", "x", {}, workspace,
                                resume=runner.ResumeOf("build-1.2", SESSION, "haiku", "low", ""))
    runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)

    assert len(launches) == 1


def test_Runner_ACodexResumeLogsTheMessageTheEngineGaveItAndRunsInTheFolder(repo, monkeypatch):
    from test_flow_runner import _codex_workspace

    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx = make_context(repo, overrides={"roles": {"code_reviewer": "codex"}})
    workspace = _codex_workspace(ctx)
    run_records.begin(ctx.store, "review-1.2", "code_reviewer", "review", "codex", "", "low", "local", workspace, {},
                      resumes="review-1")
    seen = {}

    def monitor(argv, **kwargs):
        seen.update(kwargs, argv=argv)
        return process.ProcessOutcome(0, False, False, False, 1)

    request = runner.RunRequest("code_reviewer", "review", "explain finding 2", {}, workspace,
                                resume=runner.ResumeOf("review-1.2", "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b", "", "low", ""))
    result = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)

    assert seen["argv"][1:4] == ["exec", "resume", "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"] and seen["cwd"] == workspace.resolve()
    messages = [json.loads(line) for line in result.log_path.read_text().splitlines()[1:] if line.startswith("{")]
    assert messages == [{"type": "sakura_message", "text": "explain finding 2"}]


# -- the worker that waits for a run to end and continues it with what was held

class FakeContext:
    def __init__(self, store, requests):
        self.store, self.repo, self.requests = store, store.repo, requests
        self.config = config.load(store.repo, overrides={"roles": {"code_reviewer": "codex"}})
        self.runner = SimpleNamespace(run=self._run)

    def _run(self, request):
        """Record the request and end the run it continues, as a supervised run would."""
        self.requests.append(request)
        run_records.mark_running(self.store, request.resume.name, {"pid": 1, "command": "x"})
        run_records.log_path(self.store, request.resume.name).write_text(INIT)
        run_records.finish(self.store, request.resume.name, 0)


def test_Worker_WaitsForTheRunToEndThenContinuesItWithWhatWasHeld(task, monkeypatch):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    agent_messages.hold(task, "review-1", "explain finding 2", "cli:person")
    run_records.log_path(task, "review-1").write_text(
        json.dumps({"type": "thread.started", "thread_id": "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"}) + "\n")
    requests, ticks = [], []

    def sleep(seconds):
        ticks.append(seconds)
        run_records.finish(task, "review-1", 0)  # the run ends while the worker waits

    monkeypatch.setattr(agent_resume.time, "sleep", sleep)

    agent_resume.work(FakeContext(task, requests), "review-1")

    assert ticks and len(requests) == 1
    assert requests[0].template == "explain finding 2" and requests[0].resume.name == "review-1.2"
    assert requests[0].resume.session_id == "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"


def test_Worker_NothingHeld_StartsNothing(task):
    finished_builder(task)
    requests = []

    agent_resume.work(FakeContext(task, requests), "build-1")

    assert requests == [] and sorted(task.read_state()["runs"]) == ["build-1"]


def test_Worker_OnlyOneWaitsPerAgent(task):
    finished_builder(task)
    agent_messages.hold(task, "build-1", "held", "cli:person")
    requests = []

    with agent_resume.worker_lock(task, "build-1") as first:
        assert first
        agent_resume.work(FakeContext(task, requests), "build-1")

    assert requests == [], "a second worker leaves the held message to the first"
    assert agent_messages.held_for(task.read_state(), {"build-1"})


def test_Worker_AMessageHeldWhileTheContinuationRuns_ContinuesItAgain(task):
    finished_builder(task)
    agent_messages.hold(task, "build-1", "first", "cli:person")
    requests = []

    def run(request):
        context._run(request)
        if len(requests) == 1:
            agent_messages.hold(task, "build-1", "second", "cli:person")

    context = FakeContext(task, requests)
    context.runner = SimpleNamespace(run=run)
    agent_resume.work(context, "build-1")

    assert [request.template for request in requests] == ["first", "second"]
    assert [request.resume.name for request in requests] == ["build-1.2", "build-1.3"]


# -- the command

def test_Command_AgentWorkerIsAnOperatorCommandNotOfferedToRoles():
    from sakura.flow import brakes

    assert "agent-worker" in brakes.OPERATOR_VERBS
