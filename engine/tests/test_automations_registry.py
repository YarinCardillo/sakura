"""Automations: allow-listed engine verbs on a schedule, proposed by the master, installed on the person's accept."""

import io
import json
import os
import plistlib
from datetime import datetime, timezone

import pytest

from sakura import keeper
from sakura.automations import cli, registry
from sakura.digest import cli as digest_cli, config, launchd
from sakura.flow import waits

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)


class FakeLaunchctl:
    def __init__(self):
        self.calls = []

    def __call__(self, arguments):
        self.calls.append(arguments)
        return 0


@pytest.fixture
def where(tmp_path):
    return launchd.Agent(launchctl=FakeLaunchctl(), folder=tmp_path / "agents", home=tmp_path)


def proposal(**changes):
    fields = {"name": "wake-lanes", "description": "Wake lanes that wait on the budget", "schedule": "every 15 minutes",
              "verb": "flow wake"}
    return registry.propose(**{**fields, **changes}, author="master:s1", now=NOW)


@pytest.mark.parametrize("verb", ["sh -c 'rm -rf x'", "/bin/ls", "flow wake; reboot", "flow wake && x", "digest run", "FLOW WAKE", ""])
def test_Propose_AVerbThatIsNotOnTheAllowList_IsRefusedWithOneConstantReason(verb):
    with pytest.raises(registry.AutomationRefused) as raised:
        proposal(verb=verb)
    assert str(raised.value) == registry.VERB_REFUSAL
    assert registry.pending() == []


@pytest.mark.parametrize("schedule", ["every 3 minutes", "every 1441 minutes", "daily 25:00", "whenever", "daily 7:30", ""])
def test_Propose_ABadSchedule_IsRefused(schedule):
    with pytest.raises(registry.AutomationRefused, match="schedule"):
        proposal(schedule=schedule)


def test_Propose_BadNameOrOverlongText_IsRefused():
    for fields in ({"name": "Bad Name"}, {"name": "morning-digest"}, {"description": "x" * 400}, {"description": ""}):
        with pytest.raises(registry.AutomationRefused):
            proposal(**fields)


def test_Propose_WritesAPendingDefinitionAndAWaitOfKindAutomation():
    proposal()
    (pending,) = registry.pending()
    assert pending.enabled is False and registry.load() == []
    (wait,) = [found for found in waits.collect([]) if found.kind == "automation"]
    assert wait.kind == waits.AUTOMATION and "wake-lanes" in wait.title and "flow wake" in wait.detail
    assert waits.split_key(json.loads(wait.id)[4]) == ("wake-lanes", pending.digest)


def test_Accept_InstallsTheAgentAndListsItEnabled(where):
    pending = proposal()
    found = registry.accept("wake-lanes", pending.digest, where, NOW)
    assert found.enabled and found.installed
    assert (where.folder / "local.sakura.auto.wake-lanes.plist").is_file()
    (row,) = [row for row in registry.rows() if row["name"] == "wake-lanes"]
    assert row == {"name": "wake-lanes", "description": "Wake lanes that wait on the budget",
                   "schedule_text": "every 15 minutes", "enabled": True, "installed": True, "last_run_at": None,
                   "last_result": None}
    assert registry.pending() == []


def test_Accept_ADigestThatIsNotTheOneShown_IsRefusedAndInstallsNothing(where):
    proposal()
    with pytest.raises(registry.AutomationRefused, match="no longer waiting"):
        registry.accept("wake-lanes", "000000000000", where, NOW)
    assert not where.folder.exists() and len(registry.pending()) == 1


def test_Reject_RemovesThePendingDefinition(where):
    pending = proposal()
    registry.reject("wake-lanes", pending.digest)
    assert registry.pending() == [] and registry.load() == []


def test_Disable_UnloadsTheAgentAndKeepsTheDefinition(where):
    registry.accept("wake-lanes", proposal().digest, where, NOW)
    registry.disable("wake-lanes", where)
    assert not (where.folder / "local.sakura.auto.wake-lanes.plist").exists()
    assert [(a.name, a.enabled, a.installed) for a in registry.load()] == [("wake-lanes", False, False)]
    registry.enable("wake-lanes", where)
    assert (where.folder / "local.sakura.auto.wake-lanes.plist").is_file() and registry.load()[0].enabled


def test_Remove_DeletesTheDefinitionAndTheAgent(where):
    registry.accept("wake-lanes", proposal().digest, where, NOW)
    registry.remove("wake-lanes", where)
    assert registry.load() == [] and not list(where.folder.glob("*.plist"))
    with pytest.raises(registry.AutomationRefused, match="no automation"):
        registry.remove("wake-lanes", where)


def test_MorningDigest_TheOldInstallShowsAsAnAutomation(tmp_path, where):
    path = config.config_path()
    path.write_text('at = "06:45"\n[[source]]\nid = "work"\nrun = ["/opt/work.sh"]\n')
    os.chmod(path, 0o600)
    assert digest_cli.main(["install"], agent=where, stdout=io.StringIO()) == 0
    (row,) = [row for row in registry.rows() if row["name"] == "morning-digest"]
    assert row["schedule_text"] == "daily 06:45" and row["enabled"] and row["installed"]
    assert (where.folder / "local.sakura.digest.plist").is_file()
    registry.disable("morning-digest", where)
    assert not (where.folder / "local.sakura.digest.plist").exists()
    assert [row["enabled"] for row in registry.rows() if row["name"] == "morning-digest"] == [False]
    registry.enable("morning-digest", where)
    assert plistlib.loads((where.folder / "local.sakura.digest.plist").read_bytes())["StartCalendarInterval"] == {
        "Hour": 6, "Minute": 45}


def test_Rows_TheKeeperAndThePhonePageAreListedAlwaysOn():
    keeper.run_once(datetime.now(timezone.utc), keeper.Ports(repos=lambda: [], is_alive=lambda e: False,
                                                              resume=lambda *_: None, lanes=lambda: 3))
    rows = {row["name"]: row for row in registry.rows()}
    assert rows["keeper"]["schedule_text"] == "always on" and rows["keeper"]["enabled"] is True
    assert rows["phone-page"]["schedule_text"] == "always on" and rows["phone-page"]["enabled"] is True


def test_Run_RunsOnlyTheAllowListedArgvAndRecordsTheResult(where):
    registry.accept("wake-lanes", proposal().digest, where, NOW)
    seen = []
    code = registry.run("wake-lanes", NOW, run_verb=lambda argv: seen.append(argv) or 0)
    assert code == 0 and seen == [("flow", "wake")]
    (row,) = [row for row in registry.rows() if row["name"] == "wake-lanes"]
    assert row["last_run_at"] == "2026-11-18T09:00:00Z" and row["last_result"] == "ok"
    assert registry.run("wake-lanes", NOW, run_verb=lambda argv: 3) == 3
    assert [r["last_result"] for r in registry.rows() if r["name"] == "wake-lanes"] == ["failed with exit code 3"]


def test_Run_ADefinitionEditedToAnotherVerb_IsRefusedAndNothingRuns(where):
    registry.accept("wake-lanes", proposal().digest, where, NOW)
    path = registry.folder() / "wake-lanes.toml"
    path.write_text(path.read_text().replace('"flow wake"', '"sh -c reboot"'))
    seen = []
    with pytest.raises(registry.AutomationRefused) as raised:
        registry.run("wake-lanes", NOW, run_verb=lambda argv: seen.append(argv) or 0)
    assert str(raised.value) == registry.VERB_REFUSAL and seen == []


def test_Run_ADisabledAutomation_DoesNothing(where):
    registry.accept("wake-lanes", proposal().digest, where, NOW)
    registry.disable("wake-lanes", where)
    seen = []
    assert registry.run("wake-lanes", NOW, run_verb=lambda argv: seen.append(argv) or 0) == 0 and seen == []


def test_Cli_ListJson_AcceptAndDisable(where):
    pending = proposal()
    out = io.StringIO()
    assert cli.main(["accept", "--actor=cli:owner", "--", "wake-lanes", pending.digest], stdout=out, agent=where) == 0
    assert cli.main(["disable", "--actor=cli:owner", "--", "wake-lanes"], stdout=out, agent=where) == 0
    listed = io.StringIO()
    assert cli.main(["list", "--json"], stdout=listed, agent=where) == 0
    assert {row["name"] for row in json.loads(listed.getvalue())} >= {"wake-lanes", "keeper", "phone-page"}


def test_Cli_Refusal_PrintsTheReasonAndExitsUsage(where, capsys):
    assert cli.main(["accept", "--", "ghost", "0" * 12], stdout=io.StringIO(), agent=where) == 2
    assert "no longer waiting" in capsys.readouterr().err


def test_Cli_AcceptAndReject_AreKeptAsDecidedAsksSoDoneIsNeverMissingOne(where):
    from sakura.cockpit import decided

    first, second = proposal(), proposal(name="other-wake")
    cli.main(["accept", "--actor=cli:owner", "--", "wake-lanes", first.digest], stdout=io.StringIO(), agent=where)
    cli.main(["reject", "--actor=cli:owner", "--", "other-wake", second.digest], stdout=io.StringIO(), agent=where)
    kept = decided.recent(decided.default_state_dir(), now=datetime.now().astimezone())
    assert sorted((entry["kind"], entry["answer"], entry["actor"]) for entry in kept) == [
        ("automation", "accept", "cli:owner"), ("automation", "reject", "cli:owner")]
