"""Removing an account: a local only act with a confirmation, done by Leeway's book, refused while the engine still uses it."""

import pytest
from leeway_support import account, calls, make_helper
from sakura import actors, engine_home
from sakura.cockpit import actions, api
from sakura.cockpit.account_actions import AccountTarget
from sakura.cockpit.controller import (
    REMOTE_REFUSAL,
    Confirm,
    Controller,
    Refused,
    overlay_actor,
)
from sakura.cockpit.gate import Identity
from sakura.flow import pace_accounts, registry, rules
from sakura.usage import leeway
from test_cockpit_account_actions import account_row
from test_cockpit_actions import Spawns, invoke
from test_cockpit_snapshot import make_snapshot
from test_flow_pace_accounts import _second_repo

WORK = AccountTarget("claude", "work")
PHONE = Identity("owner@example.com", "nPHONE", "phone")


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


@pytest.fixture
def helper(tmp_path, monkeypatch):
    path = make_helper(tmp_path / "helper", rows=[account("main"), account("work")])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(path))
    return path


def _removes(helper):
    return [call for call in calls(helper) if call[:2] == ["accounts", "remove"]]


def test_RemoveAction_IsLocalOnly_AndNeedsAConfirmation():
    assert "X" in actions.LOCAL_ONLY_KEYS and actions.ACTIONS["X"].confirm


def test_Remove_AccountInLeewaysBook_RunsLeewayAccountsRemove(helper):
    result, _ = invoke("X", WORK)
    assert result.exit_code == 0
    assert _removes(helper) == [["accounts", "remove", "work", "--json"]]


def test_Remove_WithoutTheLeewayHelper_IsRefusedAndSaysWhy(monkeypatch):
    monkeypatch.setenv(leeway.ENV_VARIABLE, "/nonexistent/leeway")
    with pytest.raises(actions.ActionRefused, match="needs the Leeway helper"):
        invoke("X", WORK)


def test_Remove_AccountLeewayDoesNotHold_IsRefusedWithoutAnyRemoveCall(helper):
    with pytest.raises(actions.ActionRefused, match="not in Leeway's account book"):
        invoke("X", AccountTarget("claude", "ghost"))
    assert _removes(helper) == []


def test_Remove_AccountAPersonalRulePinsAStepTo_IsRefusedNamingTheStep(helper):
    rules.set("steps.build.account", "work", None, account_ids=["main", "work"])
    with pytest.raises(actions.ActionRefused, match="step build runs on it"):
        invoke("X", WORK)
    assert _removes(helper) == []


def test_Remove_AccountAProjectsOwnLayerPinsAStepTo_IsRefusedNamingTheProject(helper, repo):
    folder = engine_home.root(None) / engine_home.project_id(repo)
    folder.mkdir(parents=True)
    (folder / engine_home.CONFIG_NAME).write_text('[step_accounts]\nbuild = "work"\n')
    registry.remember(repo)
    with pytest.raises(actions.ActionRefused, match=f"step build of {repo.name}"):
        invoke("X", WORK)
    assert _removes(helper) == []


def test_Remove_AccountAProjectsPaceAllowListHolds_IsRefusedNamingTheProject(helper, repo):
    registry.remember(repo)
    pace_accounts.set_allowed(repo, ["work"], None)
    with pytest.raises(actions.ActionRefused, match=f"pace may move steps of {repo.name} to it"):
        invoke("X", WORK)
    assert _removes(helper) == []


def test_Remove_AllowListOfAProjectThatIsNoLongerRegistered_IsPrunedAfterTheRemoval(helper, tmp_path):
    gone = _second_repo(tmp_path)
    pace_accounts.set_allowed(gone, ["main", "work"], None)
    result, _ = invoke("X", WORK)
    assert result.exit_code == 0
    assert pace_accounts.allowed(gone, None) == ["main"]


def test_Remove_ALastAllowedAccountOfAnOrphanProject_LeavesNoEmptyEntry(helper, tmp_path):
    gone = _second_repo(tmp_path)
    pace_accounts.set_allowed(gone, ["work"], None)
    invoke("X", WORK)
    assert pace_accounts.everywhere(None) == {}


def test_Remove_LeewayFails_KeepsTheAllowListAsItWas(helper, tmp_path, monkeypatch):
    def refuse(*_):
        raise leeway.LeewayError("leeway did not take the change")
    monkeypatch.setattr(leeway, "account_action", refuse)
    gone = _second_repo(tmp_path)
    pace_accounts.set_allowed(gone, ["work"], None)
    result, _ = invoke("X", WORK)
    assert result.exit_code != 0
    assert pace_accounts.allowed(gone, None) == ["work"]


def test_Remove_FromThePhone_IsRefusedAndFromTheAppAsksForAConfirmation(helper, tmp_path):
    snapshot = make_snapshot(accounts=[account_row("claude", "work", key="work")])
    controller = Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")
    fields = {"key": "X", "target": api.target_id(WORK)}
    remote = controller.perform(fields, overlay_actor(PHONE), proof_field="confirm", proof_source="overlay")
    assert isinstance(remote, Refused) and remote.reason == REMOTE_REFUSAL
    local = controller.perform(fields, actors.Actor("app", "owner"), proof_field="confirm", proof_source="cli")  # the host face
    assert isinstance(local, Confirm)
    assert _removes(helper) == []


def test_detaching_goose_preserves_an_active_runs_binding(repo, account_home):
    from sakura.accounts import registry as accounts_registry
    from sakura.accounts import write
    from sakura.flow import run_records
    from sakura.flow.store import TaskStore

    folder = account_home / ".goose-work"
    folder.mkdir(mode=0o700)
    account = write.register_goose(folder, "Local models", "ollama")
    store = TaskStore(repo, "demo")
    store.create({"task": "demo", "status": "running", "workspaces": {"0": {"path": str(repo)}}})
    run_records.begin(store, "build-1", "builder", "build", "goose", "local-model", "",
                      "fixture", repo, {}, account=account.id, account_ref={"provider": "goose", "id": account.id},
                      stored_in=str(folder), session_id="goose-session")
    run_records.mark_running(store, "build-1", {"pid": 1234})
    before = store.read_state()["runs"]
    result, _ = invoke("X", AccountTarget("goose", account.id))
    assert result.exit_code == 0
    assert accounts_registry.load().accounts == ()
    assert store.read_state()["runs"] == before and folder.is_dir()
