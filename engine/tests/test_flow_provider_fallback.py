"""A role whose preset names a provider with no account runs on an account that exists, and the ledger says so."""

import shlex

import pytest

from sakura.flow import process, provider_fallback, rule_agents, runner
from sakura.usage import leeway
from flow_support import MonitorLauncher, make_context, task_workspace
from leeway_support import account, make_helper
from test_flow_runner import _codex_workspace

CLAUDE = rule_agents.AgentAccount("claude-default", "Claude", "claude")
CODEX = rule_agents.AgentAccount("codex-default", "Codex", "codex")


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


def test_Substitute_TheProviderHasAnAccount_NothingChanges():
    assert provider_fallback.substitute("claude", "builder", [CLAUDE, CODEX]) is None


def test_Substitute_NoAccountOfTheProvider_TakesTheFirstAccountThatExistsAndSaysSo():
    found = provider_fallback.substitute("claude", "builder", [CODEX])
    assert (found.provider, found.account.id) == ("codex", "codex-default")
    assert found.words == "No Claude account exists, so the builder runs on the Codex account (Codex)."


def test_Substitute_TheOtherWay_ACodexRoleWithNoCodexAccountRunsOnClaude():
    found = provider_fallback.substitute("codex", "plan_reviewer", [CLAUDE])
    assert (found.provider, found.account.id) == ("claude", "claude-default")
    assert found.words.startswith("No Codex account exists")


@pytest.mark.parametrize("accounts", [[], [rule_agents.AgentAccount("x", "X", "other")]])
def test_Substitute_NoAccountAtAll_StaysAsItIs(accounts):
    assert provider_fallback.substitute("claude", "builder", accounts) is None


@pytest.mark.parametrize("provider", ["exec", "none"])
def test_Substitute_AProgramThatIsNotAnAgent_StaysAsItIs(provider):
    assert provider_fallback.substitute(provider, "builder", [CODEX]) is None


def _only_codex(tmp_path, monkeypatch):
    row, book = account("codex-main", provider="codex", label="Codex")
    row["config_dir"] = str(tmp_path / "codex-home")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(row, book)])))


def _run(ctx, role, workspace):
    captured = {}

    def monitor(argv, **kwargs):
        captured["argv"], captured["env"] = argv, kwargs["env"]
        return process.ProcessOutcome(0, False, False, False, 1)

    request = runner.RunRequest(role, "intake", "go", {}, workspace)
    runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)
    return captured


def test_Run_ABuilderOfAClaudePresetWithOnlyACodexAccount_RunsOnCodexWithThatAccountsFolderAndTheLedgerSaysWhy(repo, tmp_path, monkeypatch):
    _only_codex(tmp_path, monkeypatch)
    ctx = make_context(repo, overrides={"roles": {"builder": "claude:sonnet"}})
    captured = _run(ctx, "builder", _codex_workspace(ctx))
    assert shlex.join(captured["argv"]).count("codex") and 'default_permissions="sakura-work"' in captured["argv"]
    assert captured["env"]["CODEX_HOME"] == str(tmp_path / "codex-home")
    assert "No Claude account exists, so the builder runs on the Codex account (Codex)." in ctx.store.ledger.read_text()


def test_Run_WithoutAccountsToCheck_ARoleKeepsItsProvider(repo, tmp_path):
    ctx = make_context(repo, overrides={"roles": {"builder": "claude:sonnet"}})
    captured = _run(ctx, "builder", task_workspace(ctx))
    assert captured["argv"][1] == "-p"
