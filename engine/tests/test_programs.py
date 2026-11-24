"""Programs the engine starts itself: pinned to a real path outside every repository, with a purpose-made environment."""

import os
import stat
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from conftest import git
from flow_support import make_context
from sakura import programs
from sakura.flow import phases, process, tmux
from sakura.flow.phases import cleanup, pull_request


@pytest.fixture(autouse=True)
def fresh_pins(monkeypatch):
    monkeypatch.setattr(programs, "_resolved", {})


def _tool(folder: Path, name: str, body: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    tool = folder / name
    tool.write_text(f"#!/bin/sh\n{body}\n")
    tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
    return tool


def _path(monkeypatch, *entries):
    monkeypatch.setenv("PATH", os.pathsep.join(str(entry) for entry in entries))


def test_Path_ResolvesAnAbsoluteRealPath(tmp_path, monkeypatch):
    good = _tool(tmp_path / "host", "gh", "exit 0")
    _path(monkeypatch, tmp_path / "host")

    assert programs.path("gh") == os.path.realpath(good)


@pytest.mark.parametrize("entry", ["", "relative/bin", "."])
def test_Path_StripsRelativeAndEmptyEntries(tmp_path, monkeypatch, entry):
    _tool(tmp_path / "relative" / "bin", "gh", "exit 0")
    good = _tool(tmp_path / "host", "gh", "exit 0")
    monkeypatch.chdir(tmp_path)
    _path(monkeypatch, entry, tmp_path / "host")

    assert programs.path("gh") == os.path.realpath(good)


def test_Path_AGhInAWorkspaceVirtualEnvironmentOnPathIsSkipped(repo, tmp_path, monkeypatch):
    _tool(repo / ".venv" / "bin", "gh", "exit 0")
    good = _tool(tmp_path / "host", "gh", "exit 0")
    _path(monkeypatch, repo / ".venv" / "bin", tmp_path / "host")

    assert programs.path("gh", repo) == os.path.realpath(good)


def test_Path_WhenEveryCopyIsRepositoryControlledItRefusesAndSaysWhere(repo, tmp_path, monkeypatch):
    _tool(repo / ".venv" / "bin", "gh", "exit 0")
    _path(monkeypatch, repo / ".venv" / "bin")

    with pytest.raises(programs.ProgramError, match="no gh on PATH may be run outside the cage.*lies under"):
        programs.path("gh", repo)


def test_Path_WhenNoneExistsItSaysSo(tmp_path, monkeypatch):
    _path(monkeypatch, tmp_path)

    with pytest.raises(programs.ProgramError, match="no gh on PATH"):
        programs.path("gh")


def test_Environment_PassesOnlyWhatThePurposeNeedsAndFixesPath(tmp_path):
    base = {"HOME": "/home/x", "GH_TOKEN": "t", "AWS_SECRET_ACCESS_KEY": "s", "PATH": "/workspace/.venv/bin",
            "SAKURA_ENGINE_HOME": "/e", "GIT_DIR": "/elsewhere"}

    env = programs.environment("gh", str(tmp_path / "bin" / "gh"), base)

    assert env["GH_TOKEN"] == "t" and env["HOME"] == "/home/x" and env["GH_PROMPT_DISABLED"] == "1"
    assert not {"AWS_SECRET_ACCESS_KEY", "SAKURA_ENGINE_HOME", "GIT_DIR"} & set(env)
    assert env["PATH"].split(os.pathsep)[0] == str(tmp_path / "bin") and "/workspace" not in env["PATH"]


def test_Environment_ASystemToolGetsNoTokenAtAll():
    env = programs.environment("system", "/bin/ps", {"HOME": "/h", "GH_TOKEN": "t", "TERM": "xterm"})

    assert env == {"HOME": "/h", "PATH": "/bin:/usr/bin:/usr/sbin:/sbin"}


# -- the call sites ----------------------------------------------------------------------------------------------


def _plant(repo, tmp_path, monkeypatch, name, marker):
    _tool(repo / ".venv" / "bin", name, f"touch {marker}; echo '[]'")
    _tool(tmp_path / "host", name, "echo '[]'")
    _path(monkeypatch, repo / ".venv" / "bin", tmp_path / "host", "/usr/bin", "/bin")


def test_PullRequestLookup_DoesNotRunAGhPlantedInTheWorkspace(repo, tmp_path, monkeypatch):
    marker = tmp_path / "planted-ran"
    _plant(repo, tmp_path, monkeypatch, "gh", marker)
    ctx = make_context(repo)
    state = {"branch": "feat/demo", "base_branch": "main"}

    outcome = pull_request._existing_draft(ctx, state, "abc")

    assert outcome is None and not marker.exists()


def test_PullRequestLookup_WithOnlyAPlantedGhItBlocksInsteadOfRunningIt(repo, tmp_path, monkeypatch):
    marker = tmp_path / "planted-ran"
    _tool(repo / ".venv" / "bin", "gh", f"touch {marker}")
    _path(monkeypatch, repo / ".venv" / "bin", "/usr/bin", "/bin")
    ctx = make_context(repo)

    outcome = pull_request._existing_draft(ctx, {"branch": "b", "base_branch": "main"}, "abc")

    assert outcome.kind == "blocked" and "no gh on PATH may be run outside the cage" in outcome.reason and not marker.exists()


def test_CleanupLookup_DoesNotRunAGhPlantedInTheWorkspace(repo, tmp_path, monkeypatch):
    marker = tmp_path / "planted-ran"
    _tool(repo / ".venv" / "bin", "gh", f"touch {marker}; echo '{{\"state\": \"MERGED\"}}'")
    _tool(tmp_path / "host", "gh", "echo '{\"state\": \"OPEN\"}'")
    _path(monkeypatch, repo / ".venv" / "bin", tmp_path / "host", "/usr/bin", "/bin")

    assert cleanup._pr_state("https://example.test/pull/1", repo) == "OPEN" and not marker.exists()


def test_Caffeinate_IsResolvedWithoutRelativeEntries(tmp_path, monkeypatch):
    marker = tmp_path / "planted-ran"
    _tool(tmp_path / "cwd", "caffeinate", f"touch {marker}")
    monkeypatch.chdir(tmp_path / "cwd")
    _path(monkeypatch, ".", "/usr/bin")

    with process.hold_awake(os.getpid()):
        pass

    assert not marker.exists()


def test_ProcessTable_RunsThePinnedPsWithAnEmptyEnvironment(tmp_path, monkeypatch):
    seen = {}

    def fake_run(argv, **kwargs):
        seen.update(argv=argv, env=kwargs.get("env"))
        return SimpleNamespace(stdout="1 0 launchd\n")

    monkeypatch.setattr(process.subprocess, "run", fake_run)
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", "s")

    assert process._process_table() == {1: (0, "launchd")}
    assert os.path.isabs(seen["argv"][0]) and "AWS_SECRET_ACCESS_KEY" not in seen["env"]


def test_Tmux_TheDefaultRunnerPinsTmuxAndScrubsItsEnvironment(repo, tmp_path, monkeypatch):
    marker = tmp_path / "planted-ran"
    _tool(repo / ".venv" / "bin", "tmux", f"touch {marker}")
    real = _tool(tmp_path / "host", "tmux", "echo hi")
    _path(monkeypatch, repo / ".venv" / "bin", tmp_path / "host")
    monkeypatch.chdir(repo)
    monkeypatch.setenv("AWS_SECRET_ACCESS_KEY", "s")
    seen = {}

    def fake_run(argv, **kwargs):
        seen.update(argv=argv, env=kwargs["env"])
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(tmux.subprocess, "run", fake_run)
    tmux.TmuxClient()._call(["list-sessions"])

    assert seen["argv"][0] == os.path.realpath(real) and not marker.exists()
    assert "AWS_SECRET_ACCESS_KEY" not in seen["env"]


# -- the agent CLIs do not depend on the caller's PATH ---------------------------------------------------------------------


@pytest.fixture
def install_places(tmp_path, monkeypatch):
    """The usual install places, replaced by a folder of the test, and no host configuration."""
    folder = tmp_path / "install"
    monkeypatch.setattr(programs, "INSTALL_FOLDERS", (str(folder),))
    monkeypatch.delenv("CLAUDE_CODE_EXECUTABLE", raising=False)
    return folder


def test_AgentCli_MinimalPath_IsFoundInAnInstallPlaceAndRecorded(tmp_path, monkeypatch, install_places):
    claude = _tool(install_places, "claude", "exit 0")
    _path(monkeypatch, "/usr/bin", "/bin")

    assert programs.resolution("claude", tmp_path) == (os.path.realpath(claude), programs.SOURCE_INSTALL)


def test_AgentCli_OnPath_StaysTheChoiceOverAnInstallPlace(tmp_path, monkeypatch, install_places):
    _tool(install_places, "claude", "exit 0")
    onpath = _tool(tmp_path / "host", "claude", "exit 0")
    _path(monkeypatch, tmp_path / "host", "/usr/bin")

    assert programs.resolution("claude", tmp_path) == (os.path.realpath(onpath), programs.SOURCE_PATH)


def test_AgentCli_HostConfiguration_BeatsPathAndInstallPlaces(tmp_path, monkeypatch, install_places):
    _tool(install_places, "codex", "exit 0")
    _tool(tmp_path / "host", "codex", "exit 0")
    chosen = _tool(tmp_path / "chosen", "codex", "exit 0")
    _path(monkeypatch, tmp_path / "host")
    from sakura import engine_home

    (engine_home.root() / programs.HOST_FILE).write_text(f'codex = "{chosen}"\n')

    assert programs.resolution("codex", tmp_path) == (os.path.realpath(chosen), programs.SOURCE_HOST)


def test_AgentCli_ClaudeExecutableVariable_IsHostConfiguration(tmp_path, monkeypatch, install_places):
    chosen = _tool(tmp_path / "chosen", "claude-2", "exit 0")
    monkeypatch.setenv("CLAUDE_CODE_EXECUTABLE", str(chosen))
    _path(monkeypatch, "/usr/bin")

    assert programs.resolution("claude", tmp_path) == (os.path.realpath(chosen), programs.SOURCE_HOST)


def test_AgentCli_HostConfigurationThatIsNotUsable_IsRefusedNotSkipped(tmp_path, monkeypatch, install_places):
    _tool(install_places, "claude", "exit 0")
    monkeypatch.setenv("CLAUDE_CODE_EXECUTABLE", str(tmp_path / "missing"))

    with pytest.raises(programs.ProgramError, match="CLAUDE_CODE_EXECUTABLE"):
        programs.path("claude", tmp_path)


# -- the bundled tmux: the app names its own copy in SAKURA_TMUX ----------------------------------------------------------


def test_Tmux_SakuraTmuxVariable_BeatsPathAndIsHostConfiguration(tmp_path, monkeypatch, install_places):
    _tool(tmp_path / "host", "tmux", "exit 0")
    bundled = _tool(tmp_path / "bundle", "tmux", "exit 0")
    _path(monkeypatch, tmp_path / "host")
    monkeypatch.setenv("SAKURA_TMUX", str(bundled))

    assert programs.resolution("tmux", tmp_path) == (os.path.realpath(bundled), programs.SOURCE_HOST)


def test_Tmux_SakuraTmuxUnset_UsesPath(tmp_path, monkeypatch, install_places):
    onpath = _tool(tmp_path / "host", "tmux", "exit 0")
    _path(monkeypatch, tmp_path / "host")
    monkeypatch.delenv("SAKURA_TMUX", raising=False)

    assert programs.resolution("tmux", tmp_path) == (os.path.realpath(onpath), programs.SOURCE_PATH)


def test_Tmux_SakuraTmuxThatIsNotUsable_IsRefusedNotSkipped(tmp_path, monkeypatch, install_places):
    _tool(tmp_path / "host", "tmux", "exit 0")
    _path(monkeypatch, tmp_path / "host")
    monkeypatch.setenv("SAKURA_TMUX", str(tmp_path / "missing"))

    with pytest.raises(programs.ProgramError, match="SAKURA_TMUX"):
        programs.path("tmux", tmp_path)


def test_Tmux_NothingOnPathAndNoSakuraTmux_StillSaysNoTmuxOnPath(tmp_path, monkeypatch, install_places):
    _path(monkeypatch, tmp_path)
    monkeypatch.delenv("SAKURA_TMUX", raising=False)

    with pytest.raises(programs.ProgramError, match="no tmux on PATH"):
        programs.path("tmux", tmp_path)


@pytest.mark.parametrize("mode", [0o777, 0o757, 0o775])
def test_AgentCli_InstallPlaceFileOthersCanWrite_IsNotTheirOwn(tmp_path, monkeypatch, install_places, mode):
    claude = _tool(install_places, "claude", "exit 0")
    claude.chmod(mode)
    _path(monkeypatch, "/usr/bin", "/bin")

    with pytest.raises(programs.ProgramError, match="claude"):
        programs.path("claude", tmp_path)


def test_AgentCli_InstallPlaceInsideTheRepository_IsNotUsed(repo, monkeypatch):
    _tool(repo / "bin", "claude", "exit 0")
    monkeypatch.setattr(programs, "INSTALL_FOLDERS", (str(repo / "bin"),))
    monkeypatch.delenv("CLAUDE_CODE_EXECUTABLE", raising=False)
    _path(monkeypatch, "/usr/bin", "/bin")

    with pytest.raises(programs.ProgramError):
        programs.path("claude", repo)


def test_AgentCli_NotAnAgentTool_NeverFallsBackToInstallPlaces(tmp_path, monkeypatch, install_places):
    _tool(install_places, "gh", "exit 0")
    _path(monkeypatch, "/usr/bin")

    with pytest.raises(programs.ProgramError, match="no gh on PATH"):
        programs.path("gh", tmp_path)


def test_Role_MinimalPath_StillStartsAClaudeRoleFromAnInstallPlaceAndTheLedgerNamesIt(repo, tmp_path, monkeypatch, install_places):
    from flow_support import MonitorLauncher, task_workspace
    from sakura.flow import runner

    claude = _tool(install_places, "claude", "exit 0")
    _path(monkeypatch, "/usr/bin", "/bin")
    ctx = make_context(repo, overrides={"dial": "save", "roles": {"planner": "claude:haiku"}})
    captured = {}

    def monitor(argv, **kwargs):
        captured["argv"] = argv
        return process.ProcessOutcome(0, False, False, False, 1)

    request = runner.RunRequest("planner", "intake", "prompt", {}, task_workspace(ctx), {})
    runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)

    assert captured["argv"][0] == os.path.realpath(claude)
    assert f"claude program {os.path.realpath(claude)} (found in {programs.SOURCE_INSTALL})" in ctx.store.ledger.read_text()


# -- one trust check for every program run outside the cage (review 25, findings 1 and 2) ---------------------------------------


@pytest.fixture(autouse=True)
def a_stranger_is_in_every_group(monkeypatch):
    """Group write is refused when another account is in the group; the tests of that rule say otherwise where they need it."""
    monkeypatch.setattr(programs, "_group_members", lambda gid: {"root", programs.pwd.getpwuid(os.getuid()).pw_name, "stranger"})


def _trusted_tool(tmp_path, name="tool"):
    return _tool(tmp_path / "host", name, "exit 0")


def test_Untrusted_AnOrdinaryOwnedFileInAnOwnedFolder_IsTrusted(tmp_path):
    tool = _trusted_tool(tmp_path)
    assert programs.untrusted(str(tool)) == "" and programs.trusted(str(tool)) == os.path.realpath(tool)


@pytest.mark.parametrize("mode", [0o775, 0o757, 0o777])
def test_Untrusted_AFolderAboveTheFileThatGroupOrOthersCanWrite_IsRefused(tmp_path, mode):
    tool = _trusted_tool(tmp_path)
    (tmp_path / "host").chmod(mode)
    assert "can be written by its group or by others" in programs.untrusted(str(tool))


def test_Untrusted_AnAncestorFurtherUp_IsCheckedToo(tmp_path):
    tool = _tool(tmp_path / "a" / "b" / "c", "tool", "exit 0")
    (tmp_path / "a").chmod(0o770)
    assert "a can be written" in programs.untrusted(str(tool)).replace(str(tmp_path) + "/", "")


@pytest.mark.parametrize("mode", [0o775, 0o757, 0o777])
def test_Untrusted_AFileThatGroupOrOthersCanWrite_IsRefused(tmp_path, mode):
    tool = _trusted_tool(tmp_path)
    tool.chmod(mode)
    assert "can be written" in programs.untrusted(str(tool))


def test_Untrusted_ALinkIsJudgedByItsCanonicalTarget(tmp_path):
    target = _tool(tmp_path / "open", "tool", "exit 0")
    (tmp_path / "open").chmod(0o777)
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin" / "tool").symlink_to(target)
    assert "can be written" in programs.untrusted(str(tmp_path / "bin" / "tool"))


def test_Untrusted_AFileOfAnotherAccount_IsRefused(tmp_path, monkeypatch):
    tool = _trusted_tool(tmp_path)
    monkeypatch.setattr(programs.os, "getuid", lambda: 54321)
    assert "belongs to another account" in programs.untrusted(str(tool))


def test_Untrusted_NotAFileOrMissing_IsRefused(tmp_path):
    assert "does not exist" in programs.untrusted(str(tmp_path / "nothing"))
    assert "not a regular file" in programs.untrusted(str(tmp_path))


def test_Untrusted_UnderARegisteredRepositoryOtherThanTheCurrentOne_IsRefused(repo, tmp_path):
    from sakura.flow import registry

    other = tmp_path / "other-project"
    other.mkdir()
    git(tmp_path, "init", "-q", "-b", "main", str(other))
    registry.remember(other)
    tool = _tool(other / "node_modules" / ".bin", "tool", "exit 0")
    assert "where a repository or an agent can write" in programs.untrusted(str(tool), [])


def test_Untrusted_UnderTheEngineHomeTaskFolders_IsRefused(tmp_path):
    from sakura import engine_home

    tool = _tool(engine_home.root() / "repo-x" / "flow" / "demo", "tool", "exit 0")
    assert "where a repository or an agent can write" in programs.untrusted(str(tool))


def test_Untrusted_UnderTheSystemTmpFolders_IsRefusedUnlessTheTestsSwitchItOff(tmp_path, monkeypatch):
    tool = _trusted_tool(tmp_path)
    monkeypatch.setattr(programs, "FORBID_SYSTEM_TMP", True)
    assert "where a repository or an agent can write" in programs.untrusted(str(tool))
    monkeypatch.setattr(programs, "FORBID_SYSTEM_TMP", False)
    assert programs.untrusted(str(tool)) == ""


def test_Path_AGroupWritableCopyIsSkippedForALaterTrustedOne(tmp_path, monkeypatch):
    bad = _tool(tmp_path / "bad", "gh", "exit 0")
    (tmp_path / "bad").chmod(0o775)
    good = _tool(tmp_path / "good", "gh", "exit 0")
    _path(monkeypatch, tmp_path / "bad", tmp_path / "good")
    assert programs.path("gh") == os.path.realpath(good) and bad.exists()


def test_Path_OnlyAGroupWritableCopy_RefusesSayingWhy(tmp_path, monkeypatch):
    _tool(tmp_path / "bad", "node", "exit 0")
    (tmp_path / "bad").chmod(0o775)
    _path(monkeypatch, tmp_path / "bad")
    with pytest.raises(programs.ProgramError, match="node on PATH may be run outside the cage.*written by its group"):
        programs.path("node")


def test_InstallPlace_AGroupWritableInstallFolder_IsNotUsed(tmp_path, monkeypatch, install_places):
    _tool(install_places, "claude", "exit 0")
    install_places.chmod(0o775)
    _path(monkeypatch, "/usr/bin", "/bin")
    with pytest.raises(programs.ProgramError):
        programs.path("claude", tmp_path)


def test_HostFile_ANamedProgramThroughALinkIntoAWritableFolder_IsRefused(tmp_path, monkeypatch, install_places):
    from sakura import engine_home

    target = _tool(tmp_path / "open", "claude", "exit 0")
    (tmp_path / "open").chmod(0o777)
    link = tmp_path / "bin" / "claude"
    link.parent.mkdir()
    link.symlink_to(target)
    (engine_home.root() / programs.HOST_FILE).write_text(f'claude = "{link}"\n')
    with pytest.raises(programs.ProgramError, match="programs.toml names"):
        programs.path("claude", tmp_path)


def test_GitProgram_AGroupWritableGitOnPath_IsNotUsed(tmp_path, monkeypatch):
    from sakura import git_program

    monkeypatch.setattr(git_program, "_resolved", {})
    _tool(tmp_path / "bad", "git", "exit 0")
    (tmp_path / "bad").chmod(0o775)
    _path(monkeypatch, tmp_path / "bad")
    with pytest.raises(git_program.GitProgramError, match="may be run outside the cage"):
        git_program.path()


def _as_owned_by(monkeypatch, folder, uid):
    real_stat = os.stat

    def fake(path, *args, **kwargs):
        info = real_stat(path, *args, **kwargs)
        if os.path.realpath(path) != os.path.realpath(folder):
            return info
        fields = list(info)
        fields[stat.ST_UID] = uid
        return os.stat_result(fields)

    monkeypatch.setattr(programs.os, "stat", fake)


def test_Untrusted_AnAncestorFolderOwnedByAnotherUid_IsRefusedWhateverItsMode(tmp_path, monkeypatch):
    tool = _tool(tmp_path / "owned" / "bin", "tool", "exit 0")
    _as_owned_by(monkeypatch, tmp_path / "owned", 54321)
    assert "owned" in programs.untrusted(str(tool)) and "belongs to another account" in programs.untrusted(str(tool))


def test_Untrusted_AnAncestorOwnedByRoot_IsAccepted(tmp_path, monkeypatch):
    tool = _tool(tmp_path / "owned" / "bin", "tool", "exit 0")
    _as_owned_by(monkeypatch, tmp_path / "owned", 0)
    assert programs.untrusted(str(tool)) == ""


def test_Untrusted_AStickyFolderIsAcceptedOnlyWhenTheEntryBelowItIsTheUsers(tmp_path, monkeypatch):
    tool = _tool(tmp_path / "shared" / "mine", "tool", "exit 0")
    (tmp_path / "shared").chmod(0o1777)
    assert programs.untrusted(str(tool)) == ""
    _as_owned_by(monkeypatch, tmp_path / "shared" / "mine", 0)
    assert "can be written by its group or by others" in programs.untrusted(str(tool))
    _as_owned_by(monkeypatch, tmp_path / "shared" / "mine", 54321)
    assert "belongs to another account" in programs.untrusted(str(tool))


def test_Untrusted_AWorldWritableFolderWithoutTheStickyBit_IsRefusedEvenWhenTheEntryIsTheUsers(tmp_path):
    tool = _tool(tmp_path / "shared" / "mine", "tool", "exit 0")
    (tmp_path / "shared").chmod(0o777)
    assert "can be written by its group or by others" in programs.untrusted(str(tool))


# -- review 25 follow-ups: no planted Git, fail closed, identities not spellings, the master's repository -----------------------


def test_Untrusted_AGitPlantedEarlyOnPath_IsNeverRunWhileTheRootsAreListed(repo, tmp_path, monkeypatch):
    from sakura import git_program
    from sakura.flow import registry

    marker = tmp_path / "planted-git-ran"
    planted = tmp_path / "planted" / "git"
    planted.parent.mkdir()
    planted.write_text(f"#!/bin/sh\ntouch {marker}\nexec /usr/bin/git \"$@\"\n")
    planted.chmod(0o755)
    registry.remember(repo)
    tool = _trusted_tool(tmp_path)
    monkeypatch.setattr(git_program, "_resolved", {})
    monkeypatch.setattr(programs, "FORBID_SYSTEM_TMP", True)
    _path(monkeypatch, tmp_path / "planted", "/usr/bin", "/bin")

    programs.untrusted(str(tool))

    assert not marker.exists()


def test_Untrusted_ARegistryThatCannotBeRead_RefusesInsteadOfListingFewerRoots(tmp_path, monkeypatch):
    from sakura.flow import registry

    tool = _trusted_tool(tmp_path)

    def broken(*_args, **_kwargs):
        raise registry.RegistryError("the registry is corrupt")

    monkeypatch.setattr(registry, "repos", broken)
    assert "cannot be listed" in programs.untrusted(str(tool))


def test_Untrusted_ACaseVariantOfAForbiddenRoot_IsRefused(tmp_path):
    from sakura import engine_home

    tool = _tool(engine_home.root() / "repo-x" / "flow" / "demo", "tool", "exit 0")
    spelled = Path(str(tool).replace("engine-home", "ENGINE-HOME"))
    if not spelled.exists():
        pytest.skip("this volume tells apart names that differ in case")
    assert "where a repository or an agent can write" in programs.untrusted(str(spelled))


def test_Untrusted_AFolderReachedThroughAnotherSpellingOfTheSameFolder_IsRefused(repo, tmp_path):
    from sakura.flow import registry

    registry.remember(repo)
    alias = tmp_path / "alias"
    alias.symlink_to(repo)
    tool = _tool(repo / "bin", "tool", "exit 0")
    assert "where a repository or an agent can write" in programs.untrusted(str(alias / "bin" / "tool"))
    assert tool.exists()


@pytest.mark.parametrize("members,accepted", [
    ({"root", "me"}, True), ({"me"}, True), (set(), True), ({"root", "me", "other"}, False), ({"other"}, False)])
def test_Untrusted_AGroupWritableFolderIsAcceptedOnlyWhenNoOneElseIsInTheGroup(tmp_path, monkeypatch, members, accepted):
    tool = _tool(tmp_path / "cellar" / "bin", "node", "exit 0")
    (tmp_path / "cellar").chmod(0o775)
    me = programs.pwd.getpwuid(os.getuid()).pw_name
    monkeypatch.setattr(programs, "_group_members", lambda _gid: {me if name == "me" else name for name in members})
    monkeypatch.setattr(programs, "_has_nested_groups", lambda _name: False)
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_name="admin", gr_mem=[]))
    monkeypatch.setattr(programs, "ADMIN_GROUP_ID", os.stat(tmp_path).st_gid)  # the test folders belong to the person's group
    reason = programs.untrusted(str(tool))
    assert (reason == "") is accepted
    assert accepted or "can be written by its group or by others" in reason


def test_Untrusted_AWorldWritableFolder_IsRefusedWhoeverIsInTheGroup(tmp_path, monkeypatch):
    tool = _tool(tmp_path / "open" / "bin", "node", "exit 0")
    (tmp_path / "open").chmod(0o777)
    monkeypatch.setattr(programs, "_group_members", lambda _gid: set())
    assert "can be written" in programs.untrusted(str(tool))


def test_GroupMembers_AreTheListedMembersAndTheAccountsWhosePrimaryGroupItIs(monkeypatch):
    monkeypatch.undo()
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_mem=["listed"]))
    monkeypatch.setattr(programs.pwd, "getpwall", lambda: [SimpleNamespace(pw_name="primary", pw_gid=7),
                                                          SimpleNamespace(pw_name="elsewhere", pw_gid=8)])
    monkeypatch.setattr(programs.pwd, "getpwnam", lambda name: SimpleNamespace(pw_name=name))
    assert programs._group_members(7) == {"listed", "primary"}
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: (_ for _ in ()).throw(KeyError(7)))
    assert programs._group_members(7) == {"?"}


@pytest.mark.parametrize("gid,name", [(12, "everyone"), (20, "staff"), (61, "localaccounts"), (500, "com.apple.access_ssh"),
                                      (501, "developers"), (80, "wheel"), (81, "admin")])
def test_GroupIsPrivate_EveryGroupButAdminOfGid80_IsNeverPrivateWhateverItLists(monkeypatch, gid, name):
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_name=name, gr_mem=[]))
    monkeypatch.setattr(programs, "_group_members", lambda _gid: {"root"})
    monkeypatch.setattr(programs, "_has_nested_groups", lambda _name: False)
    assert programs._group_is_private(gid) is False


def test_GroupIsPrivate_AnUnknownGid_IsNotPrivate(monkeypatch):
    def unknown(_gid):
        raise KeyError(80)

    monkeypatch.setattr(programs.grp, "getgrgid", unknown)
    assert programs._group_is_private(80) is False


def test_GroupMembers_AListedNameThatIsNoAccount_MakesTheGroupNeverPrivate(monkeypatch):
    monkeypatch.undo()
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_name="admin", gr_mem=["root", "ghost-account"]))
    monkeypatch.setattr(programs.pwd, "getpwall", lambda: [])
    assert programs._group_members(80) == {"?"}


def test_GroupIsPrivate_AnotherAccountWithAdminAsPrimaryGroup_IsNotPrivate(monkeypatch):
    monkeypatch.undo()
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_name="admin", gr_mem=["root"]))
    monkeypatch.setattr(programs.pwd, "getpwall", lambda: [SimpleNamespace(pw_name="other", pw_gid=80)])
    monkeypatch.setattr(programs, "_has_nested_groups", lambda _name: False)
    assert programs._group_is_private(80) is False


def test_GroupIsPrivate_AGroupWithNestedGroups_IsNotPrivate(monkeypatch):
    monkeypatch.setattr(programs.grp, "getgrgid", lambda _gid: SimpleNamespace(gr_name="admin", gr_mem=[]))
    monkeypatch.setattr(programs, "_group_members", lambda _gid: {"root"})
    monkeypatch.setattr(programs, "_has_nested_groups", lambda _name: True)
    assert programs._group_is_private(80) is False


@pytest.mark.parametrize("returncode,output,nested", [
    (0, "No such key: NestedGroups\n", False), (0, "NestedGroups: ABCD-1234\n", True),
    (56, "<dscl_cmd> DS Error: -14009 (eDSUnknownNodeName)", True)])
def test_HasNestedGroups_ReadsTheDirectoryServiceAndRefusesOnAnythingButNoSuchKey(monkeypatch, returncode, output, nested):
    monkeypatch.setattr(programs.sys, "platform", "darwin")
    monkeypatch.setattr(programs.subprocess, "run", lambda argv, **_kw: SimpleNamespace(
        returncode=returncode, stdout=output, stderr="", argv=argv))
    assert programs._has_nested_groups("admin") is nested


def test_HasNestedGroups_AFailedLookup_Refuses(monkeypatch):
    monkeypatch.setattr(programs.sys, "platform", "darwin")

    def broken(*_args, **_kwargs):
        raise OSError("no dscl")

    monkeypatch.setattr(programs.subprocess, "run", broken)
    assert programs._has_nested_groups("admin") is True


def test_GroupIsPrivate_OnThisMacTheAdminGroupHoldsOnlyThePersonAndRoot(monkeypatch):
    import sys as system

    monkeypatch.undo()

    if system.platform != "darwin":
        pytest.skip("macOS directory service")
    assert programs._group_is_private(80) is True


def test_Path_ASystemTool_IsResolvedWithoutReadingAnyFlowConfiguration(monkeypatch):
    from sakura.flow import config as flow_config

    def forbidden(*_args, **_kwargs):
        raise AssertionError("resolving a system tool read the flow configuration")

    monkeypatch.setattr(programs, "_resolved", {})
    monkeypatch.setattr(flow_config, "load", forbidden)
    monkeypatch.setattr(flow_config, "defaults", forbidden)
    assert programs.path("ps") in ("/bin/ps", "/usr/bin/ps")
