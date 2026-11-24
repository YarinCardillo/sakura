"""Adding an account from the app: the new login folder and its command (`g`), and the account book entry (`j`)."""

import json
import os
import shlex
from pathlib import Path

import pytest
from leeway_support import account, calls, make_helper
from sakura.cockpit import actions, host_actions
from sakura.usage import leeway
from test_cockpit_actions import invoke

ENGINE = host_actions.ENGINE


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


def _book(tmp_path, monkeypatch, *extra):
    """A Leeway with the default Claude account, and the helper's path."""
    row, book = account("claude-default", label="Claude")
    helper = make_helper(tmp_path / "helper", rows=[(row, book), *extra])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    return helper


def _login_folder(account_home, name=".claude-2", mode=0o700):
    folder = account_home / name
    folder.mkdir(mode=mode)
    folder.chmod(mode)
    return folder


def _add(folder, provider="claude", label="Claude Work"):
    return invoke("j", ENGINE, {"folder": str(folder), "provider": provider, "label": label})[0]


def _add_calls(helper):
    return [call for call in calls(helper) if call[:2] == ["accounts", "add"]]


# The new login folder

def test_NewLoginFolder_Claude_IsAnEmptyPrivateFolderWithTheLoginCommand(account_home):
    result, spawns = invoke("g", ENGINE, {"provider": "claude"})
    document = json.loads(result.output)
    folder = account_home / ".claude-2"
    assert result.exit_code == 0 and document["folder"] == str(folder)
    assert folder.is_dir() and list(folder.iterdir()) == [] and (folder.stat().st_mode & 0o777) == 0o700
    assert document["argv"] == ["claude", "auth", "login"] and document["env"] == {"CLAUDE_CONFIG_DIR": str(folder)}
    assert shlex.split(document["command"]) == ["env", f"CLAUDE_CONFIG_DIR={folder}", "claude", "auth", "login"]
    assert spawns.calls == []  # the engine never runs the login


def test_NewLoginFolder_Codex_NamesCodexHome(account_home):
    document = json.loads(invoke("g", ENGINE, {"provider": "codex"})[0].output)
    assert document["folder"] == str(account_home / ".codex-2")
    assert document["env"] == {"CODEX_HOME": document["folder"]} and document["argv"] == ["codex", "login"]


def test_NewLoginFolder_AskedTwice_MakesTwoFolders(account_home):
    first = json.loads(invoke("g", ENGINE, {"provider": "claude"})[0].output)["folder"]
    second = json.loads(invoke("g", ENGINE, {"provider": "claude"})[0].output)["folder"]
    assert (Path(first).name, Path(second).name) == (".claude-2", ".claude-3")


def test_NewLoginFolder_TheCageDeniesIt(account_home):
    from sakura.accounts import folders
    folder = Path(json.loads(invoke("g", ENGINE, {"provider": "codex"})[0].output)["folder"])
    assert Path(os.path.realpath(folder)) in folders.login_folders(account_home)


@pytest.mark.parametrize("provider", ["gemini", "", "claude;ls"])
def test_NewLoginFolder_UnknownProvider_IsRefusedAndNothingIsMade(account_home, provider):
    with pytest.raises(actions.ActionRefused, match="invalid provider"):
        invoke("g", ENGINE, {"provider": provider})
    assert not [entry for entry in account_home.iterdir() if entry.name.startswith((".claude", ".codex"))]


# Adding the account

def test_AddAccount_AddsTheFolderThenNamesItWhatThePersonTyped(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    folder = _login_folder(account_home)
    result = _add(folder, label="Claude Work")
    assert result.exit_code == 0
    assert json.loads(result.output) == {"id": "added-2", "provider": "claude", "label": "Claude Work"}
    made = [call for call in calls(helper) if call[:1] == ["accounts"]]
    assert made == [["accounts", "list", "--json"], ["accounts", "add", str(folder), "--provider", "claude", "--json"],
                    ["accounts", "rename", "added-2", "Claude Work", "--json"]]


def test_AddAccount_Codex_PassesTheProvider(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    folder = _login_folder(account_home, ".codex-2")
    assert _add(folder, "codex", "Codex Two").exit_code == 0
    assert _add_calls(helper) == [["accounts", "add", str(folder), "--provider", "codex", "--json"]]


@pytest.mark.parametrize("make", ["elsewhere", "no-claude-name", "nested"])
def test_AddAccount_FolderNotADotLoginFolderOfTheHome_IsRefusedAndLeewayIsNotAsked(account_home, tmp_path, monkeypatch, make):
    helper = _book(tmp_path, monkeypatch)
    folder = {"elsewhere": tmp_path / ".claude-9", "no-claude-name": account_home / ".work",
              "nested": account_home / "Documents" / ".claude-2"}[make]
    folder.mkdir(parents=True)
    result = _add(folder)
    assert result.exit_code == 2 and "dot folder of your home" in result.output
    assert _add_calls(helper) == []


def test_AddAccount_ALinkToAFolderElsewhere_IsRefused(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir(mode=0o700)
    link = account_home / ".claude-2"
    link.symlink_to(elsewhere)
    assert _add(link).exit_code == 2 and _add_calls(helper) == []


def test_AddAccount_FolderOthersCanWrite_IsRefused(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    folder = _login_folder(account_home, mode=0o770)
    result = _add(folder)
    assert result.exit_code == 2 and "others can write" in result.output and _add_calls(helper) == []


@pytest.mark.parametrize("path", ["relative/.claude-2", "~/.claude-2"])
def test_AddAccount_PathNotAbsolute_IsRefused(account_home, tmp_path, monkeypatch, path):
    _book(tmp_path, monkeypatch)
    assert _add(path).exit_code == 2


def test_AddAccount_FolderMissing_IsRefused(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    assert _add(account_home / ".claude-2").exit_code == 2 and _add_calls(helper) == []


def test_AddAccount_AFolderAlreadyInTheBook_IsSaidSo(account_home, tmp_path, monkeypatch):
    folder = _login_folder(account_home)
    row, book = account("work", label="Work")
    row["config_dir"] = str(folder)
    helper = _book(tmp_path, monkeypatch, (row, book))
    result = _add(folder)
    assert result.exit_code == 2 and "already" in result.output
    assert not [call for call in calls(helper) if call[:2] == ["accounts", "rename"]]


def test_AddAccount_WithoutLeeway_SaysItNeedsTheHelper(account_home, monkeypatch):
    monkeypatch.delenv(leeway.ENV_VARIABLE, raising=False)
    result = _add(_login_folder(account_home))
    assert result.exit_code == 2 and "Leeway helper" in result.output


def test_AddAccount_ANameStartingWithADash_IsRefusedBeforeTheHelperIsAsked(account_home, tmp_path, monkeypatch):
    helper = _book(tmp_path, monkeypatch)
    result = _add(_login_folder(account_home), label="-rf")
    assert result.exit_code != 0 and _add_calls(helper) == []


@pytest.mark.parametrize("inputs", [{"folder": "/x", "provider": "claude"}, {"folder": "/x", "provider": "gemini", "label": "A"},
                                    {"folder": "/x", "provider": "claude", "label": "a\nb"}])
def test_AddAccount_BadInputs_AreRefusedBeforeAnythingRuns(inputs):
    with pytest.raises(actions.ActionRefused):
        invoke("j", ENGINE, inputs)


def test_AccountActions_AreLocalOnlyAndBelongToTheEngineTarget():
    assert {"j", "g"} <= actions.LOCAL_ONLY_KEYS
    assert {"j", "g"} <= {action.key for action in actions.applicable(ENGINE)}


def test_goose_rejects_duplicate_keychain_slot_across_roots(account_home):
    from sakura.accounts import registry, write

    first = _login_folder(account_home, ".goose-one")
    second = _login_folder(account_home, ".goose-two")
    write.register_goose(first, "First", "openai")
    with pytest.raises(registry.AccountsConfigError, match="Keychain slot"):
        write.register_goose(second, "Second", "openai")
    assert len(registry.load().accounts) == 1


@pytest.mark.parametrize("streams_reply", [True, False])
def test_keyless_local_goose_is_ready_after_explicit_check(account_home, tmp_path, monkeypatch, streams_reply):
    import hashlib
    import sys

    from sakura import engine_home, programs
    from sakura.accounts import goose, write
    from sakura.cockpit.account_actions import AccountTarget

    folder = _login_folder(account_home, ".goose-work")
    (folder / "config").mkdir(mode=0o700)
    (folder / "config" / "config.yaml").touch()
    account = write.register_goose(folder, "Local models", "ollama")
    adapter = tmp_path / "fixture-adapter"
    adapter.write_text(f"#!{sys.executable}\nstreams_reply = {streams_reply!r}\n" + """
import json
import os
import sys

assert sys.argv[1:] == ["acp"]
assert "OPENAI_API_KEY" not in os.environ
assert os.environ["GOOSE_PROVIDER"] == "ollama"
assert os.environ["OLLAMA_HOST"] == "http://127.0.0.1:11434"
for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        result = {"protocolVersion": 1, "agentInfo": {"name": "fixture", "version": "1.60.2"}}
    elif method == "session/new":
        assert request["params"]["mcpServers"] == []
        result = {"sessionId": "fixture-connection"}
    else:
        assert method == "session/prompt"
        assert request["params"]["sessionId"] == "fixture-connection"
        if streams_reply:
            print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "fixture-connection", "update": {
                    "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "OK"}
                }
            }}), flush=True)
        result = {"stopReason": "end_turn"}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
""")
    adapter.chmod(0o700)
    digest = hashlib.sha256(adapter.read_bytes()).hexdigest()
    (engine_home.root() / programs.HOST_FILE).write_text(f'goose = {json.dumps(str(adapter.resolve()))}\n'
        f'[acp.goose]\nagent_name = "fixture"\nsetup_safe = true\n'
        f'adapter = {{path = {json.dumps(str(adapter.resolve()))}, version = "1.60.2", sha256 = "{digest}"}}\n')
    monkeypatch.setenv("OPENAI_API_KEY", "test-placeholder")
    target = AccountTarget("goose", account.id)
    assert actions.is_local_only("accountcheck", target.key)
    result, _ = invoke("accountcheck", target)
    assert result.exit_code == 0
    expected = ({"readiness": "ready", "reason": None} if streams_reply else
                {"readiness": "needs_setup", "reason": "connection_failed"})
    assert json.loads(result.output) == goose.readiness(account) == expected
    (folder / "config" / "config.yaml").write_text("provider: ollama")
    assert goose.readiness(account) == {"readiness": "unknown", "reason": "configuration_changed"}


def test_goose_configuration_refuses_a_linked_config_file(account_home, tmp_path, monkeypatch, capsys):
    from sakura import cli, engine_home, programs
    from test_acp_qualification import install

    folder = _login_folder(account_home, ".goose-work")
    (folder / "config").mkdir(mode=0o700)
    outside = tmp_path / "outside.yaml"
    outside.touch()
    (folder / "config" / "config.yaml").symlink_to(outside)
    adapter = install(tmp_path / "fixture-adapter")
    manifest = engine_home.root() / programs.HOST_FILE
    manifest.write_text(f'goose = {json.dumps(str(adapter.resolve()))}\n'
                        + manifest.read_text().replace('[acp.goose]\n', '[acp.goose]\nsetup_safe = true\n'))
    monkeypatch.setattr(os, "execve", lambda *_: pytest.fail("started against a linked config"))
    assert cli.main(["setup", "configure-goose", f"--folder={folder}"]) == 2
    assert capsys.readouterr().err.strip() == "configuration_changed"
