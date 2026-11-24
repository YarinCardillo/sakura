"""Two lanes started back to back: the second one, created and not yet given its repository, must not disturb the first."""

import pytest

from conftest import git
from flow_support import make_context
from sakura import install
from sakura.flow import changes, cli, config, engine, exits, others, resume, runner, store, task_repository


@pytest.fixture
def project(repo, monkeypatch):
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: exits.OK)
    return repo


def test_Changes_ALaneWithNoRepositoryYet_IsNotKnownNotAnError(repo):
    ctx = make_context(repo, task="second", state={"base_commit": git(repo, "rev-parse", "HEAD")})
    assert task_repository.STATE_KEY not in ctx.store.read_state()
    assert changes.of_lane(ctx.store.read_state(), repo) is None


def test_Others_ALaneStillWaitingForItsRepository_IsListedAsNotKnownAndTheSummaryGoesOn(repo):
    make_context(repo, task="second", state={"base_commit": git(repo, "rev-parse", "HEAD")})
    text = others.summary([repo], "repo/first")
    assert f"{repo.name}/second" in text and "Files changed: not known" in text


def test_Start_TheSecondLaneIsCreatedBeforeItsRepository_TheFirstLanesDriverStillReadsTheOthers(project, monkeypatch):
    """Real proof 5: lane A's driver built its role's others.md while lane B existed without its repository, and died
    with 'has no repository of its own yet'."""
    assert cli.main(["start", "-m", "first thing", "--actor=cli:person"]) == exits.OK
    first = store.TaskStore(project, next(iter(store.task_stores(project))).task_id)
    seen = []
    real = cli._make_task_repository

    def read_the_others_first(task_store, cfg, repo):
        seen.append(runner.AgentRunner(first, config.load(project))._others())  # the first lane's driver, mid start
        real(task_store, cfg, repo)

    monkeypatch.setattr(cli, "_make_task_repository", read_the_others_first)
    assert cli.main(["start", "-m", "second thing", "--actor=cli:person"]) == exits.OK
    assert len(seen) == 1 and "not known" in seen[0]
    assert all(task_repository.STATE_KEY in lane.read_state() for lane in store.task_stores(project))


# -- a driver that dies leaves the lane Stop and Resume can act on ---------------------------------------------------------------


def _dies(_ctx):
    raise task_repository.TaskRepositoryError("task demo has no repository of its own yet")


def test_Advance_ADriverThatRaises_LeavesTheLaneBlockedNamingTheErrorNeverRunning(repo, monkeypatch):
    monkeypatch.setattr(engine, "_HANDLERS", {})
    ctx = make_context(repo)
    engine.register("intake", _dies)
    with pytest.raises(task_repository.TaskRepositoryError):
        engine.advance(ctx)
    state = store.TaskStore(repo, "demo").read_state()
    assert state["status"] == "blocked" and state["phase"] == "intake"
    assert "engine stopped on an error" in state["reason"] and "no repository of its own yet" in state["reason"]
    assert "engine stopped on an error" in store.TaskStore(repo, "demo").ledger.read_text()


def test_Advance_AfterTheDriverDied_ResumeIsAdmittedAndStopTakes(repo, monkeypatch, capsys):
    monkeypatch.setattr(engine, "_HANDLERS", {})
    ctx = make_context(repo)
    engine.register("intake", _dies)
    with pytest.raises(task_repository.TaskRepositoryError):
        engine.advance(ctx)
    fresh = store.TaskStore(repo, "demo")
    state = fresh.read_state()
    assert not resume.retry_futile(state)
    resume.require_stopped(state, resume.revision(state))
    monkeypatch.chdir(repo)
    assert cli.main(["stop", "--actor=cli:person", "demo"]) == exits.OK
    assert store.TaskStore(repo, "demo").read_state()["status"] == "stopped"


def test_Advance_AnErrorWhileWritingTheBlock_DoesNotHideTheOriginalError(repo, monkeypatch):
    monkeypatch.setattr(engine, "_HANDLERS", {})
    ctx = make_context(repo)
    engine.register("intake", _dies)
    real = engine._set_status

    def failing_block(context, status, reason):
        if status == "blocked":
            raise OSError("disk full")
        real(context, status, reason)

    monkeypatch.setattr(engine, "_set_status", failing_block)
    with pytest.raises(task_repository.TaskRepositoryError):
        engine.advance(ctx)


def test_Stop_ARunningLaneWithNoDriverBehindIt_BecomesStoppedAndResumeIsThenAdmitted(repo, monkeypatch):
    """A driver killed outright (not by an error the engine could catch) leaves `running` with nothing behind it."""
    make_context(repo, state={"status": "running", "phase": "build"})
    monkeypatch.chdir(repo)
    assert cli.main(["stop", "--actor=cli:person", "demo"]) == exits.OK
    state = store.TaskStore(repo, "demo").read_state()
    assert state["status"] == "stopped" and "it was running" in state["reason"]
    resume.require_stopped(state, resume.revision(state))
