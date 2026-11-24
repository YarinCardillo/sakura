"""Work a resumed run does is part of its lane: it is reviewed and merged like a fix round, the lane does not merge or
deliver while a resume is live, and a lane that was delivered before the resume committed says what was not delivered."""

import threading
import time
from pathlib import Path

import pytest

from flow_run_support import INIT, LIVE_IDENTITY, add_run
from sakura.flow import config as flow_config, engine, exits, final_head, resumed_work, run_records, workspaces
from sakura.flow.phases import build

from conftest import git
from test_flow_build import _ctx, _zero, _commit, _engine_tree, _task, _use

TASK_ENV = "SAKURA_FLOW_BUILD_TASK"


@pytest.fixture(autouse=True)
def fast(monkeypatch):
    monkeypatch.setattr(workspaces, "_prepare", lambda ctx, name, path: None)
    monkeypatch.setattr(resumed_work, "POLL_SECONDS", 0.01)
    monkeypatch.setattr(engine, "_HANDLERS", {})


def _with_final_review(ctx):
    object.__setattr__(ctx, "steps", flow_config.PresetSteps(True, True, True, True))
    ctx.config.values["knobs"]["final_review_rounds"] = 1
    return ctx


def _workspace_of(ctx, task_id="T1") -> Path:
    state = ctx.store.read_state()
    return Path(state["workspaces"][state["build"][task_id]["workspace"]]["path"])


def _resume(ctx, name="build-1.2", state="finished", task_id="T1"):
    """A resumed run of the builder of `task_id` that committed `resumed.txt` in its workspace."""
    workspace = _workspace_of(ctx, task_id)
    if "build-1" not in ctx.store.read_state().get("runs", {}):
        add_run(ctx.store, "build-1", INIT, cwd=workspace, env={TASK_ENV: task_id})
    add_run(ctx.store, name, INIT, cwd=workspace, env={TASK_ENV: task_id}, resumes="build-1", state=state)
    (workspace / "resumed.txt").write_text("written after a message\n")
    git(workspace, "add", "resumed.txt")
    git(workspace, "commit", "--no-verify", "-qm", "feat: the person asked for this")
    return git(workspace, "rev-parse", "HEAD")


def _register_pipeline(ctx, delivered):
    engine.register("build", build.run)
    for phase in ("evaluate", "report", "cleanup"):
        engine.register(phase, lambda c: engine.PhaseOutcome("next"))
    engine.register("pr", lambda c: (delivered.append(_engine_tree(c)), engine.PhaseOutcome("next"))[1])
    ctx.store.update_state(lambda state: state.update(phase="build"))


def test_Advance_AResumedBuilderCommitDuringTheFinalReview_IsInTheDeliveredBranch(repo, tmp_path):
    ctx = _with_final_review(_ctx(repo, tmp_path, [_task("T1")]))
    runner = _use(ctx, _commit)
    delivered = []
    _register_pipeline(ctx, delivered)

    def during_the_final_review(request):
        if request.step == "final_review" and not (_workspace_of(ctx) / "resumed.txt").exists():
            _resume(ctx)

    runner.on_review = during_the_final_review

    assert engine.advance(ctx) == exits.OK

    assert len(delivered) == 1 and {"t1-0.txt", "resumed.txt"} <= delivered[0]
    reviewed = [request.step or request.phase for request in runner.of("code_reviewer")]
    assert reviewed.count("final_review") == 2, "the branch with the resumed commit was reviewed again"


def test_Advance_AResumedCommitIsReviewedAsATaskBeforeItIsMerged(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, _commit)
    _register_pipeline(ctx, [])
    runner.on_review = lambda request: (None if (_workspace_of(ctx) / "resumed.txt").exists() else _resume(ctx))

    engine.advance(ctx)

    task_reviews = [request for request in runner.of("code_reviewer", "T1")]
    assert len(task_reviews) == 2
    assert ctx.store.read_state()["build"]["T1"]["status"] == "merged"


def test_Settle_ALiveResume_WaitsUntilItEnds(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    _resume(ctx, state="running")
    ctx.store.update_state(lambda state: state.update(phase="evaluate"))
    ended = []

    def end_it():
        time.sleep(0.15)
        ended.append(time.monotonic())
        run_records.finish(ctx.store, "build-1.2", 0)

    threading.Thread(target=end_it).start()
    assert resumed_work.settle(ctx) is None

    assert ended and time.monotonic() >= ended[0]
    assert ctx.store.read_state()["phase"] == "build", "the commit it left sends the lane back to build"


def test_Settle_AStopWhileWaitingForAResume_StopsTheLane(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    _resume(ctx, state="running")
    ctx.store.update_state(lambda state: state.update(phase="evaluate"))
    ctx.store.stop_file.write_text("")

    assert resumed_work.settle(ctx).kind == "stopped"


def test_Settle_AResumeThatLeftNoCommit_ChangesNothing(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    add_run(ctx.store, "build-1", INIT, cwd=_workspace_of(ctx), env={TASK_ENV: "T1"})
    add_run(ctx.store, "build-1.2", INIT, cwd=_workspace_of(ctx), env={TASK_ENV: "T1"}, resumes="build-1")
    ctx.store.update_state(lambda state: state.update(phase="evaluate"))

    assert resumed_work.settle(ctx) is None
    assert ctx.store.read_state()["phase"] == "evaluate"


def test_Settle_ACommitNoResumeMade_IsStillIgnored(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    workspace = _workspace_of(ctx)
    (workspace / "survivor.txt").write_text("never reviewed\n")
    git(workspace, "add", "survivor.txt")
    git(workspace, "commit", "--no-verify", "-qm", "survivor")
    ctx.store.update_state(lambda state: state.update(phase="evaluate"))

    assert resumed_work.settle(ctx) is None
    assert ctx.store.read_state()["phase"] == "evaluate"


def test_Settle_BeforeTheBuild_DoesNothing(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])

    assert resumed_work.settle(ctx) is None


# -- a resume that commits after the lane was delivered

def _delivered(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    ctx.store.update_state(lambda state: state.update(phase="cleanup", status="report_ready", reason=""))
    return ctx


def test_FlagLateDelivery_AResumeCommittedAfterDelivery_TheLaneIsBlockedWithWhatWasNotDelivered(repo, tmp_path):
    ctx = _delivered(repo, tmp_path)
    _resume(ctx)

    resumed_work.flag_late_commits(ctx.store)

    state = ctx.store.read_state()
    assert state["status"] == "blocked"
    assert state["reason"].startswith("Your message to the builder produced 1 commit after the lane was delivered: "
                                      "it is not in the delivered branch.")
    assert "Resume the lane to deliver again" in state["reason"]


def test_FlagLateDelivery_TwoCommits_SaysSoInThePlural(repo, tmp_path):
    ctx = _delivered(repo, tmp_path)
    _resume(ctx)
    workspace = _workspace_of(ctx)
    (workspace / "more.txt").write_text("more\n")
    git(workspace, "add", "more.txt")
    git(workspace, "commit", "--no-verify", "-qm", "more")

    resumed_work.flag_late_commits(ctx.store)

    assert "produced 2 commits after the lane was delivered: they are not in the delivered branch." in (
        ctx.store.read_state()["reason"])


def test_FlagLateDelivery_ALaneThatIsNotDelivered_IsLeftAlone(repo, tmp_path):
    ctx = _delivered(repo, tmp_path)
    ctx.store.update_state(lambda state: state.update(status="running"))
    _resume(ctx)

    resumed_work.flag_late_commits(ctx.store)

    assert ctx.store.read_state()["status"] == "running"


def test_FlagLateDelivery_ResumingTheBlockedLane_ReviewsMergesAndDeliversTheCommitAgain(repo, tmp_path):
    ctx = _delivered(repo, tmp_path)
    runner = _use(ctx, _commit)
    _resume(ctx)
    resumed_work.flag_late_commits(ctx.store)
    delivered = []
    _register_pipeline(ctx, delivered)
    ctx.store.update_state(lambda state: state.update(phase="cleanup"))

    assert engine.advance(ctx) == exits.OK

    assert delivered and "resumed.txt" in delivered[0]
    assert [request.step or request.phase for request in runner.of("code_reviewer")] == ["review"]


def test_Worker_ATurnThatCommitsAfterTheLaneWasDelivered_BlocksTheLaneAtTheEndOfTheRun(repo, tmp_path):
    from types import SimpleNamespace

    from flow_run_support import SESSION
    from sakura.flow import agent_resume

    ctx = _delivered(repo, tmp_path)
    workspace = _workspace_of(ctx)
    add_run(ctx.store, "build-1", INIT, cwd=workspace, env={TASK_ENV: "T1"})
    run_records.begin(ctx.store, "build-1.2", "builder", "build", "claude", "sonnet", "low", "local", workspace,
                      {TASK_ENV: "T1"}, resumes="build-1", session_id=SESSION, pending_text="one more thing")

    def answers_with_a_commit(request):
        run_records.mark_running(ctx.store, "build-1.2", LIVE_IDENTITY)
        (workspace / "resumed.txt").write_text("late\n")
        git(workspace, "add", "resumed.txt")
        git(workspace, "commit", "--no-verify", "-qm", "late")
        run_records.finish(ctx.store, "build-1.2", 0)

    context = SimpleNamespace(store=ctx.store, repo=ctx.repo, runner=SimpleNamespace(run=answers_with_a_commit))
    agent_resume.run_begun(context, "build-1.2")

    state = ctx.store.read_state()
    assert state["status"] == "blocked" and "1 commit after the lane was delivered" in state["reason"]


# -- a resume of the final fix builder, which works in workspace 0, and a lane that pushed (the 040g sequence)

PR_LOOKUP_FAILED = "pull request lookup failed: To get started with GitHub CLI, please run: gh auth login"


def _pushed_and_blocked_at_the_pull_request(repo, tmp_path, status="blocked", reason=PR_LOOKUP_FAILED, pushed=True):
    """A lane that built, passed its final review (a fix round of the final builder ran in workspace 0), pushed its branch and
    stopped at the pull request step, where the person then wrote to the final builder, which committed in workspace 0."""
    from sakura.flow import delivery

    ctx = _with_final_review(_ctx(repo, tmp_path, [_task("T1")]))
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    zero = _zero(ctx)
    head = final_head.recorded(ctx.store.read_state())
    add_run(ctx.store, "build-5", INIT, cwd=zero, env={TASK_ENV: "final-1"})
    add_run(ctx.store, "build-5.2", INIT, cwd=zero, env={TASK_ENV: "final-1"}, resumes="build-5")
    (zero / "late.txt").write_text("written after the push\n")
    git(zero, "add", "late.txt")
    git(zero, "commit", "--no-verify", "-qm", "feat: the person asked for a comment")
    if pushed:
        delivery.record(ctx.store, "local", head, "feat/flow-demo")
    ctx.store.update_state(lambda state: state.update(phase="pr", status=status, reason=reason, contract_sha256=""))
    return ctx


def test_Settle_AResumeOfTheFinalBuilderInWorkspaceZero_IsTakenIntoTheEnginesHead(repo, tmp_path):
    ctx = _pushed_and_blocked_at_the_pull_request(repo, tmp_path)

    assert resumed_work.settle(ctx) is None

    assert "late.txt" in _engine_tree(ctx)
    assert ctx.store.read_state()["phase"] == "build", "the final review and the checks see it before the push goes again"


def test_Advance_TheLaneThatPushedAndWasBlockedAtThePullRequest_PushesTheResumedCommitBeforeItGoesOn(repo, tmp_path):
    ctx = _pushed_and_blocked_at_the_pull_request(repo, tmp_path)
    resumed_work.flag_late_commits(ctx.store)
    pushed = []
    _register_pipeline(ctx, pushed)
    ctx.store.update_state(lambda state: state.update(phase="pr"))

    assert engine.advance(ctx) == exits.OK

    assert len(pushed) == 1 and {"t1-0.txt", "late.txt"} <= pushed[0], "the pull request step pushed the branch with it"


def test_FlagLateCommits_ALaneBlockedAtThePullRequestAfterItPushed_SaysWhatIsNotInThePushedBranchAndKeepsWhyItIsStuck(repo, tmp_path):
    ctx = _pushed_and_blocked_at_the_pull_request(repo, tmp_path)

    resumed_work.flag_late_commits(ctx.store)

    state = ctx.store.read_state()
    assert state["status"] == "blocked"
    assert state["reason"].startswith("Your message to the builder produced 1 commit after the lane pushed its branch: "
                                      "it is not in the pushed branch. Resume the lane to push it before it goes on to "
                                      "the pull request.")
    assert "GitHub CLI is not logged in" in state["reason"]
    resumed_work.flag_late_commits(ctx.store)
    assert ctx.store.read_state()["reason"] == state["reason"], "a second look says the same, once"


@pytest.mark.parametrize("status,pushed,expected_status,said", [
    ("running", True, "running", False),
    ("awaiting_approval", True, "awaiting_approval", False),
    ("blocked", False, "blocked", False),
    ("blocked", True, "blocked", True),
    ("report_ready", True, "blocked", True),
    ("stopped", True, "stopped", True),
    ("failed", True, "failed", True),
    ("stopped", False, "stopped", True),
    ("failed", False, "failed", True),
])
def test_FlagLateCommits_EveryStatusOfTheLane_NeverLosesACommitInSilence(repo, tmp_path, status, pushed, expected_status, said):
    from sakura.flow import waits

    ctx = _pushed_and_blocked_at_the_pull_request(repo, tmp_path, status=status, reason="", pushed=pushed)

    resumed_work.flag_late_commits(ctx.store)

    state = ctx.store.read_state()
    asked = [wait.detail for wait in waits.collect([repo])]
    assert state["status"] == expected_status
    assert any("Your message to the builder produced 1 commit" in detail for detail in asked) is said, asked


def test_FlagLateCommits_AStoppedOrFailedLane_SaysItInItsAskAndChangesNothingElse(repo, tmp_path):
    from sakura.flow import waits

    ctx = _pushed_and_blocked_at_the_pull_request(repo, tmp_path, status="failed", reason="the build failed")
    before = ctx.store.read_state()

    resumed_work.flag_late_commits(ctx.store)

    state = ctx.store.read_state()
    (wait,) = waits.collect([repo])
    assert (state["status"], state["phase"], state["reason"]) == ("failed", "pr", "the build failed")
    assert wait.detail.startswith("Your message to the builder produced 1 commit after the lane failed: it is not in the "
                                  "pushed branch. The lane failed, so nothing else was done with it.")
    assert wait.technical == "the build failed"  # requirement changed (round 6, item 2): the reason is kept apart, in plain words above
    assert before["build"] == state["build"]
