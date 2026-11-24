"""`sakura stack up|down`: the person's own command, run from the engine's read-only checkout, never by an agent."""

import io
import json
import os
from pathlib import Path

import pytest

from conftest import git
from sakura import cli, engine_home
from sakura.flow import cage, cage_server, config

CHECKOUT_MARKER = "stack-up"


def _commit(repo, name, text):
    (repo / name).write_text(text)
    git(repo, "add", name)
    git(repo, "commit", "-qm", f"add {name}")


def _checkout(repo):
    return engine_home.project_dir(repo) / "stack" / repo.name


def test_StackUp_HostCommand_RunsFromTheReadOnlyCheckoutOfTheCommittedHead(repo, monkeypatch, capsys, tmp_path):
    marker = tmp_path / "ran.txt"
    _commit(repo, "compose.sh", f"echo committed > {marker}\n")
    (repo / "compose.sh").write_text(f"echo uncommitted > {marker}\n")
    engine_home.config_file().write_text('[stack]\nup = "sh compose.sh"\n')
    monkeypatch.chdir(repo)
    assert cli.main(["stack", "up"]) == 0
    out = capsys.readouterr().out
    assert marker.read_text() == "committed\n" and "stack.up exited 0" in out
    assert "outside the cage" in out and git(repo, "rev-parse", "HEAD")[:12] in out
    checkout = _checkout(repo)
    assert git(checkout, "rev-parse", "HEAD") == git(repo, "rev-parse", "HEAD")
    assert not os.access(checkout / "compose.sh", os.W_OK), "the checkout's tracked files are read-only"


def test_StackDown_HostCommand_RunsLikeUp(repo, monkeypatch, capsys):
    engine_home.config_file().write_text(f'[stack]\nup = "true"\ndown = "touch {repo}/{CHECKOUT_MARKER}"\n')
    monkeypatch.chdir(repo)
    assert cli.main(["stack", "down"]) == 0 and (repo / CHECKOUT_MARKER).exists()


def test_StackUp_NoCommand_SaysSo(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    assert cli.main(["stack", "up"]) != 0
    assert "no stack.up command" in capsys.readouterr().out


def test_StackApprove_IsNoLongerACommand(repo, monkeypatch):
    engine_home.config_file().write_text('[stack]\nup = "true"\n')
    monkeypatch.chdir(repo)
    with pytest.raises(SystemExit) as raised:
        cli.main(["stack", "approve", "feedbeef"])
    assert raised.value.code == 2


@pytest.mark.parametrize("target", ["/etc", "../../outside", "sub/../../.."])
def test_StackUp_ARevisionWithASymlinkLeavingTheCheckout_IsRefused(repo, monkeypatch, capsys, target, tmp_path):
    marker = tmp_path / "ran.txt"
    (repo / "data").symlink_to(target)
    git(repo, "add", "data")
    git(repo, "commit", "-qm", "link out")
    engine_home.config_file().write_text(f'[stack]\nup = "touch {marker}"\n')
    monkeypatch.chdir(repo)
    assert cli.main(["stack", "up"]) != 0
    assert "data" in capsys.readouterr().out and not marker.exists()


def test_StackUp_ASymlinkInsideTheCheckout_IsKept(repo, monkeypatch, tmp_path):
    marker = tmp_path / "ran.txt"
    _commit(repo, "compose.sh", f"test -L link && touch {marker}\n")
    (repo / "link").symlink_to("compose.sh")
    git(repo, "add", "link")
    git(repo, "commit", "-qm", "link inside")
    engine_home.config_file().write_text('[stack]\nup = "sh compose.sh"\n')
    monkeypatch.chdir(repo)
    assert cli.main(["stack", "up"]) == 0 and marker.exists()


def test_Stack_InsideARoleOrACage_IsRefused(repo, monkeypatch, capsys):
    engine_home.config_file().write_text(f'[stack]\nup = "touch {repo}/{CHECKOUT_MARKER}"\n')
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["stack", "up"]) != 0
    monkeypatch.delenv("SAKURA_FLOW_ROLE")
    monkeypatch.setattr(engine_home, "sandboxed", lambda: True)
    assert cli.main(["stack", "up"]) != 0
    assert not (repo / CHECKOUT_MARKER).exists() and "agent cage" in capsys.readouterr().out


@pytest.mark.parametrize("layer, text", [("flow.toml", '[stack]\nup = "x"\n'), ("local.toml", '[flow.stack]\nup = "x"\n')])
def test_StackConfig_ARepositoryLayer_CannotSetTheCommands(repo, layer, text):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / layer).write_text(text)
    with pytest.raises(config.FlowConfigError, match="stack.up may only be set by the host"):
        config.load(repo)


def test_StackConfig_TheProjectLayerInTheEngineHome_SetsTheCommand(repo):
    path = engine_home.project_config_file(repo)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('[stack]\nup = "docker compose up -d"\n')
    loaded = config.load(repo)
    assert loaded.values["stack"]["up"] == "docker compose up -d" and loaded.origins["stack.up"] == "project"


# -- no agent path to the stack -------------------------------------------------------------------


def _prepared(repo):
    roots = (engine_home.root(), engine_home.account_root())
    site = cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "build-1", "builder", repo, repo / ".git",
                        roots, repo)
    return cage.prepare(site, cage.Policy("default"), 2, Path("/home/someone"))


def test_StackTool_TheCageServer_OffersNoStackTool(repo, tmp_path):
    run_cage = _prepared(repo)
    policy = json.loads(run_cage.policy.read_text())
    replies = []
    out = type("Out", (), {"write": lambda self, text: replies.append(json.loads(text)), "flush": lambda self: None})()
    messages = [{"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "stack",
                                                                                "arguments": {"action": "up"}}}]
    cage_server.serve(policy, io.StringIO("".join(json.dumps(message) + "\n" for message in messages)), out)
    assert "stack" not in {tool["name"] for tool in replies[0]["result"]["tools"]}
    assert replies[1]["error"]["message"] == "unknown tool"  # a fixed sentence: the client's tool name is not echoed
    assert "stack" not in policy and not hasattr(cage_server, "run_stack")


def test_StackTool_ClaudeSettings_AllowNoStackTool(repo):
    settings = json.loads(_prepared(repo).settings.read_text())
    assert not any("stack" in rule for rule in settings["permissions"]["allow"])


def test_StackPolicy_ACagePolicyNamingTheStack_IsRefused():
    folder = engine_home.root() / cage.POLICIES_DIR
    folder.mkdir()
    (folder / "old.toml").write_text('stack = "ask"\n')
    with pytest.raises(cage.CageError, match="sakura stack up"):
        cage.load_policy("old")
