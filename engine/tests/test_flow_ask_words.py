"""The title and body of each ask put the engine's state into plain words."""

import json
import re
from datetime import datetime, timezone

import pytest

from flow_support import make_context
from leeway_support import account, make_helper
from sakura.budget import guard
from sakura.flow import ask_words, stop_words, budget_guard, checks, contract, pace_check, waits
from sakura.usage import leeway

from test_flow_pace import FakeAdvisor
from test_flow_waits import AWAITING, _row, _task

# Words of the engine that a person has no use for in a title or a body.
ENGINE_WORDS = ("gate", "task_review", "plan_review", "final_review", "budget-wait", "rows green", "row R", "step_accounts")
UNTIL = "2026-11-19T06:30:00+00:00"


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


def _ask(repo, gate, detail="", **state):
    _task(repo, "demo", [_row("R1", "check"), _row("R2", "check")], status=AWAITING, phase=state.pop("phase", "build"),
          reason=f"awaiting approval: sakura flow approve demo {gate}{detail}", **state)
    (wait,) = waits.collect([repo])
    return wait


def _assert_plain(wait):
    for word in ENGINE_WORDS:
        assert word not in wait.title and word not in wait.detail, (word, wait.title, wait.detail)


def test_Ask_SpecGate_TitleSaysToReadTheSpecAndApproveIt(repo):
    wait = _ask(repo, "spec", " (read /x/spec.md first)", phase="intake")
    assert wait.title == "Read the spec and approve it"
    _assert_plain(wait)


def test_Ask_ChecksGate_TitleCountsTheChecksAndTheBodyListsTheCommands(repo):
    wait = _ask(repo, "checks", " (read the contract first)", phase="contract",
                **{checks.PENDING_KEY: {"commands": ["pytest -q", "python3 e2e/run.py --all"]}})
    assert wait.title == "Run these 2 checks?"
    first, blank, *commands = wait.detail.split("\n")
    assert first == ask_words.CHECKS_PROVE and blank == "" and commands == ["- pytest -q", "- python3 e2e/run.py --all"]
    _assert_plain(wait)


def test_Ask_ChecksGateWithOneCommand_SaysCheckInTheSingular(repo):
    wait = _ask(repo, "checks", phase="contract", **{checks.PENDING_KEY: {"commands": ["pytest -q"]}})
    assert wait.title == "Run this check?" and wait.detail.endswith("- pytest -q")


def _waiting(rule, action="defer", account_id="work", until=UNTIL):
    return {"waiting": {"on": "budget", "action": action, "until": until, "rule": rule, "account": account_id,
                        "reason": "the task_review step lands above 90% of its week at the reset"}}


def _two_accounts(tmp_path, monkeypatch):
    rows = [account("personal", label="Personal"), account("work", label="Work account")]
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "h", rows=rows)))


def test_Ask_BudgetWaitOnPace_NamesTheAccountItsLineAndWhenItStartsAgain(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    wait = _ask(repo, "budget-wait", **_waiting("pace"))
    assert wait.title == "Start now on Work account?"
    assert "Work account is forecast to end its week above the pace line you set." in wait.detail
    expected = datetime.fromisoformat(UNTIL).astimezone().strftime(ask_words.TIME_FORMAT)
    assert f"starts the lane again on {expected}" in wait.detail
    _assert_plain(wait)


@pytest.mark.parametrize(("rule", "words"), [("week", "used its week down to the reserve"),
                                             ("five_hour", "5-hour window is nearly full"),
                                             ("pacing", "waits for the night")])
def test_Ask_BudgetWaitOnTheOtherRules_SaysWhyInPlainWords(repo, tmp_path, monkeypatch, rule, words):
    _two_accounts(tmp_path, monkeypatch)
    wait = _ask(repo, "budget-wait", **_waiting(rule))
    assert wait.title == "Start now on Work account?" and words in wait.detail
    _assert_plain(wait)


def test_Ask_BudgetWaitWithNoKnownAccount_SaysTheAccount(repo):
    wait = _ask(repo, "budget-wait", **_waiting("week", account_id=""))
    assert wait.title == "Start now on the account?"


def test_Ask_BudgetWaitThatOnlyThePersonCanSettle_AsksWhetherToContinue(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    wait = _ask(repo, "budget-wait", **_waiting("staleness", action="ask", until=None))
    assert wait.title == "Continue without knowing how much of Work account's week is used?"
    _assert_plain(wait)


def test_Ask_BudgetPreflight_AsksWhetherToStartTheBuildAndKeepsTheEstimate(repo):
    wait = _ask(repo, "budget", " (about 12% of your week; read /x/plan.md first)")
    assert wait.title == "Start the build?" and wait.detail == "about 12% of your week"
    _assert_plain(wait)


def test_Ask_Overlap_NamesTheLaneAndTheFilesWithoutAHead(repo):
    overlap = {"lane": "other-lane", "files": ["app.py", "api.py"], "head": "0123456789abcdef"}
    wait = _ask(repo, "overlap", overlap=overlap, phase="pr")
    assert wait.title == "Delivery overlaps another lane"
    # requirement changed (round 6, N29): one sentence, the lanes named in quotes, the files sorted
    assert wait.detail == '"other-lane" delivered before this lane and changed api.py and app.py, which this lane also changes.'
    assert "0123456789" not in wait.detail


def _overlap_ask(repo, conflicts):
    return _ask(repo, "overlap", overlap={"lane": conflicts[0]["lane"], "files": conflicts[0]["files"], "head": "0123456789abcdef",
                                          "conflicts": conflicts}, phase="pr")


def test_Ask_OverlapWithTwoLanesThatChangedTheSameFile_IsOneSentenceNamingBothAndTheFileOnce(repo):
    both = [{"lane": "Lines and characters", "files": ["tests/test_tally.py"], "head": "a" * 16},
            {"lane": "Top and unique words", "files": ["tests/test_tally.py"], "head": "b" * 16}]

    wait = _overlap_ask(repo, both)

    assert wait.detail == ('"Lines and characters" and "Top and unique words" delivered before this lane and both changed '
                           "tests/test_tally.py, which this lane also changes.")
    assert "first" not in wait.detail and wait.detail.endswith(", which this lane also changes.")


def test_Ask_OverlapWithLanesThatChangedOtherFiles_GroupsTheFilesWithTheLanesThatChangedThem(repo):
    mixed = [{"lane": "A", "files": ["app.py", "api.py"], "head": "a" * 16},
             {"lane": "B", "files": ["api.py"], "head": "b" * 16},
             {"lane": "C", "files": ["api.py", "cli.py"], "head": "c" * 16}]

    wait = _overlap_ask(repo, mixed)

    assert wait.detail == ('"A", "B" and "C" delivered before this lane and changed files this lane also changes: '
                           'api.py ("A", "B" and "C"); app.py ("A"); cli.py ("C").')


def test_Ask_FinalReview_SaysWhatTheReviewerWantsAndThatThePersonDecides(repo):
    wait = _ask(repo, "final-review", **{"final_review": {"rounds": 3}})
    assert wait.title == "Deliver although the review still wants changes?"
    assert wait.detail == "The reviewer still asked for changes after 3 rounds. You decide whether to deliver the branch as it is."
    _assert_plain(wait)


def test_Ask_FinalReview_SaysOneCountOfRoundsAndCarriesTheReviewersFindings(repo):
    findings = ["src/count.py: the two tasks name the same thing differently", "T2 and review-3 left /home/me/work/x/notes.md stale"]
    wait = _ask(repo, "final-review", **{"final_review": {"rounds": 2, "asked_rounds": 1, "findings": findings}})

    assert wait.detail == "The reviewer still asked for changes after 1 round. You decide whether to deliver the branch as it is."
    assert wait.findings == ("src/count.py: the two tasks name the same thing differently",
                             "A task and a run left notes.md stale")
    _assert_plain(wait)


def test_Ask_FinalReview_ABranchThatNeverHadFindingsKept_HasNone(repo):
    wait = _ask(repo, "final-review", **{"final_review": {"rounds": 3}})

    assert wait.findings == () and "after 3 rounds" in wait.detail


def test_Ask_OpusGate_SaysATaskNeedsTheStrongestModel(repo):
    wait = _ask(repo, "opus:t3")
    assert wait.title == "Use the strongest model for a task?" and wait.detail == ask_words.OPUS_DETAIL
    _assert_plain(wait)


def test_Ask_AnyOtherGate_NeverShowsItsName(repo):
    wait = _ask(repo, "mystery")
    assert wait.title == "Approve to continue"
    _assert_plain(wait)


def test_Ask_BlockedLane_SaysItIsStuck(repo):
    _task(repo, "demo", status="blocked", reason="push stalled; see the log")
    (wait,) = waits.collect([repo])
    assert wait.title == "This lane is stuck" and wait.detail == "Pushing the branch took too long. Check the connection to the remote, then resume the lane."


def test_Ask_OwnerRow_TitleSaysWhatToTryAndNotTheRowId(repo):
    _task(repo, "demo", [_row("R1", flow="open the page and look")], phase="pr")
    (wait,) = waits.collect([repo])
    assert wait.title == "Try it yourself: open the page and look"
    assert "R1" not in wait.title


# -- the budget guard keeps the account of the wait ----------------------------------------------------------------

def test_Park_ADecisionOfAnAccount_KeepsItInTheWaitingRecord(repo):
    ctx = make_context(repo, task="a")
    now = datetime(2026, 11, 18, 12, 0, tzinfo=timezone.utc)
    budget_guard.park(ctx.store, guard.Decision(guard.DEFER, None, "why", pace_check.PACE_RULE, "work"), now)
    assert ctx.store.read_state()["waiting"]["account"] == "work"


def test_Guard_APaceWait_NamesTheAccountItWaitsOn(repo):
    from test_flow_pace_accounts import _another_lane_runs, _evaluate

    _another_lane_runs(repo)
    decision = _evaluate(make_context(repo, task="a"), FakeAdvisor({"personal": 95}))
    assert (decision.action, decision.account) == (guard.DEFER, "personal")


def test_OwnerRowTitle_UsesTheFirstLineAndCutsALongOne():
    assert ask_words.owner_row_title("open the page\nthen close it") == "Try it yourself: open the page"
    assert len(ask_words.owner_row_title("x" * 500)) == len("Try it yourself: ") + ask_words.OWNER_ROW_TITLE_CHARACTERS
    assert ask_words.owner_row_title("") == "Try it yourself"


def test_Ask_OverlapWithATitledLane_NamesItByItsTitle(repo):
    from test_cockpit_snapshot import create_task

    create_task(repo, "laneb", title="Rewrite the parser")
    overlap = {"lane": f"{repo.name}/laneb", "files": ["app.py"], "head": "0123456789abcdef"}
    wait = _ask(repo, "overlap", overlap=overlap, phase="pr")
    assert wait.detail == '"Rewrite the parser" delivered before this lane and changed app.py, which this lane also changes.' 


# -- the stuck ask says what is wrong in words, never the engine's own line ------------------------------------------------------


def _blocked_by(repo, reason, prover="check", **state):
    _task(repo, "demo", [_row("R1", prover), _row("R2", prover)], status="blocked", phase="pr", reason=reason, **state)
    (wait,) = [found for found in waits.collect([repo]) if found.kind == "blocked"]
    return wait


def _real_precondition_reason(repo, rows, head="h"):
    from sakura.flow.phases import pull_request

    ctx = make_context(repo, task="probe")
    return pull_request._precondition({}, rows, ctx.steps, head, "probe")


def test_Ask_BlockedOnOpenRows_SaysTheChecksHaveNotPassedWithoutTheWordRows(repo):
    reason = _real_precondition_reason(repo, [_row("R1", "check")])
    wait = _blocked_by(repo, reason)
    assert wait.title == ask_words.BLOCKED_TITLE and wait.detail == ask_words.CHECKS_NOT_PASSED
    assert "row" not in wait.detail.lower() and "contract" not in wait.detail.lower()


def test_Ask_BlockedOnTheFullTests_SaysTheProjectTestsHaveNotPassed(repo):
    reason = _real_precondition_reason(repo, [])
    wait = _blocked_by(repo, reason)
    assert reason == "full tests have not passed on the current head"
    assert wait.detail == ask_words.TESTS_NOT_PASSED and "head" not in wait.detail


def test_Ask_BlockedOnTheFullTestsOfAnOldConfiguration_SaysTheSame(repo):
    wait = _blocked_by(repo, "full tests have not passed on the current test configuration")
    assert wait.detail == ask_words.TESTS_NOT_PASSED


def test_Ask_BlockedOnOwnerRows_SaysHowManyThingsAreLeftToTryAndNamesNoCommand(repo):
    from sakura.flow import owner_marks

    reason = owner_marks.blocked_reason("demo", [_row("R1", "owner"), _row("R2", "owner")])
    wait = _blocked_by(repo, reason, prover="owner")
    assert wait.detail == ask_words.owner_rows_detail(2) and "sakura" not in wait.detail and "R1" not in wait.detail


def test_Ask_BlockedWithEveryOwnerRowMarked_SaysToResumeTheLane(repo):
    from sakura.flow import owner_marks

    wait = _blocked_by(repo, owner_marks.ALL_MARKED_REASON)
    assert wait.detail == ask_words.ALL_TRIED_DETAIL


def test_BlockedDetail_AReasonItDoesNotKnow_GetsTheGeneralSentenceNotTheEnginesWords():
    # Requirement changed (round 6, item 2): no reason reaches a person in the engine's words.
    assert ask_words.blocked_detail("something new happened") == stop_words.GENERIC


# -- an engine error that becomes an ask: a plain sentence, with the raw detail kept apart

GH_LOOKUP = ("pull request lookup failed: To get started with GitHub CLI, please run:  gh auth login; "
             "see /home/someone/engine/flow/lane/runs/operations/pull-request-lookup.log")


def test_Ask_AFailedPullRequestLookupBecauseGhIsNotLoggedIn_SaysWhatToDo_WithoutAHostPath(repo):
    _task(repo, "demo", status="blocked", reason=GH_LOOKUP)

    (wait,) = waits.collect([repo])

    assert wait.detail == "GitHub CLI is not logged in: run gh auth login in a terminal, then resume the lane."
    assert "/Users" not in wait.detail and ".log" not in wait.detail
    assert wait.technical == GH_LOOKUP, "the raw line stays, apart"


def test_Ask_AnotherFailedPullRequestLookup_SaysWhatFailedAndWhatToCheck(repo):
    _task(repo, "demo", status="blocked", reason="pull request lookup failed: HTTP 502; see /home/someone/x/pull-request-lookup.log")

    (wait,) = waits.collect([repo])

    assert wait.detail.startswith("Looking up the lane's pull request on GitHub failed") and "gh auth status" in wait.detail
    assert "/Users" not in wait.detail and "HTTP 502" in wait.technical


def test_Ask_AnUnknownEngineReasonWithAHostPath_ShowsItWithoutThePath(repo):
    _task(repo, "demo", status="blocked", reason="something new happened; see /home/someone/x/runs/push.log")

    (wait,) = waits.collect([repo])

    assert wait.detail == stop_words.GENERIC and "/Users" not in wait.detail
    assert wait.technical == "something new happened; see /home/someone/x/runs/push.log"


def test_Ask_AKnownPlainReason_HasNoSeparateTechnicalDetail(repo):
    plain = "The planner ended without writing the contract and gave no reason. Resume the lane to ask it again, or stop it."
    _task(repo, "demo", status="blocked", reason=plain)

    (wait,) = waits.collect([repo])

    assert wait.detail == plain and wait.technical == ""


# -- a lane stuck in the build says what the builder hit, never the task's id

REFUSED = ("T1 still needs changes after 2 fix rounds: builder made no commit: the commit check refused its commits: "
           "commits on protected branch 'main' are refused")


def test_Ask_AStuckBuildWhoseCommitsWereRefused_NamesTheCauseAndTheChecksOwnReason(repo):
    _task(repo, "demo", status="blocked", reason=REFUSED)

    (wait,) = waits.collect([repo])

    assert wait.detail.startswith("The builder's commits were refused by the commit check, as its tool printed: "
                                  "\"commits on protected branch 'main' are refused\"")
    assert "resume the lane" in wait.detail and "T1" not in wait.detail
    assert wait.technical == REFUSED


def test_Ask_AStuckBuildThatMadeNoCommit_SaysSoWithoutTheTaskId(repo):
    _task(repo, "demo", status="blocked", reason="T3 still needs changes after 2 fix rounds: builder made no commit")

    (wait,) = waits.collect([repo])

    assert wait.detail.startswith("The builder made no commit after 2 fix rounds") and "T3" not in wait.detail


def test_Ask_AStuckBuildWithOtherFindings_KeepsThemAndDropsTheTaskId(repo):
    _task(repo, "demo", status="blocked",
          reason="T2 still needs changes after 1 fix round: focused tests failed (sakura test focused); fix them and commit")

    (wait,) = waits.collect([repo])

    assert wait.detail.startswith("The builder's work still needed changes after 1 fix round: focused tests failed")
    assert "T2" not in wait.detail


# -- an ask from a reviewer's verdict carries the reviewer's reason in two sentences at most, in plain words

CONTRACT_REFUSED = (
    "the code reviewer did not approve the contract flows: e2e/tally_cli.py run_on_file: TemporaryDirectory("
    "ignore_cleanup_errors=True) needs Python 3.10+; on older interpreters every --lines case crashes with TypeError "
    "instead of checking behaviour; e2e/tally_cli.py case_import_guard: uses assert for count_words, which is stripped "
    "under -O, so R11 can pass for a broken count_words; print the value and compare explicitly; "
    "/home/someone/work/tally/e2e/tally_cli.py run_tally inherits the host environment, so R1 to R10 depend on it")
PLAN_REFUSED = ("plan review still asked for changes after 2 fix rounds (3 reviews): T1 and T2 both edit src/count.py; "
                "split the shared function out first")
STUCK_REVIEWED = ("T2 still needs changes after 2 fix rounds: the reviewer found that review-3 left "
                  "src/lines/counter.py reading the whole file into memory, which breaks on large inputs")


def _reviewer_detail(repo, reason):
    _task(repo, "demo", status="blocked", reason=reason)
    (wait,) = waits.collect([repo])
    return wait


@pytest.mark.parametrize("reason", [CONTRACT_REFUSED, PLAN_REFUSED, STUCK_REVIEWED])
def test_Ask_AReviewersVerdict_HasPlainWordsInDetailAndTheRawLineInTechnical(repo, reason):
    wait = _reviewer_detail(repo, reason)

    assert wait.detail and wait.technical == reason
    for leak in ("e2e/", "src/", "/Users", "T1", "T2", "review-3", "build-"):
        assert leak not in wait.detail, (leak, wait.detail)
    assert len(wait.detail) <= 400


def test_Ask_TheContractReviewersVerdict_SaysWhoWantedWhatInAtMostTwoSentencesOfItsOwn(repo):
    wait = _reviewer_detail(repo, CONTRACT_REFUSED)

    assert wait.detail.startswith("The reviewer did not approve the contract's checks. ")
    reason = wait.detail.removeprefix("The reviewer did not approve the contract's checks. ")
    assert reason.startswith("tally_cli.py run_on_file: TemporaryDirectory(ignore_cleanup_errors=True) needs Python 3.10+;")
    assert len([part for part in re.split(r"(?<=[.;]) +", reason) if part]) <= 2


def test_Ask_TheSameVerdictAskedAgain_HasPlainWordsAgain(repo):
    task = _task(repo, "demo", status="blocked", reason=CONTRACT_REFUSED)
    (first,) = waits.collect([repo])
    again = CONTRACT_REFUSED.replace("tally_cli.py run_on_file", "tally_cli.py run_file")
    task.store.update_state(lambda state: state.update(status="blocked", reason=again))

    (second,) = waits.collect([repo])

    assert first.detail.startswith(f"{ask_words.CONTRACT_REFUSED_WORDS} ") and second.detail.startswith(first.detail[:60])
    assert second.detail != first.detail and second.technical == again


def test_Ask_APlanReviewsVerdict_NamesThePlanAndKeepsTheReason(repo):
    wait = _reviewer_detail(repo, PLAN_REFUSED)

    assert wait.detail.startswith("The plan reviewer still wanted changes after 2 fix rounds. ")
    assert "split the shared function out first" in wait.detail


def test_Ask_StuckLane_PreservesCheckIdsAndCapitalizesReviewerSentences(repo):
    reason = "the code reviewer did not approve the contract flows: checks R1-R8 need coverage. try R11 too."
    wait = _reviewer_detail(repo, reason)

    assert wait.detail == "The reviewer did not approve the contract's checks. Checks R1-R8 need coverage. Try R11 too."
    assert wait.technical == reason


def test_PlainFindings_PreservesCheckIdsAndCapitalizesSentences():
    assert ask_words.plain_findings(["run R1-R8 again. check R11 too."]) == ("Run R1-R8 again. Check R11 too.",)


# -- a contract with nothing new to prove is a plain ask the person can answer

NOTHING_NEW = "contract has no non-baseline row: nothing proves the new behavior"


def test_Ask_AContractWithNothingNewToProve_SaysSoPlainlyAndNamesTheChoices(repo):
    _task(repo, "demo", status="blocked", phase="contract", reason=NOTHING_NEW)

    (wait,) = waits.collect([repo])

    assert wait.detail.startswith("The contract found nothing new to prove: the project may already do what the lane asks.")
    assert "resume the lane with a note" in wait.detail.lower() and "stop" in wait.detail.lower()
    assert wait.technical == NOTHING_NEW


def test_Resume_ALaneBlockedForNothingNewToProve_IsResumableNotFutile(repo):
    from sakura.flow import resume

    ctx = _task(repo, "demo", status="blocked", phase="contract", reason=NOTHING_NEW)

    assert resume.retry_futile(ctx.store.read_state()) == ""


# -- a finding is kept whole up to a generous limit, and cut at a word with an ellipsis

def test_PlainFindings_AFindingOfSeveralHundredCharacters_IsKeptWhole():
    finding = "src/count.py: " + "the two tasks name the same thing differently and the readers cannot tell which is meant; " * 6

    assert ask_words.plain_findings([finding]) == (finding.strip(),)
    assert len(finding) > 400


def test_PlainFindings_AFindingBeyondTheLimit_IsCutAtAWordAndEndsWithAnEllipsis():
    finding = "word " * 400

    [shown] = ask_words.plain_findings([finding])

    assert shown.endswith("word...") and len(shown) <= ask_words.MAX_FINDING_CHARACTERS + 3
    assert not shown.endswith(" ...") and "wor..." not in shown
