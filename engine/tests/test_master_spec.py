"""The master's launch files: its cage, instructions, MCP config and the spec the app starts it from."""

import time
import json
import os
from pathlib import Path

import pytest

from sakura import engine_home, programs
from sakura.flow import brakes, exits, presets, registry
from sakura.master import cli, spec

ACCOUNT_HOME = Path("/home/someone")
STANDARD = presets.BUILTINS["standard"]
HOST_TOOLS = {}


@pytest.fixture(autouse=True)
def host_tools(tmp_path):
    """Real files the host's choices can name (the trust check refuses a path that does not exist)."""
    folder = tmp_path / "host"
    folder.mkdir()
    for name in ("adapter", "claude", "file-adapter", "env-adapter"):
        (folder / name).write_text("#!/bin/sh\nexit 0\n")
        (folder / name).chmod(0o755)
    HOST_TOOLS["folder"] = os.path.realpath(folder)


def _h(name):
    return f"{HOST_TOOLS['folder']}/{name}"
SOLO = presets.BUILTINS["solo"]


def _prepare(preset=STANDARD, environ=None):
    return spec.prepare(engine_home.root(), preset, account_home=ACCOUNT_HOME, environ=environ or {})


def _settings(prepared) -> dict:
    return json.loads(prepared.settings_path.read_text())


def _filesystem(prepared) -> dict:
    return _settings(prepared)["sandbox"]["filesystem"]


def test_MasterSpec_Settings_WritesOnlyTheMasterFolder():
    prepared = _prepare()
    folder = Path(os.path.realpath(prepared.cwd))
    assert folder == Path(os.path.realpath(spec.master_folder(engine_home.root())))
    writable = [Path(path) for path in _filesystem(prepared)["allowWrite"]]
    assert writable and all(path == folder or folder in path.parents for path in writable)
    assert _settings(prepared)["permissions"]["blockWritesOutsideWorkingDirectories"] is True


def test_MasterSpec_Folder_IsOutsideTheEngineHome():
    folder = spec.master_folder(engine_home.root())
    assert engine_home.root() not in folder.parents and folder != engine_home.root()
    assert spec.master_folder(Path("/x/Sakura/engine")) == Path("/x/Sakura/master")


def test_MasterSpec_Settings_EngineHomeDeniedForRead():
    prepared = _prepare()
    assert str(engine_home.root()) in _filesystem(prepared)["denyRead"]
    assert f"Read(/{engine_home.root()}/**)" in _settings(prepared)["permissions"]["deny"]


def test_MasterSpec_RegisteredRepository_ReadableNotWritable(repo):
    (repo / "src").mkdir()
    registry.remember(repo)
    filesystem = _filesystem(_prepare())
    assert str(repo / "README") in filesystem["allowRead"] and str(repo / "src") in filesystem["allowRead"]
    assert not any(Path(path) == repo or repo in Path(path).parents for path in filesystem["allowWrite"])


def test_MasterSpec_RepositoryGitFolder_Denied(repo):
    registry.remember(repo)
    allowed = _filesystem(_prepare())["allowRead"]
    assert str(repo) not in allowed and str(repo / ".git") not in allowed
    assert not any(str(repo / ".git") in path for path in allowed)


def test_MasterSpec_RepositoryLinkAtTheTop_NotGranted(repo, tmp_path):
    (tmp_path / "elsewhere").mkdir()
    (repo / "pointer").symlink_to(tmp_path / "elsewhere")
    registry.remember(repo)
    assert str(repo / "pointer") not in _filesystem(_prepare())["allowRead"]


def test_MasterSpec_SoloPreset_DeniesSubagentTools():
    assert {"Agent", "Task"} <= set(_settings(_prepare(SOLO))["permissions"]["deny"])
    assert not {"Agent", "Task"} & set(_settings(_prepare(STANDARD))["permissions"]["deny"])


def test_MasterSpec_InsideRoleSession_Refused(monkeypatch, capsys):
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    assert cli.main(["spec", "--json"]) == exits.USAGE
    assert brakes.ENGINE_ROLE_REFUSAL in capsys.readouterr().out
    assert not spec.master_folder(engine_home.root()).exists()


def test_MasterSpec_InsideAgentCage_Refused(monkeypatch, capsys):
    monkeypatch.setattr(engine_home, "sandboxed", lambda: True)
    assert cli.main(["spec", "--json"]) == exits.USAGE
    assert cli.SANDBOX_REFUSAL in capsys.readouterr().out


def test_MasterSpec_InstructionsFile_WriteDenied():
    prepared = _prepare()
    assert str(prepared.instructions_path) in _filesystem(prepared)["denyWrite"]
    assert prepared.instructions_path.parent == prepared.cwd and "You are the master" in prepared.instructions_path.read_text()


def test_MasterSpec_HostInstructions_AppendedAtEveryStart():
    (engine_home.root() / spec.HOST_INSTRUCTIONS_NAME).write_text("Answer in Italian.\n")
    assert "Answer in Italian." in _prepare().instructions_path.read_text()
    (engine_home.root() / spec.HOST_INSTRUCTIONS_NAME).write_text("Answer in Dutch.\n")
    text = _prepare().instructions_path.read_text()
    assert "Answer in Dutch." in text and "Italian" not in text


def test_MasterSpec_InstructionsTargetPlantedAsLink_RefusedAndLinkTargetUnchanged(tmp_path):
    prepared = _prepare()
    victim = tmp_path / "victim.txt"
    victim.write_text("keep")
    prepared.instructions_path.unlink()
    prepared.instructions_path.symlink_to(victim)
    with pytest.raises(spec.MasterRefused, match="not a regular file"):
        _prepare()
    assert victim.read_text() == "keep" and prepared.instructions_path.is_symlink()


def test_MasterSpec_InstructionsTemporaryNamePlantedAsLink_LinkTargetUnchanged(tmp_path):
    prepared = _prepare()
    victim = tmp_path / "victim.txt"
    victim.write_text("keep")
    for name in (f".{spec.INSTRUCTIONS_NAME}.{os.getpid()}", f".{spec.INSTRUCTIONS_NAME}.part"):
        (prepared.cwd / name).symlink_to(victim)
    again = _prepare()
    assert victim.read_text() == "keep" and "You are the master" in again.instructions_path.read_text()
    assert not again.instructions_path.is_symlink() and again.instructions_path.stat().st_mode & 0o777 == 0o600


def test_MasterSpec_InstructionsTemporaryFile_NeverLeftBehind():
    prepared = _prepare()
    assert sorted(path.name for path in prepared.cwd.iterdir() if path.name.startswith(".instructions")) == []


def test_MasterSpec_Hooks_NoneBecauseThereIsNoTask():
    assert _settings(_prepare())["hooks"] == {}


def test_MasterSpec_McpConfig_NamesTheMasterServerAndTheMasterToolsAreAllowed():
    prepared = _prepare(environ={engine_home.ENV_VARIABLE: str(engine_home.root())})
    entry = json.loads(prepared.mcp_config_path.read_text())["mcpServers"]["sakura"]
    assert entry["args"][:2] == ["master", "serve"] and entry["env"] == {engine_home.ENV_VARIABLE: str(engine_home.root())}
    allowed = _settings(prepared)["permissions"]["allow"]
    assert "mcp__sakura__lane_open" in allowed and "mcp__sakura__lane_stop" in allowed


def test_MasterSpec_AcpServers_AreTheConfigInTheProtocolShape():
    prepared = _prepare(environ={engine_home.ENV_VARIABLE: str(engine_home.root())})
    [server] = prepared.mcp_servers
    assert server["name"] == "sakura" and server["args"][:2] == ["master", "serve"]
    assert server["env"] == [{"name": engine_home.ENV_VARIABLE, "value": str(engine_home.root())}]


def test_MasterSpec_StartMode_IsDefaultAndSessionsMustBeClosed():
    document = _prepare().document()
    assert document["start_mode"] == "default" and document["close_sessions"] is True
    assert "CLAUDECODE" in document["unset_env"] and "TMUX" in document["unset_env"]


def test_MasterSpec_AdapterAndClaude_ComeFromTheEnvironmentOnly():
    environ = {spec.ADAPTER_VARIABLE: _h("adapter"), spec.CLAUDE_VARIABLE: _h("claude")}
    prepared = _prepare(environ=environ)
    assert prepared.argv == [_h("adapter")] and prepared.env[spec.CLAUDE_VARIABLE] == _h("claude")
    assert prepared.terminal_argv[0] == _h("claude")
    assert _prepare(environ={}).argv == []


def test_MasterSpec_TerminalCommand_IsStrictAndExplicit():
    argv = _prepare(environ={spec.CLAUDE_VARIABLE: _h("claude")}).terminal_argv
    assert argv[argv.index("--permission-mode") + 1] == "default" and "--strict-mcp-config" in argv
    assert argv[argv.index("--setting-sources") + 1] == "" and "--dangerously-skip-permissions" not in argv
    assert "You are the master" in argv[argv.index("--append-system-prompt") + 1]


def test_MasterSpec_Model_IsTheLeadTier():
    assert _prepare().options["model"] == "opus"
    assert _prepare().options["settingSources"] == []


def test_MasterSpec_FolderReplacedByLink_Refused(tmp_path):
    folder = spec.master_folder(engine_home.root())
    (tmp_path / "elsewhere").mkdir()
    folder.symlink_to(tmp_path / "elsewhere")
    with pytest.raises(spec.MasterRefused, match="real folder"):
        _prepare()


def test_MasterCli_SpecJson_PrintsValidJson(capsys):
    assert cli.main(["spec", "--json"]) == exits.OK
    document = json.loads(capsys.readouterr().out)
    assert document["start_mode"] == "default" and Path(document["cwd"]).is_dir()


def test_MasterCli_NoVerb_ExecsTheTerminalMasterInItsFolder(monkeypatch):
    monkeypatch.setenv(spec.CLAUDE_VARIABLE, _h("claude"))
    monkeypatch.setenv("CLAUDECODE", "1")
    started = []
    monkeypatch.setattr(os, "chdir", lambda path: started.append(("cd", Path(path))))
    assert cli.main([], execute=lambda program, argv, env: started.append((program, argv[:1], env))) == exits.OK
    assert started[0] == ("cd", spec.master_folder(engine_home.root()))
    program, _, env = started[1]
    assert program == _h("claude") and "CLAUDECODE" not in env


def test_MasterCli_NoClaude_SaysSo(monkeypatch, capsys, tmp_path):
    monkeypatch.delenv(spec.CLAUDE_VARIABLE, raising=False)
    monkeypatch.setenv("PATH", "/nonexistent")
    monkeypatch.setattr(programs, "INSTALL_FOLDERS", (str(tmp_path / "no-install-place"),))
    monkeypatch.setattr(programs, "_resolved", {})
    assert cli.main([], execute=lambda *args: pytest.fail("started")) == exits.USAGE
    assert cli.NO_CLAUDE in capsys.readouterr().err


def test_MasterSpec_AcpOptions_StrictMcpSoNoUserServerStarts():
    options = _prepare().options
    assert options["strictMcpConfig"] is True and options["settingSources"] == []


def test_MasterSpec_Instructions_ReachTheAdapterAsASystemPromptAppend():
    (engine_home.root() / spec.HOST_INSTRUCTIONS_NAME).write_text("Answer in Italian.\n")
    prepared = _prepare()
    document = prepared.document()
    assert document["system_prompt_append"] == prepared.instructions_path.read_text()
    assert "You are the master" in document["system_prompt_append"] and "Answer in Italian." in document["system_prompt_append"]


def test_MasterSpec_Environment_IsAnExplicitAllowList():
    document = _prepare().document()
    allowed = set(document["env_allow"])
    assert {"PATH", "HOME", "CLAUDE_CONFIG_DIR", spec.CLAUDE_VARIABLE, engine_home.ENV_VARIABLE} <= allowed
    for secret in ("GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "ANTHROPIC_API_KEY", "NPM_TOKEN", "OPENAI_API_KEY"):
        assert secret not in allowed


def test_MasterCli_TerminalMaster_GetsOnlyTheAllowListAndTheCageEnvironment(monkeypatch):
    monkeypatch.setenv(spec.CLAUDE_VARIABLE, _h("claude"))
    for name, value in (("GITHUB_TOKEN", "ghp_secret"), ("AWS_SECRET_ACCESS_KEY", "aws"), ("NODE_OPTIONS", "--require x"),
                        ("PYTHONPATH", "/x"), ("CLAUDECODE", "1")):
        monkeypatch.setenv(name, value)
    started = []
    monkeypatch.setattr(os, "chdir", lambda path: None)
    assert cli.main([], execute=lambda program, argv, env: started.append(env)) == exits.OK
    [env] = started
    assert not {"GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "NODE_OPTIONS", "PYTHONPATH", "CLAUDECODE"} & set(env)
    assert env["PATH"] and env["TMPDIR"] and env[spec.CLAUDE_VARIABLE] == _h("claude")
    assert set(env) <= set(spec.ENV_ALLOW) | {"TMUX_TMPDIR"} | set(_prepare().options["env"])


# -- an app started from Finder (proof defect D2): absolute paths, and a PATH built from known directories


def _fake_install(tmp_path, monkeypatch, *names):
    from sakura import programs

    folder = tmp_path / "install"
    folder.mkdir(exist_ok=True)
    for name in names:
        tool = folder / name
        tool.write_text("#!/bin/sh\nexit 0\n")
        tool.chmod(0o755)
    monkeypatch.setattr(programs, "INSTALL_FOLDERS", (str(folder),))
    monkeypatch.setattr(programs, "_resolved", {})
    monkeypatch.delenv(spec.CLAUDE_VARIABLE, raising=False)
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    return folder


def test_MasterSpec_NodeAdapter_CarriesAbsolutePathsOfNodeAndTheAdapter(tmp_path, monkeypatch):
    folder = _fake_install(tmp_path, monkeypatch, "node", "claude")
    adapter = tmp_path / "adapter" / "index.js"
    adapter.parent.mkdir()
    adapter.write_text("#!/usr/bin/env node\nconsole.log(1)\n")
    (tmp_path / "bin").mkdir()
    link = tmp_path / "bin" / "claude-agent-acp"
    link.symlink_to(adapter)

    document = _prepare(environ={spec.ADAPTER_VARIABLE: str(link)}).document()

    node, real = os.path.realpath(folder / "node"), os.path.realpath(adapter)
    assert document["node"] == node and document["adapter"] == real and document["argv"] == [node, real]
    assert document["env"][spec.CLAUDE_VARIABLE] == os.path.realpath(folder / "claude")


def test_MasterSpec_NonNodeAdapter_IsStartedByItself(tmp_path, monkeypatch):
    _fake_install(tmp_path, monkeypatch, "node")
    native = tmp_path / "adapter-native"
    native.write_bytes(b"\xcf\xfa\xed\xfe binary")
    native.chmod(0o755)
    document = _prepare(environ={spec.ADAPTER_VARIABLE: str(native)}).document()
    assert document["argv"] == [os.path.realpath(native)]


def test_MasterSpec_Path_IsBuiltFromKnownAbsoluteDirectoriesNotTheCallers(tmp_path, monkeypatch):
    folder = _fake_install(tmp_path, monkeypatch, "node", "claude")
    document = _prepare(environ={"PATH": "/workspace/.venv/bin:relative:", spec.ADAPTER_VARIABLE: _h("adapter")}).document()
    entries = document["env"]["PATH"].split(os.pathsep)
    assert document["path"] == document["env"]["PATH"] and str(folder) in entries
    assert all(os.path.isabs(entry) for entry in entries) and "/workspace/.venv/bin" not in entries
    for known in ("/usr/bin", "/bin"):
        assert known in entries


def test_MasterSpec_NoNode_FieldsAreEmptyAndTheAdapterIsStartedAsGiven(tmp_path, monkeypatch):
    _fake_install(tmp_path, monkeypatch)
    document = _prepare(environ={spec.ADAPTER_VARIABLE: _h("adapter")}).document()
    assert document["node"] == "" and document["argv"] == [_h("adapter")]


# -- "in parallel" means one lane per independent part (proof defect D5)


def _instructions() -> str:
    return _prepare().instructions_path.read_text()


def test_MasterInstructions_TwoIndependentOutcomes_AreTwoLanesOpenedInTheSameTurn():
    text = " ".join(_instructions().split())
    assert '"a --help option and a --version option, in parallel"' in text
    assert "one lane per outcome" in text and "in the same turn" in text
    assert "up to the number of lanes you may have open" in text and "Never open one lane and wait" in text


def test_MasterInstructions_OutcomesTouchingOneFile_StillGetOneLaneEach():
    text = " ".join(_instructions().split())
    assert "even when both change the same file" in text and "never a reason to merge" in text
    assert "the board and the engine's delivery check" in text


def test_MasterInstructions_OneLaneOnlyWhenOnePartNeedsTheOthersResult():
    text = " ".join(_instructions().split())
    assert "needs the result of the other" in text and "go in one lane" in text
    assert "depend on each other" not in text  # the old sentence the model read as a reason to merge


def test_MasterInstructions_MoreOutcomesThanLanes_SaysWhichLanesAreOpenAndWhatWaits():
    text = " ".join(_instructions().split())
    assert "tell the person which lanes are open and what waits" in text


# -- the adapter comes from host configuration first (proof defect D2b): an app opened from Finder sets no variable


def _host_file(text):
    (engine_home.root() / programs.HOST_FILE).write_text(text)


def test_MasterSpec_AdapterFromTheHostFile_NoVariableSet():
    _host_file(f'acp_adapter = "{_h("file-adapter")}"\n')
    document = _prepare(environ={}).document()
    assert document["argv"] == [_h("file-adapter")] and document["adapter"] == _h("file-adapter")
    assert document["adapter_problem"] == ""


def test_MasterSpec_AdapterHostFile_BeatsTheVariable():
    _host_file(f'acp_adapter = "{_h("file-adapter")}"\n')
    assert _prepare(environ={spec.ADAPTER_VARIABLE: _h("env-adapter")}).document()["argv"] == [_h("file-adapter")]


def test_MasterSpec_AdapterVariable_WhenTheFileNamesNone():
    _host_file('claude = "/x"\n')
    document = _prepare(environ={spec.ADAPTER_VARIABLE: _h("env-adapter")}).document()
    assert document["argv"] == [_h("env-adapter")] and document["adapter_problem"] == ""


def test_MasterSpec_NoAdapterNamed_TheProblemNamesTheFileAndTheKey():
    document = _prepare(environ={}).document()
    problem = document["adapter_problem"]
    assert document["argv"] == [] and programs.HOST_FILE in problem and "acp_adapter" in problem
    assert str(engine_home.root()) in problem and spec.ADAPTER_VARIABLE in problem


@pytest.mark.parametrize("value", ['"relative/adapter"', '""', "3", '"~/adapter"'])
def test_MasterSpec_AdapterHostEntryNotAnAbsolutePath_ProblemNamesTheKeyAndNothingStarts(value):
    _host_file(f"acp_adapter = {value}\n")
    document = _prepare(environ={spec.ADAPTER_VARIABLE: _h("env-adapter")}).document()
    assert document["argv"] == [] and "acp_adapter" in document["adapter_problem"] and "absolute" in document["adapter_problem"]


# -- the master's programs go through the same trust check (review 25, finding 2) ------------------------------------------------


def _open_folder_tool(tmp_path, name):
    folder = tmp_path / "open"
    folder.mkdir(exist_ok=True)
    tool = folder / name
    tool.write_text("#!/bin/sh\nexit 0\n")
    tool.chmod(0o755)
    folder.chmod(0o777)
    return tool


def test_MasterSpec_ClaudeExecutableThroughALinkIntoAWritableFolder_IsRefusedWithTheReason(tmp_path):
    target = _open_folder_tool(tmp_path, "claude")
    link = tmp_path / "bin-claude"
    link.symlink_to(target)
    document = _prepare(environ={spec.CLAUDE_VARIABLE: str(link)}).document()
    assert "can be written" in document["claude_problem"] and spec.CLAUDE_VARIABLE in document["claude_problem"]
    assert spec.CLAUDE_VARIABLE not in document["env"] and document["terminal_argv"] == []


def test_MasterSpec_ClaudeExecutableOfATrustedFile_IsPassedAsItsCanonicalPath(tmp_path):
    link = tmp_path / "claude-link"
    link.symlink_to(_h("claude"))
    document = _prepare(environ={spec.CLAUDE_VARIABLE: str(link)}).document()
    assert document["claude_problem"] == "" and document["env"][spec.CLAUDE_VARIABLE] == _h("claude")


def test_MasterSpec_AdapterInAFolderOthersCanWrite_IsRefusedAndNothingStarts(tmp_path):
    adapter = _open_folder_tool(tmp_path, "adapter.js")
    document = _prepare(environ={spec.ADAPTER_VARIABLE: str(adapter)}).document()
    assert document["argv"] == [] and "may not run outside the cage" in document["adapter_problem"]


def test_MasterSpec_AdapterThroughALinkIntoAWritableFolder_IsRefused(tmp_path):
    target = _open_folder_tool(tmp_path, "real-adapter.js")
    link = tmp_path / "adapter-link"
    link.symlink_to(target)
    assert _prepare(environ={spec.ADAPTER_VARIABLE: str(link)}).document()["argv"] == []


def test_MasterSpec_AdapterFromTheHostFileInAWritableFolder_IsRefusedToo(tmp_path):
    adapter = _open_folder_tool(tmp_path, "adapter.js")
    _host_file(f'acp_adapter = "{adapter}"\n')
    document = _prepare(environ={}).document()
    assert document["argv"] == [] and "can be written" in document["adapter_problem"]


def test_MasterSpec_NodeInAFolderOthersCanWrite_IsNeverChosen(tmp_path, monkeypatch):
    from sakura import programs

    _open_folder_tool(tmp_path, "node")
    monkeypatch.setattr(programs, "INSTALL_FOLDERS", (str(tmp_path / "open"),))
    monkeypatch.setattr(programs, "_resolved", {})
    monkeypatch.delenv(spec.CLAUDE_VARIABLE, raising=False)
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    assert _prepare(environ={spec.ADAPTER_VARIABLE: _h("adapter")}).document()["node"] == ""


def test_MasterSpec_ClaudeExecutableInsideARegisteredRepository_IsRefused(repo, tmp_path, monkeypatch):
    from sakura.flow import registry

    registry.remember(repo)
    inside = repo / ".venv" / "bin" / "claude"
    inside.parent.mkdir(parents=True)
    inside.write_text("#!/bin/sh\nexit 0\n")
    inside.chmod(0o755)
    document = _prepare(environ={spec.CLAUDE_VARIABLE: str(inside)}).document()
    assert "where a repository or an agent can write" in document["claude_problem"]


def test_MasterSpec_AdapterInsideTheRepositoryTheMasterRunsIn_IsRefused(repo, monkeypatch):
    monkeypatch.chdir(repo)
    inside = repo / "node_modules" / ".bin" / "adapter"
    inside.parent.mkdir(parents=True)
    inside.write_text("#!/bin/sh\nexit 0\n")
    inside.chmod(0o755)
    document = _prepare(environ={spec.ADAPTER_VARIABLE: str(inside)}).document()
    assert document["argv"] == [] and "where a repository or an agent can write" in document["adapter_problem"]


def test_Instructions_TellTheMasterToNameThingsAndGiveLocalTimes():
    text = _prepare().instructions_path.read_text()
    assert "Speaking to the person" in text
    assert "never an id" in text and "local time" in text
    assert "UTC" in text and "time zone is" in text


def test_Instructions_NameThePersonsTimeZone(monkeypatch):
    monkeypatch.setenv("TZ", "Europe/Rome")
    time.tzset()
    try:
        text = _prepare().instructions_path.read_text()
    finally:
        monkeypatch.delenv("TZ")
        time.tzset()
    assert "time zone is" in text and ("CET" in text or "CEST" in text) and "UTC+0" in text


def test_Instructions_SayWhichFieldNamesALaneAndThatIdsAreForTools():
    text = _prepare().instructions_path.read_text()
    assert "`project_name`" in text and "for calling tools only" in text


def _probe(prepared, path) -> bool:
    """Whether `cat path` succeeds under the master's own Seatbelt profile."""
    import subprocess
    profile = prepared.settings_path.parent / "sandbox.sb"
    done = subprocess.run(["/usr/bin/sandbox-exec", "-f", str(profile), "/bin/cat", str(path)],
                          capture_output=True, text=True, timeout=30)
    return done.returncode == 0


def _home_with(tmp_path):
    home = tmp_path / "person"
    (home / "Documents").mkdir(parents=True)
    (home / "Documents" / "note.txt").write_text("PRIVATE")
    return home


def _prepare_in(home, environ=None):
    return spec.prepare(engine_home.root(), STANDARD, account_home=home, environ=environ or {})


def test_MasterCage_ProbeReadsAFileInAnotherFolderOfTheHome_Denied(repo, tmp_path):
    home = _home_with(tmp_path)
    registry.remember(repo)
    assert not _probe(_prepare_in(home), home / "Documents" / "note.txt")


def test_MasterCage_ProbeReadsARegisteredRepositoryFile_Allowed(repo, tmp_path):
    registry.remember(repo)
    assert _probe(_prepare_in(_home_with(tmp_path)), repo / "README")


def test_MasterCage_ProbeReadsTheGitFolderOfARegisteredRepository_Denied(repo, tmp_path):
    registry.remember(repo)
    assert not _probe(_prepare_in(_home_with(tmp_path)), repo / ".git" / "HEAD")


def test_MasterCage_ReadTool_IsBlockedOutsideItsFoldersAndTheRegisteredOnes(repo, tmp_path):
    registry.remember(repo)
    permissions = _settings(_prepare_in(_home_with(tmp_path)))["permissions"]
    assert permissions["blockReadsOutsideWorkingDirectories"] is True
    assert os.path.realpath(repo) in permissions["additionalDirectories"]


def test_MasterCage_ReadTool_NeverReachesAGitFolderOfARegisteredRepository(repo, tmp_path):
    registry.remember(repo)
    deny = _settings(_prepare_in(_home_with(tmp_path)))["permissions"]["deny"]
    git = os.path.realpath(repo / ".git")
    assert f"Read(/{git}/**)" in deny and f"Read(/{git})" in deny


def test_MasterCage_NewRepositoryRegistered_ReadableAtTheNextStart(repo, tmp_path):
    home = _home_with(tmp_path)
    before = _settings(_prepare_in(home))["permissions"]["additionalDirectories"]
    assert os.path.realpath(repo) not in before
    registry.remember(repo)
    assert os.path.realpath(repo) in _settings(_prepare_in(home))["permissions"]["additionalDirectories"]
    assert _probe(_prepare_in(home), repo / "README")


def test_MasterCage_ReadTool_KnowledgeFoldersAreReachable(tmp_path, monkeypatch):
    notes = tmp_path / "notes"
    notes.mkdir()
    monkeypatch.setattr(spec, "_knowledge", lambda values, account, folder: {"shared": str(notes)})
    directories = _settings(_prepare_in(_home_with(tmp_path)))["permissions"]["additionalDirectories"]
    assert str(notes) in directories


def test_MasterCage_ReadRoots_AreNeverWritableByTheEditTools(repo, tmp_path):
    registry.remember(repo)
    deny = _settings(_prepare_in(_home_with(tmp_path)))["permissions"]["deny"]
    root = os.path.realpath(repo)
    assert f"Edit(/{root}/**)" in deny


def test_MasterCage_EnvFileOfARegisteredRepository_DeniedToEveryTool(repo, tmp_path):
    (repo / ".env").write_text("TOKEN=secret")
    registry.remember(repo)
    prepared = _prepare_in(_home_with(tmp_path))
    deny = _settings(prepared)["permissions"]["deny"]
    assert f"Read(/{os.path.realpath(repo)}/**/.env*)" in deny
    assert not _probe(prepared, repo / ".env")
    assert _probe(prepared, repo / "README")


def _access_text(tmp_path, monkeypatch=None, knowledge=None):
    if knowledge is not None:
        monkeypatch.setattr(spec, "_knowledge", lambda values, account, folder: knowledge)
    text = _prepare_in(_home_with(tmp_path)).instructions_path.read_text()
    return text.split("## What you can read and write", 1)[1].split("\n## ", 1)[0]


def test_MasterInstructions_Access_NamesTheOwnFolderAsTheOnlyPlaceItWrites(tmp_path):
    section = _access_text(tmp_path)
    folder = spec.master_folder(engine_home.root())
    assert f"write only in your own folder ({folder})" in section


def test_MasterInstructions_Access_ListsExactlyTheRegisteredRepositoriesAndKnowledgeFolders(repo, tmp_path, monkeypatch):
    registry.remember(repo)
    notes = tmp_path / "notes"
    notes.mkdir()
    section = _access_text(tmp_path, monkeypatch, {"shared": str(notes)})
    assert f"- {repo}" in section and f"- {notes}" in section
    assert str(tmp_path / "person") not in section


def test_MasterInstructions_Access_SaysWhatItCannotRead(tmp_path):
    section = " ".join(_access_text(tmp_path).split())
    for denied in ("no other folder of the home", "no credentials", "no Git folder", "only through your tools"):
        assert denied in section
    assert "never claim more" in section



@pytest.fixture
def leeway_book(tmp_path, monkeypatch):
    """A fake Leeway with a default Claude account, a second one and a Codex one, each with its own folder."""
    from sakura.usage import leeway
    from leeway_support import account, make_helper
    folders = {"main": Path("~/.claude").expanduser(), "work": tmp_path / "work-claude", "codex": tmp_path / "codex-home"}
    rows = []
    for ident, provider in (("main", "claude"), ("work", "claude"), ("codex", "codex")):
        status_row, book_row = account(ident, provider=provider, label={"main": "Claude", "work": "Claude Work",
                                                                        "codex": "Codex"}[ident])
        status_row["config_dir"] = str(folders[ident])
        rows.append((status_row, book_row))
    hidden_row, hidden_book = account("old", hidden=True)
    rows.append((hidden_row, hidden_book))
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=rows)))
    leeway.forget()
    yield folders
    leeway.forget()


def _prepare_for(account_id):
    return spec.prepare(engine_home.root(), STANDARD, account_home=ACCOUNT_HOME, environ={},
                        account=spec.account_named(account_id))


def test_MasterSpec_SecondClaudeAccount_NamesItsFolderForTheAdapterAndTheSession(leeway_book):
    document = _prepare_for("work").document()
    folder = str(leeway_book["work"])
    assert document["env"]["CLAUDE_CONFIG_DIR"] == folder
    assert document["options"]["env"]["CLAUDE_CONFIG_DIR"] == folder
    assert (document["account"], document["account_provider"], document["account_folder"]) == ("work", "claude", folder)
    assert document["codex_home"] == ""


def test_MasterSpec_NoAccount_KeepsToday_sBehaviour(leeway_book):
    document = _prepare().document()
    assert "CLAUDE_CONFIG_DIR" not in document["env"] and "CLAUDE_CONFIG_DIR" not in document["options"]["env"]
    assert (document["account"], document["account_provider"], document["account_folder"], document["codex_home"]) == ("", "", "", "")
    assert "CLAUDE_CONFIG_DIR" not in document["unset_env"]


def test_MasterSpec_DefaultClaudeAccount_LeavesTheFolderUnsetAndSaysSo(leeway_book):
    document = _prepare_for("main").document()
    assert "CLAUDE_CONFIG_DIR" not in document["env"] and "CLAUDE_CONFIG_DIR" not in document["options"]["env"]
    assert document["account"] == "main" and document["account_folder"] == str(leeway_book["main"])
    assert "CLAUDE_CONFIG_DIR" in document["unset_env"]


def test_MasterSpec_CodexAccount_GivesItsCodexHomeAndNoClaudeFolder(leeway_book):
    document = _prepare_for("codex").document()
    assert (document["account_provider"], document["codex_home"]) == ("codex", str(leeway_book["codex"]))
    assert "CLAUDE_CONFIG_DIR" not in document["env"] and "CODEX_HOME" not in document["env"]


def test_MasterSpec_AccountNamedByItsLabel_IsFound(leeway_book):
    assert _prepare_for("Claude Work").document()["account"] == "work"


@pytest.mark.parametrize("name", ["nobody", "old"])
def test_MasterSpec_UnknownOrHiddenAccount_IsRefusedNamingIt(leeway_book, name):
    with pytest.raises(spec.MasterRefused, match=name):
        spec.account_named(name)


def test_MasterSpec_AccountWithoutLeeway_IsRefusedWithTheReason(monkeypatch):
    from sakura.usage import leeway
    monkeypatch.delenv(leeway.ENV_VARIABLE, raising=False)
    leeway.forget()
    with pytest.raises(spec.MasterRefused, match="account"):
        spec.account_named("work")


def test_MasterCli_SpecWithAccount_PrintsTheAccountsFolder(leeway_book, capsys):
    assert cli.main(["spec", "--json", "--account", "work"]) == 0
    assert json.loads(capsys.readouterr().out)["env"]["CLAUDE_CONFIG_DIR"] == str(leeway_book["work"])


def test_MasterCli_AccountBeforeTheVerb_IsTakenToo(leeway_book, capsys):
    assert cli.main(["--account", "work", "spec", "--json"]) == 0
    assert json.loads(capsys.readouterr().out)["account"] == "work"


def test_MasterCli_UnknownAccount_IsAUsageErrorAndWritesNothing(leeway_book, capsys):
    assert cli.main(["spec", "--json", "--account", "nobody"]) == 2
    assert "nobody" in capsys.readouterr().out
