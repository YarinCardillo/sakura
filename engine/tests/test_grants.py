"""Grants per class: which engine tools and host MCP servers each class of agent may use."""

import io
import json
import os
from pathlib import Path

import pytest

from sakura import engine_home, git_program, grants, grants_exec, repository
from sakura.flow import cage, cage_server, config

HOME = Path("/home/someone")
HOST_COMMAND = "/bin/sh"
GITHUB = f'[github]\ncommand = "{HOST_COMMAND}"\narguments = ["stdio"]\nenvironment = ["GITHUB_TOKEN"]\n'


def _host(grants_text="", servers_text=""):
    root = engine_home.root()
    (root / grants.GRANTS_FILE).write_text(grants_text)
    (root / grants.SERVERS_FILE).write_text(servers_text)
    return root


def _load(writable=None):
    """The grants as the cage loads them: every folder a role can write is forbidden for a server's files."""
    return grants.load(engine_home.root(), cage.writable_roots() if writable is None else writable)


def _prepared(repo, role="builder"):
    root = engine_home.root()
    site = cage.RunSite(root / "proj" / "flow" / "demo", "run-1", role, repo, repository.common_dir(repo),
                        (root, engine_home.account_root()), repo)
    return cage.prepare(site, cage.Policy("default"), 2, HOME)


def test_Grants_NoHostFiles_DefaultsApply(tmp_path):
    loaded = grants.load(tmp_path, ())
    assert loaded.for_class("master").tools == grants.MASTER_TOOLS and loaded.for_class("master").mcp == ()
    for name in grants.CLASSES[1:]:
        assert loaded.for_class(name).mcp == ()


def test_Grants_Default_BuilderMcpConfigListsOnlySakura(repo):
    servers = json.loads(_prepared(repo).mcp_config.read_text())["mcpServers"]
    assert list(servers) == ["sakura"]


def test_Grants_BuilderGrantedServer_InItsMcpConfigAndAllowRules(repo):
    _host('[builder]\nmcp = ["github"]\n', GITHUB)
    run_cage = _prepared(repo)
    servers = json.loads(run_cage.mcp_config.read_text())["mcpServers"]
    assert servers["github"] == {"command": str(grants.LAUNCHER), "args": ["grants", "exec", "builder", "github"],
                                 "env": {"GITHUB_TOKEN": "${GITHUB_TOKEN}"}}
    assert "mcp__github" in json.loads(run_cage.settings.read_text())["permissions"]["allow"]
    other = json.loads(_prepared(repo, role="planner").mcp_config.read_text())["mcpServers"]
    assert list(other) == ["sakura"]


def test_Grants_MasterGrantedGithub_InMastersMcpConfigOnly(tmp_path):
    _host('[master]\nmcp = ["github"]\n', GITHUB)
    loaded = _load()
    assert list(loaded.mcp_servers("master")) == ["github"]
    for name in grants.CLASSES[1:]:
        assert loaded.mcp_servers(name) == {}


def test_Grants_UnknownServerName_Refused():
    _host('[master]\nmcp = ["github"]\n', "")
    with pytest.raises(grants.GrantsError, match="master.*github.*mcp-servers.toml"):
        _load()


def test_Grants_ServerNamedLikeTheEngineServer_Refused():
    _host("", '[sakura]\ncommand = "x"\n')
    with pytest.raises(grants.GrantsError, match="sakura"):
        _load()


@pytest.mark.parametrize("text,message", [
    ("[nobody]\nmcp = []\n", "unknown class"),
    ("[builder]\ncolour = 1\n", "unknown keys"),
    ('[builder]\ntools = ["rm_all"]\n', "unknown tool"),
    ('[builder]\ntools = ["lane_open"]\n', "unknown tool"),
    ('[master]\ntools = ["approve"]\n', "unknown tool"),
    ('[builder]\nmcp = "github"\n', "list of names"),
])
def test_Grants_MalformedGrantFile_RefusedNamingTheProblem(text, message):
    _host(text, GITHUB)
    with pytest.raises(grants.GrantsError, match=message):
        _load()


@pytest.mark.parametrize("text,message", [
    ('[github]\ncommand = ""\n', "command"),
    ('[github]\ncommand = "/bin"\n', "regular file"),
    ('[github]\ncommand = "/bin/no-such-program"\n', "regular file"),
    ('[github]\ncommand = "x"\nsecret = "abc"\n', "unknown keys"),
    ('[github]\ncommand = "/bin/sh"\nenvironment = ["lower"]\n', "environment"),
    ('[github]\ncommand = "/bin/sh"\narguments = [1]\n', "arguments"),
    ('["Bad Name"]\ncommand = "x"\n', "server name"),
])
def test_Grants_MalformedServerFile_RefusedNamingTheProblem(text, message):
    _host("", text)
    with pytest.raises(grants.GrantsError, match=message):
        _load()


def test_Grants_UnreadableHostFile_RefusedNamingIt():
    root = engine_home.root()
    (root / grants.GRANTS_FILE).write_text("[builder\n")
    with pytest.raises(grants.GrantsError, match="grants.toml"):
        grants.load(root, ())


def test_Grants_RepositoryLayerNamesGrants_Refused(repo, tmp_path):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text('[grants.builder]\nmcp = ["github"]\n')
    with pytest.raises(config.FlowConfigError, match=r"^grants\.builder\.mcp may only be set by the host"):
        config.load(repo, home=tmp_path / "home")


def test_Grants_RepositoryGrantFile_NeverRead(repo):
    (repo / grants.GRANTS_FILE).write_text('[builder]\nmcp = ["github"]\n')
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / grants.GRANTS_FILE).write_text('[builder]\nmcp = ["github"]\n')
    assert list(json.loads(_prepared(repo).mcp_config.read_text())["mcpServers"]) == ["sakura"]


def test_Grants_ClassWithTools_ServerListsOnlyThoseGranted():
    _host('[builder]\ntools = ["board_read"]\n')
    granted = _load().for_class("builder")
    policy = {"run": "r", "role": "builder", "requests": "x", "tools": list(granted.tools)}
    messages = [{"id": 1, "method": "tools/list"}, {"id": 2, "method": "tools/call",
                "params": {"name": "board_post", "arguments": {"text": "hi"}}}]
    incoming = io.StringIO("".join(json.dumps(message) + "\n" for message in messages))
    outgoing = io.StringIO()
    cage_server.serve(policy, incoming, outgoing)
    listed, refused = map(json.loads, outgoing.getvalue().splitlines())
    assert [tool["name"] for tool in listed["result"]["tools"]] == ["permission", "board_read"]
    assert refused["error"]["code"] == -32602


def test_Grants_GrantedTools_WrittenIntoThePolicyAndAllowRules(repo):
    _host('[builder]\ntools = ["board_read"]\n')
    run_cage = _prepared(repo)
    assert json.loads(run_cage.policy.read_text())["tools"] == ["board_read"]
    allow = json.loads(run_cage.settings.read_text())["permissions"]["allow"]
    assert "mcp__sakura__board_read" in allow and "mcp__sakura__board_post" not in allow


def test_Grants_DefaultRoleTools_AreTheBoardAskAndKnowledgeTools():
    for name in grants.CLASSES[1:]:
        default = ("board_read", "board_post", "ask_person", "knowledge_propose")
        expected = (*default, "run_row") if name == "evaluator" else default
        assert _load().for_class(name).tools == expected


# -- a server runs outside the role's sandbox: its files must be ones no role can have planted ---------------------


def _command_file(folder: Path, name="server") -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / name
    path.write_text("#!/bin/sh\n")
    path.chmod(0o755)
    return path


def _server_with(command: str, arguments="") -> str:
    return f'[github]\ncommand = "{command}"\n{arguments}'


@pytest.mark.parametrize("command", ["github-mcp", "npx", "./bin/server", "bin/server", "../server", "~/bin/server"])
def test_Grants_BareOrRelativeCommand_RefusedNamingTheAbsoluteForm(command):
    _host("", _server_with(command))
    with pytest.raises(grants.GrantsError, match="absolute path"):
        _load()


def test_Grants_CommandInsideAGivenWritableFolder_Refused(tmp_path):
    workspace = tmp_path / "workspaces" / "demo-0"
    _host("", _server_with(str(_command_file(workspace))))
    with pytest.raises(grants.GrantsError, match=r"inside .*workspaces/demo-0.*role can write"):
        _load((workspace,))


def test_Grants_CommandInAnotherLanesWorkspace_Refused(repo):
    other = cage.workspaces_folder() / "another-project" / "lane-b-0"
    program = _command_file(other)
    _host("", _server_with(str(program)))
    with pytest.raises(grants.GrantsError, match="role can write"):
        _load()
    assert cage.workspaces_folder() in cage.writable_roots()


def test_Grants_CommandInsideTheEngineHome_Refused():
    _host("", _server_with(str(_command_file(engine_home.root() / "bin"))))
    with pytest.raises(grants.GrantsError, match="role can write"):
        _load()


def test_Grants_CommandInTheAccountTempFolder_Refused(tmp_path):
    _host("", _server_with(str(_command_file(tmp_path / "tools"))))
    with pytest.raises(grants.GrantsError, match="role can write"):
        _load()


def test_Grants_WritableRoots_NameEveryFolderARoleCanWrite():
    import tempfile
    roots = cage.writable_roots()
    for folder in ("/tmp", "/private/tmp", "/private/var/folders", "/private/var/tmp", tempfile.gettempdir(),
                   str(engine_home.root()), f"{engine_home.root()}-workspaces"):
        assert Path(os.path.realpath(folder)) in roots


def test_Grants_WritableRoots_FollowTheConfiguredWorkspaceFolder(tmp_path):
    configured = tmp_path / "elsewhere"
    (engine_home.root() / "config.toml").write_text(f'[workspace]\nroot = "{configured}/{{repo}}"\n')
    assert Path(os.path.realpath(configured)) in cage.writable_roots()


def test_Grants_SymlinkIntoWritableFolder_Refused(tmp_path):
    writable = tmp_path / "writable"
    target = _command_file(writable)
    link = tmp_path / "trusted" / "server"
    link.parent.mkdir()
    link.symlink_to(target)
    _host("", _server_with(str(link)))
    with pytest.raises(grants.GrantsError, match="role can write"):
        _load((writable,))


def test_Grants_AbsoluteHostCommand_ResolvedForTheLauncher(tmp_path):
    target = _command_file(tmp_path / "host-tools")
    link = tmp_path / "links" / "server"
    link.parent.mkdir()
    link.symlink_to(target)
    _host('[master]\nmcp = ["github"]\n', _server_with(str(link)))
    assert _load(()).servers["github"].command == os.path.realpath(target)


@pytest.mark.parametrize("argument", ["./server.js", "../server.js", "bin/server", ".hidden", "a/b",
                                      "https://example.org/x", "--root=/abs"])
def test_Grants_RelativePathArgument_Refused(argument):
    _host("", _server_with("/bin/sh", f'arguments = ["{argument}"]\n'))
    with pytest.raises(grants.GrantsError, match="argument"):
        _load()


def test_Grants_PathArgumentInsideWorkspace_Refused(tmp_path):
    workspace = tmp_path / "ws"
    _host("", _server_with("/bin/sh", f'arguments = ["{workspace}/server.js"]\n'))
    with pytest.raises(grants.GrantsError, match=r"argument.*role can write"):
        _load((workspace,))


@pytest.mark.parametrize("argument", ["-y", "--stdio", "stdio", "@modelcontextprotocol/server-github", "/usr/bin/true"])
def test_Grants_PlainArguments_Accepted(argument):
    _host("", _server_with("/bin/sh", f'arguments = ["{argument}"]\n'))
    assert _load(()).servers["github"].arguments == (argument,)


# -- the engine's launcher: `sakura grants exec <class> <server>` ---------------------------------------------------


def _granted_host(tmp_path, arguments='arguments = ["--stdio"]\n', environment=""):
    server = _command_file(tmp_path / "host-tools", "mcp-server")
    _host('[builder]\nmcp = ["github"]\n', _server_with(str(server), arguments + environment))
    return server


def test_CageMcpConfig_GrantedServerGoesThroughLauncher(repo, tmp_path, monkeypatch):
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    server = _granted_host(tmp_path)
    entry = json.loads(_prepared(repo).mcp_config.read_text())["mcpServers"]["github"]
    assert entry["command"] == str(grants.LAUNCHER) and entry["args"] == ["grants", "exec", "builder", "github"]
    assert str(server) not in json.dumps(entry) and "--stdio" not in json.dumps(entry)


def test_GrantsExec_RunsInPrivateFolderWithFixedPath(tmp_path, monkeypatch):
    server = _granted_host(tmp_path, environment='environment = ["GITHUB_TOKEN"]\n')
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    launch = grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "workspace",
                                 environment={"PATH": "/tmp/planted:/usr/bin", "TMUX": "x", "TMUX_PANE": "%1",
                                              "TMUX_TMPDIR": "/x", "GITHUB_TOKEN": "t", "HOME": "/home/me"})
    assert launch.argv == [os.path.realpath(server), "--stdio"]
    assert launch.cwd == engine_home.root() / grants_exec.FOLDER and launch.cwd.is_dir()
    assert launch.cwd.stat().st_mode & 0o777 == 0o700 and list(launch.cwd.iterdir()) == []
    assert launch.environment["PATH"] == os.pathsep.join([*git_program.SAFE_PATH, os.path.dirname(os.path.realpath(server))])
    assert "/tmp/planted" not in launch.environment["PATH"]
    assert not [name for name in launch.environment if name.startswith("TMUX")]
    assert launch.environment["GITHUB_TOKEN"] == "t" and launch.environment["HOME"] == "/home/me"


AGENT_WRITABLE = {"npm_config_cache": "/agent/tmp/npm", "PIP_CACHE_DIR": "/agent/tmp/pip", "UV_CACHE_DIR": "/agent/tmp/uv",
                  "XDG_CACHE_HOME": "/agent/tmp/xdg", "XDG_CONFIG_HOME": "/agent/tmp/config", "TMPDIR": "/agent/tmp",
                  "PYTHONPATH": "/agent/tmp/py", "NODE_OPTIONS": "--require /agent/tmp/x.js", "NODE_PATH": "/agent/tmp/node",
                  "PYTHONSTARTUP": "/agent/tmp/s.py", "BASH_ENV": "/agent/tmp/b", "GIT_CONFIG_GLOBAL": "/agent/tmp/g",
                  "LD_PRELOAD": "/agent/tmp/l.so", "DYLD_INSERT_LIBRARIES": "/agent/tmp/l.dylib", "SOME_TOKEN": "t"}


def test_GrantsExec_AgentWritableCacheConfigAndImportPaths_NeverReachTheServer(tmp_path, monkeypatch):
    _granted_host(tmp_path, environment='environment = ["GITHUB_TOKEN"]\n')
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    given = {**AGENT_WRITABLE, "GITHUB_TOKEN": "t", "HOME": "/home/me", "LANG": "en_US.UTF-8", "PATH": "/agent/tmp:/usr/bin"}
    launch = grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "workspace", environment=given)
    environment = launch.environment
    assert not set(environment) & {"PYTHONPATH", "NODE_OPTIONS", "NODE_PATH", "PYTHONSTARTUP", "BASH_ENV", "GIT_CONFIG_GLOBAL",
                                   "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "SOME_TOKEN"}
    assert environment["GITHUB_TOKEN"] == "t" and environment["HOME"] == "/home/me" and environment["LANG"] == "en_US.UTF-8"
    for name in ("npm_config_cache", "PIP_CACHE_DIR", "UV_CACHE_DIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "TMPDIR"):
        folder = Path(environment[name])
        assert engine_home.root() in folder.parents and folder.is_dir() and folder.stat().st_mode & 0o777 == 0o700, name
        assert "/agent/" not in environment[name]


def test_GrantsExec_PrivateCacheSurvivesBetweenLaunchesAndStaysUnderTheEngineHome(tmp_path, monkeypatch):
    _granted_host(tmp_path)
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    first = grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "w", environment={})
    (Path(first.environment["npm_config_cache"]) / "kept").write_text("x")
    second = grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "w", environment={})
    assert (Path(second.environment["npm_config_cache"]) / "kept").read_text() == "x"


def test_GrantsExec_EntryChangedSinceConfig_Refused(tmp_path, monkeypatch):
    server = _granted_host(tmp_path)
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    planted = _command_file(tmp_path / "workspace")
    (engine_home.root() / grants.SERVERS_FILE).write_text(_server_with(str(planted)))
    with pytest.raises(grants.GrantsError, match="role can write"):
        grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "workspace", environment={})
    (engine_home.root() / grants.SERVERS_FILE).write_text(_server_with(str(server)))
    (engine_home.root() / grants.GRANTS_FILE).write_text("")
    with pytest.raises(grants.GrantsError, match="not granted"):
        grants_exec.prepare(engine_home.root(), "builder", "github", cwd=tmp_path / "workspace", environment={})


def test_GrantsExec_WorkingFolderIsWritable_CommandThereRefused(tmp_path, monkeypatch):
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    workspace = tmp_path / "workspace"
    server = _command_file(workspace, "mcp-server")
    _host('[builder]\nmcp = ["github"]\n', _server_with(str(server)))
    with pytest.raises(grants.GrantsError, match="role can write"):
        grants_exec.prepare(engine_home.root(), "builder", "github", cwd=workspace, environment={})


@pytest.mark.parametrize("arguments", [["builder"], ["builder", "github", "extra"], ["nobody", "github"],
                                       ["builder", "Bad Name"]])
def test_GrantsExec_BadArguments_Refused(arguments, capsys):
    assert grants_exec.main(arguments) == 2
    assert capsys.readouterr().err


def test_connector_grants_are_explicit_and_digest_is_grant_free(tmp_path):
    (tmp_path / grants.GRANTS_FILE).write_text('[master.connectors]\ncn-one = ["read", "comment_pr"]\n'
                                              '[builder.connectors]\ncn-two = ["read"]\n')
    loaded = grants.load(tmp_path, ())
    assert loaded.for_class("master").connectors == {"cn-one": ("read", "comment_pr")}
    assert loaded.for_class("builder").connectors == {"cn-two": ("read",)}
    assert loaded.for_class("digest").connectors == {}
