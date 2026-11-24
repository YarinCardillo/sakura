"""Pace: where each account lands at its reset, decided with the advisor of Leeway before a step runs."""

import json
from datetime import datetime, timezone

import pytest

from sakura import engine_home
from sakura.budget import guard
from sakura.cockpit import actions, api, host_actions
from sakura.flow import budget_guard, config, exits, inbox, pace, pace_accounts, pace_check, registry, rules, rules_cli
from sakura.flow.cli import main as flow_main

from conftest import git
from flow_support import make_context
from test_cockpit_actions import Spawns, flow_item, invoke
from test_cockpit_snapshot import make_snapshot
from test_flow_engine_budget import budget_clock, set_snapshot  # noqa: F401  (the clock set_snapshot is dated by)

RETRY_AT = "2026-11-19T06:30:00Z"


class FakeAdvisor:
    """Leeway's advisor as far as pace needs it: each account lands at `landing[id]` percent of its week at its reset.

    The first account asked for is judged against the line; with none named, the accounts are tried in order, and a
    later one under the line answers `switch`. `reserve` maps an account to the one Leeway's own policy falls back to.
    """

    def __init__(self, landing, reserve=None, verdict=None):
        self.landing, self.reserve, self.verdict = landing, reserve or {}, verdict
        self.calls = []

    def __call__(self, account, target, candidates):
        self.calls.append((account, target, candidates))
        if self.verdict:
            return {"verdict": self.verdict, "account": "", "retry_at": None, "target_used": target}
        order = [account] if account else list(self.landing)
        order += [self.reserve[account]] if account in self.reserve else []
        for position, name in enumerate(order):
            if self.landing[name] <= target:
                return {"verdict": "go" if position == 0 else "switch", "account": name, "retry_at": None,
                        "target_used": target}
        return {"verdict": "wait", "account": order[0], "retry_at": RETRY_AT, "target_used": target}


def decide(advisor, lane="normal", level="normal", step_account=pace.PROJECT_ACCOUNT, allowed=()):
    return pace.decide(step_account, lane, level, advisor, allowed=allowed)


def test_Pace_Levels_AreTheLandingLinesOfTheWeek():
    assert pace.LEVELS == ("slowest", "slow", "normal", "fast", "fastest")
    assert [pace.TARGETS[level] for level in pace.LEVELS] == [50, 70, 90, 100, None]
    assert pace.LANE_LEVELS == pace.LEVELS, "a lane has the five paces the new-lane sheet offers"


def test_Decide_NormalAndAnAccountLandingAt95_WaitsWithTheAdvisorsTime():
    advisor = FakeAdvisor({"personal": 95})
    decision = decide(advisor)
    assert decision.kind == pace.WAIT
    assert decision.until == datetime(2026, 11, 19, 6, 30, tzinfo=timezone.utc)
    assert advisor.calls == [(None, 90, None)]


def test_Decide_FastestLane_GoesWithoutAskingTheAdvisor():
    advisor = FakeAdvisor({"personal": 150})
    assert decide(advisor, lane="fastest").kind == pace.GO and advisor.calls == []


def test_Decide_ALanesOwnPace_IsItsLineWhateverTheEnginesPaceIs():
    for engine_level in pace.LEVELS:
        advisor = FakeAdvisor({"personal": 85})
        assert decide(advisor, lane="slow", level=engine_level).kind == pace.WAIT
        assert advisor.calls == [(None, 70, None)]
    assert decide(FakeAdvisor({"personal": 85}), lane="normal", level="slowest").kind == pace.GO


def test_Decide_ALaneWithNoPaceOfItsOwn_FollowsTheEngine():
    advisor = FakeAdvisor({"personal": 60})
    assert decide(advisor, lane="", level="slowest").kind == pace.WAIT
    assert decide(advisor, lane="", level="slow").kind == pace.GO
    assert decide(FakeAdvisor({"personal": 99}), lane="", level="fastest").kind == pace.GO


def test_Pace_LaneLevel_IsTheLanesOwnElseTheEnginesAndNothingElse():
    assert pace.lane_level({"lane_pace": "fast", "pace": "slow"}) == "fast"
    assert pace.lane_level({"lane_pace": "", "pace": "slow"}) == "slow"


def test_Decide_StepOnTheProjectAccount_SwitchesToAnotherAccountUnderTheLine():
    advisor = FakeAdvisor({"personal": 95, "work": 40})
    decision = decide(advisor, allowed=("work",))
    assert (decision.kind, decision.account) == (pace.SWITCH, "work")


def test_Decide_StepPinnedToAnAccount_NeverSwitches():
    advisor = FakeAdvisor({"personal": 95, "work": 40}, reserve={"personal": "work"})
    decision = decide(advisor, step_account="personal")
    assert decision.kind == pace.WAIT and decision.account == "personal"
    assert advisor.calls == [("personal", 90, None)]


def test_Decide_NoEligibleAccount_IsDeny():
    assert decide(FakeAdvisor({}, verdict="deny")).kind == pace.DENY


def test_Decide_AWaitWithNoTime_LooksAgainAfterTheRecheckDelay():
    now = datetime(2026, 11, 18, 12, 0, tzinfo=timezone.utc)
    advisor = FakeAdvisor({"personal": 99})
    silent = lambda account, target, candidates: {**advisor(account, target, candidates), "retry_at": None}  # noqa: E731
    decision = pace.decide(pace.PROJECT_ACCOUNT, "normal", "normal", silent, now=now)
    assert decision.until == now + pace.RECHECK


@pytest.mark.parametrize(("lane", "level"), [("bogus", "normal"), ("normal", "bogus")])
def test_Decide_UnknownLevel_IsRefused(lane, level):
    with pytest.raises(pace.PaceError, match="bogus"):
        decide(FakeAdvisor({"personal": 1}), lane=lane, level=level)


def test_Admit_FewerLanesRunningThanTheMinimum_NeverWaits():
    waiting = pace.Decision(pace.WAIT, "personal", datetime(2026, 11, 19, tzinfo=timezone.utc), "over the line")
    assert pace.admit(waiting, others_running=1, lanes_min=2).kind == pace.GO
    assert pace.admit(waiting, others_running=2, lanes_min=2).kind == pace.WAIT
    assert pace.admit(waiting, others_running=0, lanes_min=1).kind == pace.GO
    assert pace.admit(pace.Decision(pace.DENY, "", None, "none"), others_running=0, lanes_min=2).kind == pace.DENY


# -- the guard: the reserve, the line, the minimum --------------------------------------------------------------

def _lane(repo, task, state=None, **overrides):
    return make_context(repo, task=task, state=state, overrides=overrides or None)


def _another_lane_runs(repo):
    """Pace holds a lane back only beyond `lanes_min` (1 by default): one other lane already runs."""
    registry.remember(repo)
    _lane(repo, "busy", state={"status": "running"})


def _evaluate(ctx, advisor, now=None, role="builder"):
    return budget_guard.evaluate(ctx.store, ctx.config, now, role=role, advisor=advisor)


def test_Guard_TheOnlyLane_RunsOverTheLineBecauseOneLaneIsTheLeastNumber(repo):
    registry.remember(repo)
    assert _evaluate(_lane(repo, "a"), FakeAdvisor({"personal": 95})).action == guard.RUN


def test_Guard_NormalLaneOverTheLine_DefersUntilTheAdvisorsTimeWithThePaceRule(repo):
    _another_lane_runs(repo)
    ctx = _lane(repo, "a")
    decision = _evaluate(ctx, FakeAdvisor({"personal": 95}))
    assert (decision.action, decision.rule) == (guard.DEFER, pace_check.PACE_RULE)
    assert decision.until == datetime(2026, 11, 19, 6, 30, tzinfo=timezone.utc)
    assert "90%" in decision.reason and "build step" in decision.reason


def test_Guard_FastestLane_RunsOverTheLine(repo):
    ctx = _lane(repo, "a", knobs={"lane_pace": "fastest"})
    assert _evaluate(ctx, FakeAdvisor({"personal": 120})).action == guard.RUN


def test_Guard_ALaneThatChoseSlow_WaitsAtTheSlowLineEvenWhenTheEngineIsFast(repo):
    _another_lane_runs(repo)
    ctx = _lane(repo, "a", knobs={"lane_pace": "slow", "pace": "fast"})
    decision = _evaluate(ctx, FakeAdvisor({"personal": 80}))
    assert (decision.action, decision.rule) == (guard.DEFER, pace_check.PACE_RULE) and "70%" in decision.reason


def test_Guard_TheReserveStillStopsAFastLane(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = _lane(repo, "a", knobs={"lane_pace": "fastest"}, budget={"account": "a"})
    decision = _evaluate(ctx, FakeAdvisor({"a": 10}))
    assert (decision.action, decision.rule) == (guard.PAUSE, guard.WEEK_RULE)


def test_Guard_TheReserveStopsAFastestEngineToo(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = _lane(repo, "a", knobs={"pace": "fastest"}, budget={"account": "a"})
    assert _evaluate(ctx, FakeAdvisor({"a": 10})).action == guard.PAUSE


def test_Guard_AStepPinnedToAnAccount_IsJudgedOnThatAccount(repo, tmp_path, monkeypatch):
    from sakura.usage import leeway
    from leeway_support import account, make_helper
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "h", rows=[account("personal"), account("work")])))
    _another_lane_runs(repo)
    ctx = _lane(repo, "a", step_accounts={"build": "work"})
    advisor = FakeAdvisor({"personal": 10, "work": 99})
    assert _evaluate(ctx, advisor, role="builder").action == guard.DEFER
    assert advisor.calls == [("work", 90, None)]
    assert _evaluate(ctx, FakeAdvisor({"personal": 10, "work": 99}), role="planner").action == guard.RUN


def test_Guard_ASwitchRunsAndKeepsTheChosenAccountForTheStep(repo):
    _another_lane_runs(repo)
    pace_accounts.set_allowed(repo, ["work"], None)
    ctx = _lane(repo, "a")
    decision = _evaluate(ctx, FakeAdvisor({"personal": 95, "work": 40}))
    assert decision.action == guard.RUN
    assert ctx.store.read_state()["budget"]["pace_account"] == {"build": "work"}
    assert "[pace] build moves to the account work" in ctx.store.ledger.read_text()


def test_Guard_NoEligibleAccount_AsksThePerson(repo):
    _another_lane_runs(repo)
    decision = _evaluate(_lane(repo, "a"), FakeAdvisor({}, verdict="deny"))
    assert decision.action == guard.ASK and decision.rule == pace_check.PACE_RULE


def test_Guard_WithoutAnAdvisor_OnlyTheReserveRulesApply(repo):
    assert _evaluate(_lane(repo, "a"), None).action == guard.RUN


def test_Guard_AnAdvisorThatFails_RunsAndSaysSoOnce(repo):
    def broken(account, target, candidates):
        raise pace.AdvisorUnavailable("the advisor did not answer")

    ctx = _lane(repo, "a")
    assert _evaluate(ctx, broken).action == guard.RUN and _evaluate(ctx, broken).action == guard.RUN
    assert ctx.store.ledger.read_text().count("[pace] the advisor did not answer") == 1


def test_Guard_LanesMin_KeepsTheLeastNumberOfLanesRunning(repo):
    registry.remember(repo)
    waiting = FakeAdvisor({"personal": 95})
    first = _lane(repo, "first", knobs={"lanes_min": 2})
    _lane(repo, "second", state={"status": "running"})
    assert _evaluate(first, waiting).action == guard.RUN          # one other lane runs, two is the least
    _lane(repo, "third", state={"status": "running"})
    assert _evaluate(first, waiting).action == guard.DEFER        # two others run already


def test_Guard_LanesMin_DoesNotCountAWaitingLaneAsRunning(repo):
    registry.remember(repo)
    _lane(repo, "b", state={"status": "running", "waiting": {"on": "budget", "action": "defer"}})
    _lane(repo, "c", state={"status": "awaiting_approval"})
    first = _lane(repo, "first", knobs={"lanes_min": 1})
    assert _evaluate(first, FakeAdvisor({"personal": 95})).action == guard.RUN


def test_Guard_ReleasingAPaceWait_LiftsThePaceRuleOnly(repo, tmp_path):
    _another_lane_runs(repo)
    state = {"budget": {"released_until": "2099-01-01T00:00:00+00:00", "released_rule": pace_check.PACE_RULE}}
    assert _evaluate(_lane(repo, "a", state=state), FakeAdvisor({"personal": 95})).action == guard.RUN


# -- the engine's pace and a lane's pace -------------------------------------------------------------------------

def test_Config_PaceKnobs_TheEngineDefaultsToNormalAndALaneToTheEngines():
    knobs = config.load(None).values["knobs"]
    assert (knobs["pace"], knobs["lane_pace"]) == ("normal", "")


def test_Config_UnknownPaceLevel_IsRefused():
    (engine_home.root() / "config.toml").write_text('[knobs]\npace = "warp"\n')
    with pytest.raises(config.FlowConfigError, match="knobs.pace must be one of slowest"):
        config.load(None)


def test_Config_TheOldDayPace_IsIgnoredWithOneWarning():
    (engine_home.root() / "config.toml").write_text("[knobs]\npace_day_pct = 10\n")
    loaded = config.load(None)
    assert len(loaded.warnings) == 1 and "pace_day_pct" in loaded.warnings[0] and "sakura engine pace" in loaded.warnings[0]
    assert config.load(None, overrides=None).warnings == loaded.warnings
    assert config.load(None).values["knobs"]["pace"] == "normal"


def test_Config_NoOldDayPace_HasNoWarning():
    assert config.load(None).warnings == []


def test_Guard_TheOldDayPaceKnob_NoLongerDefersAnything(repo, tmp_path):
    set_snapshot(tmp_path, week=40.0, trend=[(24 * 1.0, 10.0)])
    ctx = _lane(repo, "a", knobs={"pace_day_pct": 5}, budget={"account": "a"})
    assert _evaluate(ctx, None).action == guard.RUN


def test_RulesSetPace_KeepsTheLevelApartFromThePresetEdits():
    rules.set_pace("fast", None)
    assert config.load(None).values["knobs"]["pace"] == "fast" and rules.is_edited() is False
    rules.set("subagents", "4", None)
    rules.clear(None)
    assert config.load(None).values["knobs"]["pace"] == "fast"


def test_RulesSetPace_UnknownLevel_IsRefusedAndNothingIsKept():
    with pytest.raises(rules.RuleRefused, match="slowest, slow, normal, fast, fastest"):
        rules.set_pace("warp", None)
    assert config.load(None).values["knobs"]["pace"] == "normal"


def test_Cli_EnginePace_SetsTheLevelAndRefusesAnUnknownOne(capsys):
    assert rules_cli.main(["pace", "--actor=cli:me", "--", "slow"]) == exits.OK
    assert config.load(None).values["knobs"]["pace"] == "slow"
    assert rules_cli.main(["pace", "--", "warp"]) == exits.USAGE
    assert "slowest" in capsys.readouterr().out


def test_Cli_FlowPace_QueuesTheLanePaceForTheEngineToApply(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    ctx = _lane(repo, "a")
    assert flow_main(["pace", "--", "a", "slow"]) == exits.OK
    inbox.consume(ctx.store, repo, "build")
    overrides = ctx.store.read_state()["overrides"]
    assert overrides["knobs"]["lane_pace"] == "slow"
    assert "pace changed to slow" in ctx.store.ledger.read_text()


def test_Cli_FlowPace_UnknownLevel_IsRefused(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    _lane(repo, "a")
    assert flow_main(["pace", "--", "a", "warp"]) == exits.USAGE
    assert "slowest, slow, normal, fast, fastest" in capsys.readouterr().out


@pytest.mark.parametrize("level", ["slowest", "fastest"])
def test_Cli_FlowPace_TakesEveryPaceTheSheetOffers(repo, monkeypatch, level):
    monkeypatch.chdir(repo)
    ctx = _lane(repo, "a")
    assert flow_main(["pace", "--", "a", level]) == exits.OK
    inbox.consume(ctx.store, repo, "build")
    assert ctx.store.read_state()["overrides"]["knobs"]["lane_pace"] == level


# -- the controls and the snapshot -------------------------------------------------------------------------------

ENGINE = host_actions.ENGINE


def test_PaceActions_BuildTheEngineAndFlowCommands():
    _, spawns = invoke("Q", ENGINE, {"level": "fast"})
    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["engine", "pace", "--", "fast"]
    _, spawns = invoke("q", flow_item("/work/repo", "t1"), {"level": "slow"})
    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["flow", "pace", "--", "t1", "slow"]
    assert {"Q", "q"} <= {action.key for action in actions.applicable(ENGINE)} | {"q"}
    assert "Q" in actions.LOCAL_ONLY_KEYS


def test_PaceActions_AnUnknownLevelIsTheReasonTheClientSees():
    spawns = Spawns(stdout="pace must be one of slowest, slow, normal, fast, fastest; got 'warp'\n", returncode=2)
    result, _ = invoke("Q", ENGINE, {"level": "warp"}, spawns=spawns)
    assert actions.refusal("Q", result).startswith("pace must be one of")


def test_Snapshot_Engine_ShowsThePaceItsLineAndTheLanes():
    document = api.snapshot_document(make_snapshot())
    assert {key: document["engine"][key] for key in ("pace", "target_pct", "lanes_min", "lanes")} == {
        "pace": "normal", "target_pct": 90, "lanes_min": 1, "lanes": 3}
    rules.set_pace("fastest", None)
    assert api.snapshot_document(make_snapshot())["engine"]["target_pct"] is None
    assert "Q" in {action["key"] for action in api.snapshot_document(make_snapshot())["engine"]["actions"]}


def test_Snapshot_TheOldDayPaceWarning_IsAProblem():
    (engine_home.root() / "config.toml").write_text("[knobs]\npace_day_pct = 10\n")
    problems = api.snapshot_document(make_snapshot())["problems"]
    assert any("pace_day_pct" in problem for problem in problems)


def test_Snapshot_ALaneItemShowsItsPace(repo):
    ctx = _lane(repo, "a", state={"status": "running"})
    overrides = {"knobs": {"lane_pace": "slow"}}
    ctx.store.update_state(lambda state: state.update(overrides=overrides))
    from test_cockpit_snapshot import make_item
    item = make_item(key=("flow", str(repo), "a"), repo=repo)
    document = api.snapshot_document(make_snapshot(items=[item]))
    assert document["items"][0]["pace"] == "slow"


def test_Snapshot_APaceTheLaneWasToldToTake_IsItsPaceAtOnceAndPendingUntilTheNextStep(repo, monkeypatch):
    monkeypatch.chdir(repo)
    ctx = _lane(repo, "a", state={"status": "running", "overrides": {"knobs": {"lane_pace": "normal"}}})
    from test_cockpit_snapshot import make_item
    item = make_item(key=("flow", str(repo), "a"), repo=repo)
    assert flow_main(["pace", "--", "a", "slow"]) == exits.OK

    told = api.snapshot_document(make_snapshot(items=[item]))["items"][0]
    inbox.consume(ctx.store, repo, "build")  # the next step starts and takes the command in
    applied = api.snapshot_document(make_snapshot(items=[item]))["items"][0]

    assert (told["pace"], told["pace_pending"]) == ("slow", True)
    assert (applied["pace"], applied["pace_pending"]) == ("slow", False)


def test_Snapshot_ALaneNobodyToldToChangePace_IsNotPending(repo):
    _lane(repo, "a", state={"status": "running"})
    from test_cockpit_snapshot import make_item
    item = make_item(key=("flow", str(repo), "a"), repo=repo)

    shown = api.snapshot_document(make_snapshot(items=[item]))["items"][0]

    assert shown["pace_pending"] is False


def test_Snapshot_ATellingToTakeThePaceItHasAlready_IsNotPending(repo, monkeypatch):
    monkeypatch.chdir(repo)
    _lane(repo, "a", state={"status": "running", "overrides": {"knobs": {"lane_pace": "slow"}}})
    from test_cockpit_snapshot import make_item
    item = make_item(key=("flow", str(repo), "a"), repo=repo)
    assert flow_main(["pace", "--", "a", "slow"]) == exits.OK

    shown = api.snapshot_document(make_snapshot(items=[item]))["items"][0]

    assert (shown["pace"], shown["pace_pending"]) == ("slow", False)


def test_Snapshot_ALaneWithNoPaceOfItsOwn_ShowsTheEnginesPace_TheOnePaceTheEngineUses(repo):
    ctx = _lane(repo, "a", state={"status": "running"})
    rules.set_pace("fastest", None)
    from test_cockpit_snapshot import make_item
    item = make_item(key=("flow", str(repo), "a"), repo=repo)
    shown = api.snapshot_document(make_snapshot(items=[item]))["items"][0]["pace"]

    assert shown == pace.lane_level(config.load(repo, overrides=ctx.store.read_state().get("overrides") or None)
                                    .values["knobs"]) == "fastest"


def test_ApiDoc_DescribesPace():
    from pathlib import Path
    doc = (Path(__file__).resolve().parents[1] / "docs" / "cockpit-api.md").read_text()
    for word in ("engine.pace", "engine.target_pct", "lane_pace", "`Q`", "`q`", "Slowest", "sakura engine pace"):
        assert word in doc, word
