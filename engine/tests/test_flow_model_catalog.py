"""The models of an account's program come from the program, are kept with their date, and never block a lane."""

import json
import stat
import sys
from pathlib import Path

import pytest
from sakura.accounts.registry import Account
from sakura.flow import model_catalog

FIXTURES = Path(__file__).parent / "fixtures" / "models"
CODEX_LIST = json.loads((FIXTURES / "codex-model-list.json").read_text())
CLAUDE_INITIALIZE = json.loads((FIXTURES / "claude-initialize.json").read_text())

CODEX_PROGRAM = '''#!{python}
import json, sys
result = json.loads(open({listing!r}).read())
for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        print(json.dumps({{"id": message["id"], "result": {{"userAgent": "fake"}}}}), flush=True)
    elif message.get("method") == "model/list":
        print(json.dumps({{"id": message["id"], "result": result}}), flush=True)
'''
CLAUDE_PROGRAM = '''#!{python}
import json, sys
answer = json.loads(open({listing!r}).read())
for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "control_request" and message["request"]["subtype"] == "initialize":
        print(json.dumps({{"type": "control_response", "response": {{"subtype": "success", "request_id": message["request_id"],
                          "response": {{"models": answer["models"], "commands": []}}}}}}), flush=True)
'''


def _program(tmp_path, template, listing, name):
    script = tmp_path / name
    script.write_text(template.format(python=sys.executable, listing=str(listing)))
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return str(script)


def _account(provider, identifier=None, tmp_path=None):
    identifier = identifier or provider
    return Account(provider, identifier, identifier.title(), (tmp_path or Path("/nonexistent")) / f".{provider}-{identifier}",
                   True, None, (), ())


def test_Refresh_Codex_TakesTheModelsEffortsAndDefaultFromItsAppServer(tmp_path):
    account = _account("codex", tmp_path=tmp_path)
    program = _program(tmp_path, CODEX_PROGRAM, FIXTURES / "codex-model-list.json", "codex")

    catalog = model_catalog.refresh(account, root=tmp_path / "home", program=program)

    ids = [model.id for model in catalog.models]
    assert ids[:3] == ["gpt-6.2-sol", "gpt-6.2-astra", "gpt-6.1-terra"] and "gpt-6-luna" in ids
    sol = catalog.find("gpt-6.2-sol")
    assert sol.label == "GPT-6.2-Sol" and sol.efforts == ("low", "medium", "high", "xhigh", "max", "ultra")
    assert sol.default_effort == "low" and sol.is_default and catalog.source == "Codex app-server model/list"
    assert catalog.fetched_at.endswith("+00:00")


def test_Refresh_Claude_TakesTheAliasesTheirFullModelsAndEffortsFromClaudeCode(tmp_path):
    account = _account("claude", "claude-default", tmp_path)
    program = _program(tmp_path, CLAUDE_PROGRAM, FIXTURES / "claude-initialize.json", "claude")

    catalog = model_catalog.refresh(account, root=tmp_path / "home", program=program)

    assert [model.id for model in catalog.models] == ["opus", "fable", "sonnet", "haiku"], "`default` is not a model"
    sonnet = catalog.find("sonnet")
    assert sonnet.resolved == "claude-sonnet-5-5" and catalog.find("claude-sonnet-5-5") is sonnet
    assert sonnet.efforts == ("low", "medium", "high", "xhigh", "max") and sonnet.label == "Sonnet"


def test_Read_AKeptList_IsReadBackWithItsDateAndStartsNoProgram(tmp_path):
    account = _account("codex", tmp_path=tmp_path)
    program = _program(tmp_path, CODEX_PROGRAM, FIXTURES / "codex-model-list.json", "codex")
    written = model_catalog.refresh(account, root=tmp_path / "home", program=program)

    found = model_catalog.read(account, tmp_path / "home")

    assert found == written
    assert model_catalog.path(account, tmp_path / "home").stat().st_mode & 0o777 == 0o600


def test_Read_NothingKept_IsNoneNotAnError(tmp_path):
    assert model_catalog.read(_account("codex", tmp_path=tmp_path), tmp_path / "home") is None


def test_Refresh_AProgramThatCannotBeStarted_RaisesWithWordsAndKeepsTheOldList(tmp_path):
    account = _account("codex", tmp_path=tmp_path)
    program = _program(tmp_path, CODEX_PROGRAM, FIXTURES / "codex-model-list.json", "codex")
    kept = model_catalog.refresh(account, root=tmp_path / "home", program=program)

    with pytest.raises(model_catalog.CatalogError, match="could not be started"):
        model_catalog.refresh(account, root=tmp_path / "home", program=str(tmp_path / "missing"))

    assert model_catalog.read(account, tmp_path / "home") == kept


def test_Refresh_AProgramThatListsNothing_RaisesAndKeepsNothingNew(tmp_path):
    account = _account("claude", "claude-default", tmp_path)
    empty = tmp_path / "empty.json"
    empty.write_text(json.dumps({"models": []}))
    program = _program(tmp_path, CLAUDE_PROGRAM, empty, "claude")

    with pytest.raises(model_catalog.CatalogError, match="did not list its models"):
        model_catalog.refresh(account, root=tmp_path / "home", program=program)

    assert model_catalog.read(account, tmp_path / "home") is None


def test_RefreshAll_OneFailure_IsReportedAndTheOthersAreKept(tmp_path):
    good, bad = _account("codex", tmp_path=tmp_path), _account("claude", "claude-default", tmp_path)

    def exchange(argv, env, requests, is_answer):
        if argv[1] == "app-server":
            return [{"id": 2, "result": CODEX_LIST}]
        raise model_catalog.CatalogError("the program ended without answering")

    answers = model_catalog.refresh_all([good, bad], root=tmp_path / "home", exchange=exchange)

    assert answers == {"codex": "ok", "claude-default": "the program ended without answering"}
    assert model_catalog.read(good, tmp_path / "home") is not None


def test_Refresh_Environment_NamesTheAccountsFolderExceptTheDefaultOne(tmp_path, monkeypatch):
    seen = []

    def exchange(argv, env, requests, is_answer):
        seen.append(dict(env))
        return [{"id": 2, "result": CODEX_LIST}]

    model_catalog.refresh(_account("codex", "work", tmp_path), root=tmp_path / "home", exchange=exchange)
    default = Account("codex", "codex", "Codex", Path("~/.codex").expanduser(), True, None, (), ())
    model_catalog.refresh(default, root=tmp_path / "home", exchange=exchange)

    assert seen[0]["CODEX_HOME"] == str(tmp_path / ".codex-work") and "CODEX_HOME" not in seen[1]


# -- `sakura engine models`

def _registry(monkeypatch, tmp_path, *accounts):
    from sakura.accounts import registry

    monkeypatch.setattr(registry, "load_subscriptions", lambda *a, **k: registry.Registry(tuple(accounts), False))


def test_Cli_EngineModels_ShowsWhatWasKeptPerAccountWithItsDate_AndNothingWhereNothingWas(tmp_path, monkeypatch, capsys):
    from sakura.flow import rules_cli

    codex, claude = _account("codex", tmp_path=tmp_path), _account("claude", "claude-default", tmp_path)
    _registry(monkeypatch, tmp_path, codex, claude)
    model_catalog.refresh(codex, exchange=lambda *a: [{"id": 2, "result": CODEX_LIST}])

    assert rules_cli.main(["models", "--json"]) == 0

    listed = {item["id"]: item for item in json.loads(capsys.readouterr().out)["accounts"]}
    assert listed["codex"]["models"][0]["id"] == "gpt-6.2-sol" and listed["codex"]["fetched_at"]
    assert listed["claude-default"]["models"] == [] and listed["claude-default"]["fetched_at"] == ""
    assert (listed["codex"]["label"], listed["codex"]["provider"]) == ("Codex", "codex")


def test_Cli_EngineModelsRefresh_AsksEveryProgramAndSaysWhatFailed(tmp_path, monkeypatch, capsys):
    from sakura.flow import rules_cli

    codex, claude = _account("codex", tmp_path=tmp_path), _account("claude", "claude-default", tmp_path)
    _registry(monkeypatch, tmp_path, codex, claude)

    def fetch(account, *, root=None, program=None, exchange=None):
        if account.provider == "claude":
            raise model_catalog.CatalogError("Claude Code did not list its models")
        return model_catalog.Catalog("codex", account.id, "2026-11-20T12:00:00+00:00", "test", ())

    monkeypatch.setattr(model_catalog, "refresh", fetch)

    assert rules_cli.main(["models", "refresh"]) == 0

    out = capsys.readouterr().out
    assert "Codex: ok" in out and "Claude-Default: Claude Code did not list its models" in out


def test_Cli_EngineModelsRefresh_InsideARole_IsRefused(monkeypatch, capsys):
    from sakura.flow import rules_cli

    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")

    assert rules_cli.main(["models", "refresh"]) != 0


def test_Action_RefreshModels_RunsTheEngineCommandAndIsNeverFromARemoteDevice():
    from sakura.cockpit import actions, host_actions
    from test_cockpit_actions import invoke

    _, spawns = invoke("M", host_actions.ENGINE, {})

    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["engine", "models", "--", "refresh"]
    assert "M" in actions.LOCAL_ONLY_KEYS


def test_refresh_models_action_selects_only_one_provider_scoped_account():
    from sakura.cockpit import host_actions
    from test_cockpit_actions import invoke

    _, spawns = invoke("M", host_actions.ENGINE, {"provider": "goose", "account": "local"})
    argv = [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]
    assert argv[1:] == ["engine", "models", "--provider=goose", "--account=local", "--", "refresh"]
