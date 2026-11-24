"""The agent of one automation: its property list per schedule kind, its label, and a faked install."""

import plistlib
from pathlib import Path

from sakura.automations import launchd, registry
from sakura.digest import launchd as digest_launchd

HOME = Path("/home/someone")
BIN = Path("/opt/sakura/bin/sakura")


class FakeLaunchctl:
    def __init__(self):
        self.calls = []

    def __call__(self, arguments):
        self.calls.append(arguments)
        return 0


def automation(schedule="daily 07:30", name="wake-lanes", verb="flow wake"):
    return registry.Automation(name, "Wake lanes", schedule, verb)


def test_Label_IsPrefixedByTheName():
    assert launchd.label_of("wake-lanes") == "local.sakura.auto.wake-lanes"
    assert launchd.label_of("morning-digest") == "local.sakura.digest"  # the digest's own agent, which stays as it was


def test_Plist_Daily_UsesACalendarTimeAndRunsTheAutomationByName():
    agent = launchd.plist(automation(), HOME, BIN)
    assert agent["Label"] == "local.sakura.auto.wake-lanes"
    assert agent["ProgramArguments"] == ["/opt/sakura/bin/sakura", "automations", "run", "wake-lanes"]
    assert agent["StartCalendarInterval"] == {"Hour": 7, "Minute": 30} and "StartInterval" not in agent
    assert not [word for word in agent["ProgramArguments"] if word in ("sh", "-c", "/bin/sh", "bash")]
    assert plistlib.loads(plistlib.dumps(agent)) == agent


def test_Plist_EveryNMinutes_UsesAnInterval():
    agent = launchd.plist(automation("every 15 minutes"), HOME, BIN)
    assert agent["StartInterval"] == 900 and "StartCalendarInterval" not in agent and "KeepAlive" not in agent


def test_Plist_MorningDigest_IsTheDigestsOwnAgent(tmp_path):
    agent = launchd.plist(automation("daily 07:30", "morning-digest", "digest run --scheduled"), HOME, BIN)
    assert agent == digest_launchd.plist(digest_launchd.config.parse({}), HOME, BIN)


def test_Install_Uninstall_WriteIntoTheGivenFolderOnly(tmp_path):
    fake = FakeLaunchctl()
    where = digest_launchd.Agent(launchctl=fake, folder=tmp_path / "agents", home=HOME)
    path = launchd.install(automation(), where)
    assert path == tmp_path / "agents" / "local.sakura.auto.wake-lanes.plist" and path.is_file()
    assert [call[0] for call in fake.calls] == ["bootout", "bootstrap"]
    assert launchd.uninstall("wake-lanes", where) is True and not path.exists()
