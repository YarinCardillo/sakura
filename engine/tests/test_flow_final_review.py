"""The final review: one reviewer reads the whole branch after the last build task, before the checks."""

from sakura.flow import config as flow_config, exits, inbox
from sakura.flow.cli import main as flow_main
from sakura.flow.store import TaskStore
from sakura.flow import final_head, runner, waits
from sakura.flow.phases import build

from flow_support import make_context
from test_flow_build import APPROVE, _commit, _ctx, _task, _use, no_hook_install  # noqa: F401  (the fixture is autouse)

FINAL_FINDING = {"verdict": "REVISE", "findings": ["the two tasks name the same thing differently"]}


def _with_final_review(ctx, rounds=1, on=True):
    object.__setattr__(ctx, "steps", flow_config.PresetSteps(True, True, True, on))
    ctx.config.values["knobs"]["final_review_rounds"] = rounds
    return ctx


def _final_reviews(runner_double):
    template = runner.load_template("final_review")
    return [request for request in runner_double.of("code_reviewer") if request.template == template]


def _fixes(runner_double):
    template = runner.load_template("final_fix")
    return [request for request in runner_double.of("builder") if request.template == template]


def _builds(tmp_path, repo, reviews, rounds=1, on=True):
    ctx = _with_final_review(_ctx(repo, tmp_path, [_task("T1")]), rounds, on)
    double = _use(ctx, _commit, reviews)
    return ctx, double


def test_FinalReview_On_ReadsTheWholeBranchOnceAfterTheLastTask(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, APPROVE])

    outcome = build.run(ctx)

    assert outcome.kind == "next"
    [final] = _final_reviews(double)
    assert final.values["start"] == ctx.store.read_state()["base_commit"]
    assert final.values["branch"] == final_head.recorded(ctx.store.read_state())
    assert "final review round" not in ctx.store.ledger.read_text()


def test_FinalReview_Off_GoesStraightToTheChecks(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE], on=False)

    assert build.run(ctx).kind == "next"
    assert _final_reviews(double) == []


def test_FinalReview_Approved_IsWrittenToTheLedger(repo, tmp_path):
    ctx, _ = _builds(tmp_path, repo, [APPROVE, APPROVE])

    build.run(ctx)

    assert ctx.store.read_state()["final_review"]["approved_head"] == final_head.recorded(ctx.store.read_state())


def test_FinalReview_SendBack_RunsOneMoreBuildRoundAndAReviewOfTheNewHead(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING, APPROVE])

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    [fix] = _fixes(double)
    assert fix.extra_env["SAKURA_FLOW_BUILD_TASK"] == "final-1"
    assert "the two tasks name the same thing differently" in fix.values["findings"]
    first, second = _final_reviews(double)
    assert second.values["branch"] != first.values["branch"]
    assert second.values["branch"] == final_head.recorded(ctx.store.read_state())
    assert first.values["start"] == second.values["start"]
    assert "final review sent back to build: round 1" in ctx.store.ledger.read_text()


def test_FinalReview_StillRevisingAfterTheRounds_AsksThePersonAtTheGate(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING, FINAL_FINDING], rounds=1)

    outcome = build.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "final-review")
    assert len(_fixes(double)) == 1 and len(_final_reviews(double)) == 2


def test_FinalReview_TheAskKeepsTheFindingsAndOneCountOfFixRounds_InTheSingularForOne(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING, FINAL_FINDING], rounds=1)

    outcome = build.run(ctx)

    state = ctx.store.read_state()["final_review"]
    assert state["asked_rounds"] == 1 and state["findings"] == FINAL_FINDING["findings"]
    assert "after 1 round;" in outcome.reason and "1 rounds" not in outcome.reason


def test_FinalReview_ZeroRounds_AsksAtOnceWithoutABuildRound(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING], rounds=0)

    outcome = build.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "final-review")
    assert _fixes(double) == []


def test_FinalReview_AfterThePersonApproves_TheLaneGoesOnWithoutAnotherReview(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING], rounds=0)
    build.run(ctx)
    ctx.store.update_state(lambda state: state["approvals"].update({"final-review": {"at": "now"}}))

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert len(_final_reviews(double)) == 1


def test_FinalReview_RunTwiceOnTheSameApprovedHead_ReviewsOnce(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, APPROVE])
    build.run(ctx)

    assert build.run(ctx).kind == "next"
    assert len(_final_reviews(double)) == 1


def test_FinalReview_ABuilderThatMakesNoCommit_BlocksInPlainWordsWithWhatItSaidAndTheFindings(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING])
    double.builder = lambda request: _commit(request) if request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T1" else None
    double.final = "I could not tell which names you meant."

    outcome = build.run(ctx)

    assert outcome.kind == "blocked"
    assert outcome.reason == ("The final review asked for changes, but the builder ended without making any. It said: "
                              "\"I could not tell which names you meant.\" Resume the lane to ask it again, or stop it.")
    assert "final-1" not in outcome.reason and "no commit" not in outcome.reason
    assert "final-1: builder made no commit" in ctx.store.ledger.read_text(), "the raw line stays in the ledger"
    ctx.store.update_state(lambda state: state.update(status="blocked", reason=outcome.reason, contract_sha256=""))
    (ask,) = [wait for wait in waits.collect([repo]) if wait.kind == "blocked"]
    assert ask.detail == outcome.reason and ask.findings == ("The two tasks name the same thing differently",)


def test_FinalReview_ABuilderThatMakesNoCommitAndSaysNothing_SaysThatToo(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, FINAL_FINDING])
    double.builder = lambda request: _commit(request) if request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T1" else None

    outcome = build.run(ctx)

    assert outcome.reason == ("The final review asked for changes, but the builder ended without making any and gave no "
                              "reason. Resume the lane to ask it again, or stop it.")


def test_FinalReview_AReviewerThatChangesTheCopy_Blocks(repo, tmp_path):
    ctx, double = _builds(tmp_path, repo, [APPROVE, APPROVE])
    final = runner.load_template("final_review")
    double.on_review = lambda request: (request.cwd / "extra.txt").write_text("x") if request.template == final else None

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and "The code reviewer should only read the project" in outcome.reason  # plain words (round 6, N28)


def test_FinalReview_PresetSwitchQueuedDuringTheLastBuilder_IsTakenInBeforeTheReviewDecision(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    ctx.config.values["preset"] = "solo"
    object.__setattr__(ctx, "steps", flow_config.steps(ctx.config.values))
    ctx.config.values["knobs"]["final_review_rounds"] = 1
    double = _use(ctx, _commit, [APPROVE])
    queued = []

    def builder(request):
        _commit(request)
        if not queued:
            queued.append(inbox.enqueue(ctx.store, "preset", "lean", actor="cli:alice"))

    double.builder = builder

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert len(_final_reviews(double)) == 1


def _asking_lane(repo):
    return make_context(repo, task="a", state={
        "status": "awaiting_approval", "phase": "build", "final_review": {"asked_head": "abc"},
        "reason": "awaiting approval: sakura flow approve a final-review (the final review still asked for changes)"})


def test_Cli_ApproveFinalReview_RecordsTheApprovalTheBuildPhaseReadsBack(repo, monkeypatch):
    monkeypatch.chdir(repo)
    _asking_lane(repo)

    assert flow_main(["approve", "--", "a", "final-review"]) == exits.OK

    assert "final-review" in TaskStore(repo, "a").read_state()["approvals"]


def test_Cli_ApproveFinalReview_WhenNoFinalReviewAsks_IsRefused(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    make_context(repo, task="a", state={"status": "running", "phase": "build"})

    assert flow_main(["approve", "--", "a", "final-review"]) == exits.USAGE
    assert "no final review waits for approval" in capsys.readouterr().out
