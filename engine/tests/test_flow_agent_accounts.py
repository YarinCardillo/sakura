"""The accounts and models of a step, from Leeway's book, on a fresh engine home (click proof 040m, item 5): one account
added by path and renamed "Claude", no Codex, every step pinned to its id, no model list kept yet."""

import json
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.flow import config, model_catalog, rule_agents, rules, rules_cli
from sakura.usage import leeway
from leeway_support import account, make_helper

CLAUDE_ID = "claude-aa0b79cab9b0ea15"
RAW_TAIL = "aa0b79"
CLAUDE_LIST = json.loads((Path(__file__).parent / "fixtures" / "models" / "claude-initialize.json").read_text())
PINS = "\n".join(f'{step} = "{CLAUDE_ID}"' for step in ("plan", "plan_review", "build", "task_review", "final_review", "checks"))


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


@pytest.fixture
def book(tmp_path, monkeypatch):
    """Leeway with the one account, every step of the engine home pinned to it, and no models kept."""
    row, entry = account(CLAUDE_ID, label="Claude")
    row["config_dir"] = str(tmp_path / "claude-home")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(row, entry)])))
    (engine_home.root() / "config.toml").write_text(f"[step_accounts]\n{PINS}\n")
    return row


def _exchange(argv, env, requests, is_answer):
    return [{"type": "control_response", "response": {"response": {"models": CLAUDE_LIST["models"]}}}]


def _failing(argv, env, requests, is_answer):
    raise model_catalog.CatalogError("Claude Code did not list its models")


def _agents(described=None):
    described = described or rules.describe(None)
    return [rule["agent"] for node in described["nodes"] for rule in node["rules"] if "agent" in rule]


def _strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from _strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from _strings(item)


def test_Describe_TheAccountsAreLeewaysBookWithItsLabel_NotTheRegistrysDefault(book):
    described = rules.describe(None)
    assert [(item["id"], item["label"], item["provider"]) for item in described["agents"]] == [(CLAUDE_ID, "Claude", "claude")]


def test_Describe_EveryStepNamesItsAccountInWords_AndNoLabelOrNoteHoldsTheRawId(book):
    described = rules.describe(None)
    agents = _agents(described)
    assert agents and {agent["account_label"] for agent in agents} == {"Claude"}
    words = [text for agent in agents for key in ("account_label", "models_note") for text in [agent[key]]]
    words += [item["label"] for item in described["agents"]] + [item["label"] for agent in agents for item in agent["accounts"]]
    assert not [text for text in words if RAW_TAIL in text]


def test_Describe_AStepOnTheProjectsOwnAccount_HasNoAccountLabel(book):
    (engine_home.root() / "config.toml").write_text("")
    assert {agent["account_label"] for agent in _agents()} == {None}


def test_Describe_APinnedAccountLeewayDoesNotList_IsNamedInWordsNotById(book):
    (engine_home.root() / "config.toml").write_text('[step_accounts]\nbuild = "claude-0123456789abcdef"\n')
    build = next(rule["agent"] for node in rules.describe(None)["nodes"] for rule in node["rules"] if rule["id"] == "steps.build.model")
    assert build["account_label"] == "an account Leeway does not list" and "0123456789" not in build["account_label"]


def test_Ensure_FreshHome_StartsOneRefreshWithoutWaitingAndTheStepsSayTheModelsAreBeingRead(book):
    started = []
    accounts = rules.registered_accounts()
    model_catalog.ensure(accounts, start=lambda: started.append("refresh"))
    model_catalog.ensure(accounts, start=lambda: started.append("again"))
    assert started == ["refresh"]
    for agent in _agents():
        assert (agent["models_state"], agent["models_note"]) == ("refreshing", "Reading the models of Claude.")
    assert all(item["models_state"] == "refreshing" and item["models"] == [] for item in rules.describe(None)["agents"])


def test_Refresh_OfTheLeewayAccount_GivesTheStepItsModelListAndEfforts(book):
    model_catalog.ensure(rules.registered_accounts(), start=lambda: None)
    model_catalog.refresh(rules.registered_accounts()[0], exchange=_exchange)
    described = rules.describe(None)
    build = next(rule for node in described["nodes"] for rule in node["rules"] if rule["id"] == "steps.build.model")
    assert "claude:sonnet" in build["choices"] and build["agent"]["resolved"] == "claude-sonnet-5-5"
    assert build["agent"]["models_state"] == "kept" and build["agent"]["models_note"] == ""
    effort = next(rule for node in described["nodes"] for rule in node["rules"] if rule["id"] == "steps.build.effort")
    assert "high" in effort["choices"]
    assert described["agents"][0]["models"] and described["agents"][0]["models_state"] == "kept"


def test_Refresh_AListThatCannotBeRead_IsSaidInWordsAndNotAskedAgainAtOnce(book):
    started = []
    account_ = rules.registered_accounts()[0]
    model_catalog.ensure([account_], start=lambda: started.append(1))
    with pytest.raises(model_catalog.CatalogError):
        model_catalog.refresh(account_, exchange=_failing)
    build = next(agent for agent in _agents() if agent["account"] == CLAUDE_ID)
    assert build["models_state"] == "unavailable"
    assert build["models_note"] == "The models of Claude could not be read: Claude Code did not list its models."
    model_catalog.ensure([account_], start=lambda: started.append(2))
    assert started == [1]
    later = model_catalog.RETRY_SECONDS + 1
    model_catalog.ensure([account_], start=lambda: started.append(3), now=model_catalog.time.time() + later)
    assert started == [1, 3]


def test_Ensure_AccountsThatHaveTheirModels_StartNothing(book):
    model_catalog.refresh(rules.registered_accounts()[0], exchange=_exchange)
    started = []
    model_catalog.ensure(rules.registered_accounts(), start=lambda: started.append(1))
    assert started == []


def test_EngineRulesCommand_AsksForTheModelsOfAFreshHomeOnceAndAnswersAtOnce(book, monkeypatch, capsys):
    started = []
    monkeypatch.setattr(model_catalog, "start_refresh", lambda: started.append(1))
    assert rules_cli.main(["rules", "--json"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["agents"][0]["models_state"] == "refreshing" and started == [1]


def test_ModelsRefreshMissing_ReadsOnlyTheAccountsWithNoList(book, monkeypatch, capsys):
    kept = rules.registered_accounts()[0]
    model_catalog.refresh(kept, exchange=_exchange)
    asked = []
    monkeypatch.setattr(model_catalog, "refresh", lambda account_, **options: asked.append(account_.id))
    assert rules_cli.main(["models", "refresh", "--missing"]) == 0
    assert asked == []


def test_NoTestEverStartsARealProgramToReadModels_TheBackgroundReadIsStubbedByTheSuite(tmp_path, monkeypatch, capsys):
    # An account with no list makes `engine rules` start a read, which runs the real program on a login: no test may do that.
    import subprocess
    from sakura.accounts.registry import Account
    from sakura.flow import agent_accounts

    monkeypatch.setattr(agent_accounts, "usable", lambda: [Account("claude", "claude-x", "Claude", tmp_path / "x", True, None, (), ())])

    def refuse(*args, **kwargs):
        raise AssertionError(f"a test started a program: {args}")

    monkeypatch.setattr(subprocess, "Popen", refuse)
    assert rules_cli.main(["rules", "--json"]) == 0
