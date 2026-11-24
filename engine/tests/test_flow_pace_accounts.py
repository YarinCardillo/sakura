"""Pace moves a step only to an account the person allowed for that project; with no allow-list it waits instead."""

import pytest

from sakura import actors, engine_home
from sakura.budget import guard
from sakura.cockpit import actions, api, host_actions
from sakura.cockpit.controller import REMOTE_REFUSAL, Controller, Refused, overlay_actor
from sakura.cockpit.gate import Identity
from sakura.cockpit.project_actions import ProjectTarget
from sakura.flow import budget_guard, gitutil, pace, pace_accounts, pace_check, registry, rules, rules_cli
from sakura.flow.exits import OK, USAGE

from conftest import git
from flow_support import make_context
from sakura.usage import leeway
from leeway_support import account, make_helper
from test_cockpit_actions import Spawns, invoke
from test_cockpit_snapshot import make_item, make_snapshot
from test_flow_pace import FakeAdvisor, decide

PHONE = Identity("owner@example.com", "nPHONE", "phone")
TWO_ACCOUNTS = {"personal": 95, "work": 40}


def _second_repo(tmp_path):
    path = tmp_path / "other"
    git(path.parent, "init", "-q", "-b", "main", str(path))
    gitutil.trust_repository(path)
    return path


def _another_lane_runs(repo):
    """Pace holds a lane back only beyond `lanes_min` (1 by default): one other lane already runs."""
    registry.remember(repo)
    make_context(repo, task="busy", state={"status": "running"})


def _evaluate(ctx, advisor):
    return budget_guard.evaluate(ctx.store, ctx.config, None, role="builder", advisor=advisor)


# -- the decision ------------------------------------------------------------------------------------------------

def test_Decide_AccountUnderTheLineButNotAllowed_Waits():
    decision = pace.decide(pace.PROJECT_ACCOUNT, "normal", "normal", FakeAdvisor(TWO_ACCOUNTS), allowed=())
    assert decision.kind == pace.WAIT and decision.account == "work"


def test_Decide_AccountUnderTheLineAndAllowed_Switches():
    decision = pace.decide(pace.PROJECT_ACCOUNT, "normal", "normal", FakeAdvisor(TWO_ACCOUNTS), allowed=("work",))
    assert (decision.kind, decision.account) == (pace.SWITCH, "work")


def test_Decide_AnotherAccountAllowed_StillWaitsForTheOneUnderTheLine():
    decision = pace.decide(pace.PROJECT_ACCOUNT, "normal", "normal", FakeAdvisor(TWO_ACCOUNTS), allowed=("other",))
    assert decision.kind == pace.WAIT


def test_Decide_PinnedStep_NeverMovesWhateverIsAllowed():
    advisor = FakeAdvisor(TWO_ACCOUNTS, reserve={"personal": "work"})
    decision = pace.decide("personal", "normal", "normal", advisor, allowed=("work",))
    assert decision.kind == pace.WAIT and decision.account == "personal"


# -- the guard ---------------------------------------------------------------------------------------------------

def test_Guard_UnpinnedStepAndNoAllowList_DefersAndKeepsItsAccount(repo):
    _another_lane_runs(repo)
    ctx = make_context(repo, task="a")
    decision = _evaluate(ctx, FakeAdvisor(TWO_ACCOUNTS))
    assert (decision.action, decision.rule) == (guard.DEFER, pace_check.PACE_RULE)
    assert "pace_account" not in (ctx.store.read_state().get("budget") or {})


def test_Guard_UnpinnedStepAndTheAccountAllowedForTheProject_Moves(repo):
    _another_lane_runs(repo)
    pace_accounts.set_allowed(repo, ["work"], None)
    ctx = make_context(repo, task="a")
    assert _evaluate(ctx, FakeAdvisor(TWO_ACCOUNTS)).action == guard.RUN
    assert ctx.store.read_state()["budget"]["pace_account"] == {"build": "work"}


def test_Guard_AnAccountAllowedForAnotherProject_DoesNotMoveThisOne(repo, tmp_path):
    _another_lane_runs(repo)
    pace_accounts.set_allowed(_second_repo(tmp_path), ["work"], None)
    assert _evaluate(make_context(repo, task="a"), FakeAdvisor(TWO_ACCOUNTS)).action == guard.DEFER


def test_Guard_PinnedStep_NeverMovesEvenWithTheAccountAllowed(repo, tmp_path, monkeypatch):
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "h", rows=[account("personal"), account("work")])))
    _another_lane_runs(repo)
    pace_accounts.set_allowed(repo, ["work"], None)
    ctx = make_context(repo, task="a", overrides={"step_accounts": {"build": "personal"}})
    advisor = FakeAdvisor(TWO_ACCOUNTS, reserve={"personal": "work"})
    assert _evaluate(ctx, advisor).action == guard.DEFER
    assert "pace_account" not in (ctx.store.read_state().get("budget") or {})


def test_Guard_AMoveToAnAccountNoLongerAllowed_IsDroppedBeforeTheStepRuns(repo):
    _another_lane_runs(repo)
    state = {"budget": {"pace_account": {"build": "work"}}}
    ctx = make_context(repo, task="a", state=state)
    _evaluate(ctx, FakeAdvisor({"personal": 10, "work": 10}))
    assert ctx.store.read_state()["budget"]["pace_account"] == {}


# -- the allow-list ----------------------------------------------------------------------------------------------

def test_PaceAccounts_AreKeptPerProjectInTheEngineHome(repo, tmp_path):
    gitutil.trust_repository(repo)
    other = _second_repo(tmp_path)
    pace_accounts.set_allowed(repo, ["work", "extra"], None)
    pace_accounts.set_allowed(other, ["solo"], None)
    assert pace_accounts.allowed(repo, None) == ["work", "extra"]
    assert pace_accounts.allowed(other, None) == ["solo"]
    pace_accounts.set_allowed(repo, [], None)
    assert pace_accounts.allowed(repo, None) == [] and pace_accounts.allowed(other, None) == ["solo"]


def test_PaceAccounts_NothingKept_NobodyIsAllowed(repo):
    assert pace_accounts.allowed(repo, None) == []


def test_PaceAccounts_ASetThatLooksLikeAnOption_IsRefused(repo):
    with pytest.raises(pace_accounts.AccountsRefused, match="account"):
        pace_accounts.set_allowed(repo, ["--all"], None)
    assert pace_accounts.allowed(repo, None) == []


def test_Cli_EnginePaceAccounts_KeepsTheListForTheProjectOfTheWorkingFolder(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    assert rules_cli.main(["pace-accounts", "--actor=cli:me", "--", "work,extra"]) == OK
    assert pace_accounts.allowed(repo, None) == ["work", "extra"]
    assert rules_cli.main(["pace-accounts", "--", ""]) == OK
    assert pace_accounts.allowed(repo, None) == []
    assert rules_cli.main(["pace-accounts", "--", "--all"]) == USAGE
    assert "account" in capsys.readouterr().out


# -- the act and the snapshot -----------------------------------------------------------------------------------

def test_PaceAccountsAction_BuildsTheEngineCommandInTheProject(repo):
    _, spawns = invoke("W", ProjectTarget(repo), {"accounts": "work,extra"})
    (argv, kwargs), = spawns.calls
    assert [part for part in argv if not part.startswith("--actor=")][1:] == ["engine", "pace-accounts", "--", "work,extra"]
    assert kwargs["cwd"] == repo


def test_PaceAccountsAction_IsLocalOnly_AndTheOverlayIsRefused(repo, tmp_path):
    assert "W" in actions.LOCAL_ONLY_KEYS
    snapshot = make_snapshot(projects=[repo])
    controller = Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")
    fields = {"key": "W", "target": api.target_id(ProjectTarget(repo)), "input.accounts": "work"}
    remote = controller.perform(fields, overlay_actor(PHONE), proof_field="confirm", proof_source="overlay")
    assert isinstance(remote, Refused) and remote.reason == REMOTE_REFUSAL
    local = controller.perform(fields, actors.Actor("app", "owner"), proof_field="confirm", proof_source="cli")  # the host face
    assert not isinstance(local, Refused) and local.result.exit_code == 0


def test_PaceAccountsAction_AnInputThatIsNotAListOfAccounts_IsRefused(repo):
    with pytest.raises(actions.ActionRefused, match="invalid accounts"):
        invoke("W", ProjectTarget(repo), {"accounts": "work; rm -rf"})


def test_Snapshot_ShowsPaceAllowedPerAccountForTheProjectTheLaneAndTheRules(repo, monkeypatch):
    registry.remember(repo)
    make_context(repo, task="a", state={"status": "running"})
    pace_accounts.set_allowed(repo, ["work"], None)
    monkeypatch.setattr(api.leeway_view, "read", lambda describe: api.leeway_view.LeewayView(
        accounts=[{"id": "personal", "label": "Personal", "provider": "claude", "plans": []},
                  {"id": "work", "label": "Work", "provider": "claude", "plans": []}], usage={"providers": []}))
    document = api.snapshot_document(make_snapshot(items=[make_item(key=("flow", str(repo), "a"), repo=repo)],
                                                   projects=[repo]))
    expected = [{"id": "personal", "label": "Personal", "pace_allowed": False},
                {"id": "work", "label": "Work", "pace_allowed": True}]
    assert document["items"][0]["pace_accounts"] == expected
    assert document["projects"][0]["pace_accounts"] == expected
    assert document["engine"]["pace_accounts"] == {engine_home.project_id(repo): ["work"]}
    assert rules.describe(None)["pace_accounts"] == {engine_home.project_id(repo): ["work"]}
