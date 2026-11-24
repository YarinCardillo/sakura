"""The launchd agent: its property list, and an install that is faked end to end (nothing touches the real agents)."""

import io
import plistlib
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.digest import cli, config, launchd

HOME = Path("/home/someone")
SETTINGS = config.DigestConfig("07:30", "sonnet", "en", 24, ())


class FakeLaunchctl:
    def __init__(self, bootstrap_status: int = 0):
        self.calls, self.bootstrap_status = [], bootstrap_status

    def __call__(self, arguments: list[str]) -> int:
        self.calls.append(arguments)
        return self.bootstrap_status if arguments[0] == "bootstrap" else 0


def test_Plist_CalendarAndRunAtLoad():
    agent = launchd.plist(SETTINGS, HOME, Path("/opt/sakura/bin/sakura"))
    assert agent["Label"] == "local.sakura.digest"
    assert agent["StartCalendarInterval"] == {"Hour": 7, "Minute": 30} and agent["RunAtLoad"] is True
    assert agent["ProgramArguments"] == ["/opt/sakura/bin/sakura", "digest", "run", "--scheduled"]
    assert agent["EnvironmentVariables"]["PATH"].startswith("/home/someone/.local/bin:/opt/homebrew/bin")
    assert agent["StandardOutPath"] == agent["StandardErrorPath"] == str(config.digest_folder() / "launchd.log")
    assert plistlib.loads(plistlib.dumps(agent)) == agent


def test_Plist_NoWakeKeys():
    agent = launchd.plist(SETTINGS, HOME)
    assert set(agent) == {"Label", "ProgramArguments", "StartCalendarInterval", "RunAtLoad", "EnvironmentVariables",
                          "StandardOutPath", "StandardErrorPath"}
    assert not [word for word in agent["ProgramArguments"] if "caffeinate" in word or "pmset" in word]
    assert "KeepAlive" not in agent and "WakeSystem" not in agent and "StartInterval" not in agent


def test_Plist_EngineHomeOverride_PassedOn(monkeypatch):
    assert "SAKURA_ENGINE_HOME" in launchd.plist(SETTINGS, HOME)["EnvironmentVariables"]  # the tests run under one
    monkeypatch.delenv("SAKURA_ENGINE_HOME")
    assert "SAKURA_ENGINE_HOME" not in launchd.plist(SETTINGS, HOME)["EnvironmentVariables"]


def test_Install_Uninstall_Idempotent(tmp_path):
    folder, fake = tmp_path / "LaunchAgents", FakeLaunchctl()
    first = launchd.install(SETTINGS, launchctl=fake, agents_dir=folder, home=HOME)
    content = first.read_bytes()
    assert first == folder / "local.sakura.digest.plist" and plistlib.loads(content)["RunAtLoad"] is True
    assert first.stat().st_mode & 0o777 == 0o644
    assert launchd.install(SETTINGS, launchctl=fake, agents_dir=folder, home=HOME) == first and first.read_bytes() == content
    assert [call[0] for call in fake.calls] == ["bootout", "bootstrap", "bootout", "bootstrap"]
    assert fake.calls[1][1].startswith("gui/") and fake.calls[1][2] == str(first)
    assert sorted(path.name for path in folder.iterdir()) == ["local.sakura.digest.plist"]  # no part files left
    assert launchd.uninstall(launchctl=fake, agents_dir=folder) is True
    assert launchd.uninstall(launchctl=fake, agents_dir=folder) is False
    assert not first.exists()


def test_Install_ChangedTime_ReplacesTheFile(tmp_path):
    folder = tmp_path / "LaunchAgents"
    launchd.install(SETTINGS, launchctl=FakeLaunchctl(), agents_dir=folder, home=HOME)
    later = config.DigestConfig("08:15", "sonnet", "en", 24, ())
    path = launchd.install(later, launchctl=FakeLaunchctl(), agents_dir=folder, home=HOME)
    assert plistlib.loads(path.read_bytes())["StartCalendarInterval"] == {"Hour": 8, "Minute": 15}


def test_Install_LaunchctlRefuses_ReportedWithTheFileInPlace(tmp_path):
    with pytest.raises(launchd.LaunchdError, match="could not load"):
        launchd.install(SETTINGS, launchctl=FakeLaunchctl(bootstrap_status=5), agents_dir=tmp_path, home=HOME)
    assert (tmp_path / "local.sakura.digest.plist").is_file()


def test_Cli_InstallAndUninstall_FakedAndNeverTheRealFolder(tmp_path):
    path = engine_home.root() / config.CONFIG_FILE
    path.write_text('at = "06:45"\n[[source]]\nid = "work"\nrun = ["/opt/work.sh"]\n')
    path.chmod(0o600)
    fake, out = FakeLaunchctl(), io.StringIO()
    agent = launchd.Agent(fake, tmp_path / "agents", HOME)
    assert cli.main(["install"], env={}, stdout=out, agent=agent) == 0 and "every day at 06:45" in out.getvalue()
    assert cli.main(["uninstall"], env={}, stdout=out, agent=agent) == 0 and "agent removed" in out.getvalue()
    assert cli.main(["uninstall"], env={}, stdout=out, agent=agent) == 0 and "no agent was installed" in out.getvalue()
    assert cli.main(["install"], env={"SAKURA_FLOW_ROLE": "builder"}, stdout=out, agent=agent) == 2
    assert [call[0] for call in fake.calls] == ["bootout", "bootstrap", "bootout", "bootout"]
