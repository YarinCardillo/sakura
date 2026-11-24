"""The digest's settings written by the app: time, language, model, account, lookback, and each source on or off."""

import io
import os
import plistlib
from datetime import datetime, timezone

import pytest

from sakura import engine_home
from sakura.digest import cli, config, intake, launchd, state

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)
TEXT = 'at = "07:30"\n[[source]]\nid = "work"\nrun = ["/opt/work.sh"]\n[[source]]\nid = "news"\nfeed = "https://example.com/f.xml"\n'


class FakeLaunchctl:
    def __init__(self):
        self.calls = []

    def __call__(self, arguments):
        self.calls.append(arguments)
        return 0


@pytest.fixture
def written():
    path = engine_home.root() / config.CONFIG_FILE
    path.write_text(TEXT)
    os.chmod(path, 0o600)
    return path


def run_cli(tmp_path, *argv):
    agent = launchd.Agent(launchctl=FakeLaunchctl(), folder=tmp_path / "agents", home=tmp_path)
    out = io.StringIO()
    code = cli.main(list(argv), now=lambda: NOW, stdout=out, agent=agent)
    return code, out.getvalue()


def test_Settings_ChangesOnlyWhatWasGivenAndKeepsTheSources(tmp_path, written):
    code, _ = run_cli(tmp_path, "settings", "--at=08:15", "--lookback_hours=48", "--language=it", "--actor=cli:owner", "--")
    loaded = config.load()
    assert code == 0 and (loaded.at, loaded.lookback_hours, loaded.language, loaded.model) == ("08:15", 48, "it", "sonnet")
    assert [source.id for source in loaded.sources] == ["work", "news"]


def test_Settings_ChangedTime_ReplacesTheInstalledAgentSchedule(tmp_path, written):
    run_cli(tmp_path, "install")
    plist_path = tmp_path / "agents" / "local.sakura.digest.plist"
    assert plistlib.loads(plist_path.read_bytes())["StartCalendarInterval"] == {"Hour": 7, "Minute": 30}
    run_cli(tmp_path, "settings", "--at=08:15", "--")
    assert plistlib.loads(plist_path.read_bytes())["StartCalendarInterval"] == {"Hour": 8, "Minute": 15}


def test_Settings_NoAgentInstalled_InstallsNothing(tmp_path, written):
    run_cli(tmp_path, "settings", "--at=08:15", "--")
    assert not (tmp_path / "agents").exists()


def test_Settings_BadValue_IsRefusedAndTheFileIsUntouched(tmp_path, written, capsys):
    code, _ = run_cli(tmp_path, "settings", "--at=25:99", "--")
    assert code == 2 and written.read_text() == TEXT and "at must be" in capsys.readouterr().err


def test_Settings_NothingGiven_IsRefused(tmp_path, written):
    assert run_cli(tmp_path, "settings", "--")[0] == 2


def test_Settings_NoFileYet_StartsFromTheDefaults(tmp_path):
    assert run_cli(tmp_path, "settings", "--at=09:00", "--")[0] == 0
    assert config.load().at == "09:00" and config.load().sources == ()


def test_Source_Off_SkipsItInTheRunAndKeepsItInTheFile(tmp_path, written):
    assert run_cli(tmp_path, "source", "--", "work", "off")[0] == 0
    loaded = config.load()
    assert [source.enabled for source in loaded.sources] == [False, True]
    results = intake.collect(loaded, state.State(), NOW, fetch=lambda url, timeout: b"<rss><channel/></rss>")
    assert [result.report.id for result in results] == ["news"]
    assert run_cli(tmp_path, "source", "--", "work", "on")[0] == 0 and config.load().sources[0].enabled is True


def test_Source_Unknown_IsRefused(tmp_path, written):
    assert run_cli(tmp_path, "source", "--", "ghost", "off")[0] == 2
