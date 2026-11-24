"""A resume runs on the account the conversation ran on: one resolution from the account configuration feeds the guard,
the launch environment and the transcript root of the continuation."""

import json
import os
import re
from types import SimpleNamespace

import pytest

from flow_run_support import INIT, SESSION, add_run, assistant, make_task, text, you
from flow_support import MonitorLauncher, make_context, task_workspace
from leeway_support import account, make_helper
from sakura import repository
from sakura.flow import resume_account, agent_resume, agent_steer, budget_guard, cli, config, exits, inbox, process, run_records, runner, store
from sakura.usage import leeway
from test_budget_account_reserve import configure as configure_registry

BUDGET_A = {"kind": "budget", "id": "a"}


@pytest.fixture(autouse=True)
def fresh_snapshot_clock(monkeypatch):
    """The registry helper stamps its snapshots with a clock read when its module was imported; a full run outlasts the
    guard's limit on a snapshot's age, so every test here starts from the present."""
    from datetime import datetime, timezone

    import test_budget_account_reserve as reserve

    monkeypatch.setattr(reserve, "NOW", datetime.now(timezone.utc))


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda *args, **kwargs: None)
    monkeypatch.setattr(inbox, "LIVE_WAIT_SECONDS", 0.3)
    leeway.forget()
    created = make_task(repo, tmp_path)
    created.home = tmp_path / "isolated-home"
    return created


def folder_of(task, name):
    return task.home / f".claude-{name}"


def put_transcript(task, root):
    folder = root / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(task.workspace))
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"{SESSION}.jsonl").write_text("{}\n")


def conversation_on_a(task, **extra):
    """A finished builder that ran on the registry account `a`, whose transcript is in a's folder."""
    path = add_run(task, "build-1", INIT + assistant(text("Done.")), account="a", account_ref=BUDGET_A,
                   transcript_root=str(folder_of(task, "a")), **extra)
    put_transcript(task, folder_of(task, "a"))
    return path


def fresh(task):
    return store.TaskStore(task.repo, task.task_id)


def send(task, lane_account="b", words="go on"):
    cfg = config.load(task.repo, overrides={"budget": {"account": lane_account}})
    return agent_steer.send(fresh(task), cfg, repository.common_dir(task.repo), "build-1", words, "cli:person")


def registry(tmp_path, a_week, b_week):
    configure_registry(tmp_path, {("claude", "a"): (None, a_week), ("claude", "b"): (None, b_week)})


# -- the guard judges the account of the conversation

def test_Resume_TheGuardJudgesTheAccountTheConversationRanOn_NotTheOneTheLaneUsesNow(task, tmp_path):
    registry(tmp_path, a_week=99.0, b_week=10.0)
    conversation_on_a(task)

    outcome = send(task, lane_account="b")

    assert outcome.outcome == "refused" and "99%" in outcome.words
    assert sorted(fresh(task).read_state()["runs"]) == ["build-1"]


def test_Resume_AnExhaustedAccountOfTheLaneDoesNotStopAConversationOfAnother(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=99.0)
    conversation_on_a(task)

    outcome = send(task, lane_account="b")

    assert (outcome.outcome, outcome.run) == ("resumed", "build-1.2")


def test_Resume_TheContinuationRecordsTheAccountAndItsFolderAsTheTranscriptRoot(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    conversation_on_a(task)

    send(task, lane_account="b")

    new = fresh(task).read_state()["runs"]["build-1.2"]
    assert new["transcript_root"] == str(folder_of(task, "a")) and new["account_ref"] == BUDGET_A and new["account"] == "a"


def test_Resume_TheContinuationItselfCanBeContinued(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    conversation_on_a(task)
    send(task, lane_account="b")
    fresh_store = fresh(task)
    run_records.mark_running(fresh_store, "build-1.2", {"pid": 1, "command": "x"})
    run_records.log_path(fresh_store, "build-1.2").write_text(INIT)
    run_records.finish(fresh_store, "build-1.2", 0)

    outcome = send(task, lane_account="b", words="and again")

    assert (outcome.outcome, outcome.run) == ("resumed", "build-1.3")


def test_Sessions_ANamedAccountRunCanBeWrittenToWithoutItsFolderInTheOperatorsEnvironment(task, tmp_path, monkeypatch, capsys):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    conversation_on_a(task)
    capsys.readouterr()

    assert cli.main(["sessions", "demo", "--json"]) == exits.OK

    import json
    assert json.loads(capsys.readouterr().out)[0]["listening"] == "resume"


def test_Resume_TheAccountIsNoLongerInTheRegistry_IsRefusedInWords(task, tmp_path):
    configure_registry(tmp_path, {("claude", "b"): (None, 10.0)})
    conversation_on_a(task)

    outcome = send(task)

    assert outcome.outcome == "refused" and "account a" in outcome.words


def test_Resume_TheTranscriptIsLookedForInTheAccountsFolderNotTheDefaultOne(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    path = add_run(task, "build-1", INIT, account="a", account_ref=BUDGET_A, transcript_root=str(folder_of(task, "a")))
    put_transcript(task, task.home / ".claude")  # only the default folder has it

    outcome = send(task)

    assert outcome.outcome == "refused" and "no longer on this Mac" in outcome.words and path.exists()


# -- a step account (Leeway)

def test_Resume_AStepAccountIsJudgedOnItsOwnNumbersWhateverTheLaneUsesNow(task, tmp_path, monkeypatch):
    main_row, main_book = account("main", week=10.0)
    work_row, work_book = account("work", week=80.0)
    main_row["config_dir"], work_row["config_dir"] = str(tmp_path / "main-dir"), str(tmp_path / "work-dir")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(main_row, main_book), (work_row, work_book)])))
    registry(tmp_path, a_week=10.0, b_week=10.0)
    add_run(task, "build-1", INIT, account="Work", account_ref={"kind": "step", "id": "work"},
            transcript_root=str(tmp_path / "work-dir"))
    put_transcript(task, tmp_path / "work-dir")

    outcome = send(task)

    assert outcome.outcome == "refused" and "80%" in outcome.words


# -- the run itself

def test_Runner_AResumeLaunchesOnTheAccountsFolderAndIsGuardedOnTheSameAccount(repo, tmp_path, monkeypatch):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    ctx = make_context(repo, overrides={"budget": {"account": "b"}})
    workspace = task_workspace(ctx)
    add_run(ctx.store, "build-1", INIT, account="a", account_ref=BUDGET_A, cwd=workspace,
            transcript_root=str(tmp_path / "isolated-home" / ".claude-a"))
    put_transcript_for_cwd(tmp_path / "isolated-home" / ".claude-a", workspace)
    seen, judged = {}, []

    def monitor(argv, **kwargs):
        seen.update(env=kwargs["env"], argv=argv)
        return process.ProcessOutcome(0, False, False, False, 1)

    real = budget_guard.require
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: (judged.append(kwargs.get("on_account")),
                                                                         real(*args, **kwargs))[1])
    name = agent_resume.start(ctx.store, ctx.config, repository.common_dir(repo), "build-1", "go on")
    context = SimpleNamespace(store=ctx.store, repo=repo, config=ctx.config,
                              runner=runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)))

    agent_resume.run_begun(context, name)

    assert "env" in seen, ctx.store.ledger.read_text()
    assert seen["env"]["CLAUDE_CONFIG_DIR"] == str(tmp_path / "isolated-home" / ".claude-a"), "the lane's account b is not used"
    assert [item.key for item in judged if item is not None] == ["a", "a"], "judged on a when decided and when launched"


def put_transcript_for_cwd(root, cwd):
    folder = root / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(cwd))
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"{SESSION}.jsonl").write_text("{}\n")


# -- a role in a repository of its own

def test_Worker_AnIsolatedRoleIsResumedInItsOwnRepository(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    role_repository = tmp_path / "role-repo.git"
    role_repository.mkdir()
    workspace = tmp_path / "role-workspace"
    workspace.mkdir()
    task.update_state(lambda state: state.setdefault("role_repositories", {}).update(
        token={"path": str(role_repository), "config_sha256": "x", "workspace": str(workspace)}))
    add_run(task, "build-1", INIT, cwd=workspace, account="a", account_ref=BUDGET_A, transcript_root=str(folder_of(task, "a")))
    put_transcript_for(task, workspace)
    requests = []
    state_store = fresh(task)
    name = agent_resume.start(state_store, config.load(task.repo, overrides={"budget": {"account": "b"}}),
                                     repository.common_dir(task.repo), "build-1", "go on")
    context = SimpleNamespace(store=state_store, repo=task.repo, runner=SimpleNamespace(run=requests.append))

    agent_resume.run_begun(context, name)

    assert requests[0].repository == role_repository.resolve() or requests[0].repository == role_repository


def put_transcript_for(task, workspace):
    folder = folder_of(task, "a") / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(workspace))
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"{SESSION}.jsonl").write_text("{}\n")


# -- the actual Codex identity, the recorded root, and records from before account_ref

def codex_folder(task, name):
    return task.home / f".codex-{name}"


def codex_run(task, ref, root, **extra):
    return add_run(task, "review-1", '{"type":"thread.started","thread_id":"019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"}\n',
                   provider="codex", role="code_reviewer", account=(ref or {}).get("id"), account_ref=ref,
                   transcript_root=str(root), **extra)


def send_review(task, lane_account="a"):
    cfg = config.load(task.repo, overrides={"budget": {"account": lane_account}, "roles": {"code_reviewer": "codex"}})
    return agent_steer.send(fresh(task), cfg, repository.common_dir(task.repo), "review-1", "explain", "cli:person")


def test_Launch_ACodexRunRecordsThePairedCodexAccountItActuallyRanOn(repo, tmp_path, monkeypatch):
    from test_flow_runner import _codex_workspace

    configure_registry(tmp_path, {("claude", "a"): (None, 10.0), ("codex", "a"): (None, 10.0)})
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"code_reviewer": "codex"}})
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))

    runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(
        runner.RunRequest("code_reviewer", "review", "x", {}, _codex_workspace(ctx)))

    record = ctx.store.read_state()["runs"]["review-1"]
    assert record["account_ref"] == {"kind": "budget", "id": "a", "codex": "a"}


def test_Launch_ACodexRunOfANamedBudgetAccountWithNoPairedCodexAccount_HasTheDefaultReference(repo, tmp_path, monkeypatch):
    from test_flow_runner import _codex_workspace

    configure_registry(tmp_path, {("claude", "a"): (None, 10.0)})
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"code_reviewer": "codex"}})
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))

    runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(
        runner.RunRequest("code_reviewer", "review", "x", {}, _codex_workspace(ctx)))

    record = ctx.store.read_state()["runs"]["review-1"]
    assert record["account_ref"] is None and record["account"] is None


def test_Resume_ACodexConversationStaysOnTheAccountItRanOn_WhenThePairingChanges(task, tmp_path):
    configure_registry(tmp_path, {("claude", "a"): (None, 10.0), ("codex", "b"): (None, 10.0)})  # codex a is gone
    codex_run(task, {"kind": "budget", "id": "a", "codex": "a"}, codex_folder(task, "a"))

    outcome = send_review(task)

    assert outcome.outcome == "refused" and "account a" in outcome.words


def test_Resume_AnAccountWhoseFolderMovedSinceTheRun_IsRefusedInWords(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    add_run(task, "build-1", INIT, account="a", account_ref=BUDGET_A, transcript_root=str(task.home / ".claude-elsewhere"))

    outcome = send(task)

    assert outcome.outcome == "refused" and "account a" in outcome.words and "folder" in outcome.words


def test_Resume_ARecordFromBeforeAccountRefsThatMatchesAConfiguredAccountsFolder_IsFoundAgain(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    conversation_on_a(task)
    task.update_state(lambda state: state["runs"]["build-1"].pop("account_ref"))

    outcome = send(task)

    assert (outcome.outcome, outcome.run) == ("resumed", "build-1.2")
    assert fresh(task).read_state()["runs"]["build-1.2"]["account_ref"] == BUDGET_A, "the continuation names it properly"


def test_Resume_ARecordFromBeforeAccountRefsWhoseFolderMatchesNoAccount_IsRefusedNamingTheAccount(task, tmp_path):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    add_run(task, "build-1", INIT, account="Work", transcript_root=str(task.home / ".claude-gone"))
    task.update_state(lambda state: state["runs"]["build-1"].pop("account_ref"))
    put_transcript(task, task.home / ".claude-gone")

    outcome = send(task)

    assert outcome.outcome == "refused" and "Work" in outcome.words


# -- the default account never gets a config variable, and a logged-out answer is a failure

def test_Apply_TheDefaultFolderReachedThroughALinkIsStillTheDefaultFolder(tmp_path, monkeypatch):
    real = tmp_path / "real-claude"
    real.mkdir()
    home = tmp_path / "linked-home"
    home.mkdir()
    (home / ".claude").symlink_to(real)  # the registry names the real path, the home names the link
    monkeypatch.setenv("HOME", str(home))
    account = resume_account.ResumeAccount(real, SimpleNamespace(key="default", config_dir=str(real), snapshot=None))

    env = resume_account.apply({"CLAUDE_CONFIG_DIR": "/inherited", "KEEP": "1"}, "claude", account)

    assert env == {"KEEP": "1"}, "Claude keys its login by the variable: the default account must not have one"


def test_Apply_ANamedAccountWhoseFolderDiffers_GetsItsFolder(tmp_path, monkeypatch):
    home = tmp_path / "home"
    (home / ".claude").mkdir(parents=True)
    other = tmp_path / "work-claude"
    other.mkdir()
    monkeypatch.setenv("HOME", str(home))
    account = resume_account.ResumeAccount(other, SimpleNamespace(key="work", config_dir=str(other), snapshot=None))

    assert resume_account.apply({"KEEP": "1"}, "claude", account) == {"KEEP": "1", "CLAUDE_CONFIG_DIR": str(other)}


def test_Runner_AResumeOfTheDefaultAccountReachedThroughALink_IsLaunchedWithoutTheVariable(repo, tmp_path, monkeypatch):
    registry(tmp_path, a_week=10.0, b_week=10.0)
    home = tmp_path / "isolated-home"
    (home / ".claude").symlink_to(folder_of(SimpleNamespace(home=home), "a"))  # the default folder is a's folder
    ctx = make_context(repo, overrides={"budget": {"account": "b"}})
    workspace = task_workspace(ctx)
    add_run(ctx.store, "build-1", INIT, account="a", account_ref=BUDGET_A, cwd=workspace,
            transcript_root=str(home / ".claude-a"))
    put_transcript_for_cwd(home / ".claude-a", workspace)
    seen = {}

    def monitor(argv, **kwargs):
        seen.update(env=kwargs["env"])
        return process.ProcessOutcome(0, False, False, False, 1)

    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    name = agent_resume.start(ctx.store, ctx.config, repository.common_dir(repo), "build-1", "go on")
    context = SimpleNamespace(store=ctx.store, repo=repo, config=ctx.config,
                              runner=runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)))

    agent_resume.run_begun(context, name)

    assert "env" in seen and "CLAUDE_CONFIG_DIR" not in seen["env"]


LOGGED_OUT_LOG = (INIT + you("go on")
                  + json.dumps({"type": "assistant", "error": "authentication_failed",
                                "message": {"role": "assistant", "content": [{"type": "text", "text": "Not logged in · Please run /login"}]}}) + "\n"
                  + json.dumps({"type": "result", "subtype": "success", "is_error": True, "result": "Not logged in · Please run /login"}) + "\n")


def test_Resume_AnAnswerThatSaysTheProviderIsNotLoggedIn_IsAFailureInWordsAndTheMessageIsKept(repo, tmp_path, monkeypatch):
    ctx = make_context(repo, overrides={"roles": {"builder": "claude:sonnet"}})
    workspace = task_workspace(ctx)
    add_run(ctx.store, "build-1", INIT, cwd=workspace)
    put_transcript_for_cwd(tmp_path / "isolated-home" / ".claude", workspace)
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    name = agent_resume.start(ctx.store, ctx.config, repository.common_dir(repo), "build-1", "go on")

    class LoggedOut:
        host = "local"

        def run(self, request, on_start):
            on_start({"pid": os.getpid(), "command": process.command_of(os.getpid())})
            request.log_path.write_text(request.log_path.read_text() + LOGGED_OUT_LOG)
            return process.ProcessOutcome(0, False, False, False, os.getpid())

        def is_alive(self, identity):
            return False

        def kill(self, identity):
            return None

    context = SimpleNamespace(store=ctx.store, repo=repo, config=ctx.config,
                              runner=runner.AgentRunner(ctx.store, ctx.config, launcher=LoggedOut()))

    assert agent_resume.run_begun(context, name) is False

    state = ctx.store.read_state()
    assert state["runs"][name]["state"] == "aborted"
    assert [entry["text"] for entry in state["agent_queue"]] == ["go on"]
    ledger = ctx.store.ledger.read_text()
    assert f"{name} could not continue: Claude is not logged in" in ledger and "claude /login" in ledger
