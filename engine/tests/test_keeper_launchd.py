"""The keeper's launchd agent: its property list, and an install that is faked end to end (nothing touches the real agents)."""

import io
import plistlib
from pathlib import Path

import pytest

from sakura import engine_home, keeper_cli, keeper_launchd
from sakura.digest import launchd as digest_launchd

HOME = Path("/home/someone")


class FakeLaunchctl:
    def __init__(self, bootstrap_status: int = 0):
        self.calls, self.bootstrap_status = [], bootstrap_status

    def __call__(self, arguments: list[str]) -> int:
        self.calls.append(arguments)
        return self.bootstrap_status if arguments[0] == "bootstrap" else 0


def test_Plist_RunsAtLoadAndKeepsAliveOnFailureOnly():
    agent = keeper_launchd.plist(HOME, Path("/opt/sakura/bin/sakura"))
    assert agent["Label"] == "local.sakura.keeper"
    assert agent["ProgramArguments"] == ["/opt/sakura/bin/sakura", "keeper", "serve"]
    assert agent["RunAtLoad"] is True and agent["KeepAlive"] == {"SuccessfulExit": False}
    assert agent["EnvironmentVariables"]["PATH"].startswith("/home/someone/.local/bin")
    assert agent["StandardOutPath"] == agent["StandardErrorPath"] == str(engine_home.root() / "keeper.log")
    assert "StartCalendarInterval" not in agent and "WakeSystem" not in agent
    assert plistlib.loads(plistlib.dumps(agent)) == agent


def test_Install_Uninstall_WritesIntoTheGivenFolderOnly(tmp_path):
    folder, fake = tmp_path / "LaunchAgents", FakeLaunchctl()
    path = keeper_launchd.install(launchctl=fake, agents_dir=folder, home=HOME)
    assert path == folder / "local.sakura.keeper.plist" and plistlib.loads(path.read_bytes())["RunAtLoad"] is True
    assert [call[0] for call in fake.calls] == ["bootout", "bootstrap"]
    assert fake.calls[1][2] == str(path)
    assert keeper_launchd.uninstall(launchctl=fake, agents_dir=folder, home=HOME) is True
    assert keeper_launchd.uninstall(launchctl=fake, agents_dir=folder, home=HOME) is False
    assert not path.exists()


def test_Install_LaunchctlRefuses_ReportedWithTheFileInPlace(tmp_path):
    with pytest.raises(digest_launchd.LaunchdError, match="could not load"):
        keeper_launchd.install(launchctl=FakeLaunchctl(bootstrap_status=5), agents_dir=tmp_path, home=HOME)
    assert (tmp_path / "local.sakura.keeper.plist").is_file()


def test_Cli_InstallAndUninstall_AreFakedAndLeaveLanesAlone(tmp_path, repo):
    from sakura.flow import store

    task = store.TaskStore(repo, "a")
    task.create({"task": "a", "status": "running", "workspaces": {}})
    before = task.read_state()
    agent, out = digest_launchd.Agent(launchctl=FakeLaunchctl(), folder=tmp_path / "agents", home=HOME), io.StringIO()
    assert keeper_cli.main(["install"], stdout=out, agent=agent) == 0
    assert (tmp_path / "agents" / "local.sakura.keeper.plist").is_file()
    assert keeper_cli.main(["uninstall"], stdout=out, agent=agent) == 0
    assert not (tmp_path / "agents" / "local.sakura.keeper.plist").exists()
    assert task.read_state() == before


def test_Cli_TakesTheActorAndSeparatorTheCockpitAdds(tmp_path):
    agent, out = digest_launchd.Agent(launchctl=FakeLaunchctl(), folder=tmp_path / "agents", home=HOME), io.StringIO()
    assert keeper_cli.main(["install", "--actor=overlay:owner@example.com:nPHONE", "--"], stdout=out, agent=agent) == 0
    assert keeper_cli.main(["phone-page", "--actor=cli:owner", "--", "off"], stdout=out, agent=agent) == 0
