import json
import subprocess
from pathlib import Path

from sakura.quota import codexbar

SECRET = "someone@example.invalid"


def entry(provider="codex", **usage):
    body = {"loginMethod": "prolite", "accountEmail": SECRET, "identity": SECRET,
            "secondary": {"windowMinutes": 10080, "usedPercent": 9, "resetsAt": "2026-11-04T09:56:53Z"},
            "primary": None, **usage}
    return {"provider": provider, "usage": body}


def test_Parse_WeeklyWindowOnly_FillsSevenDayAndLeavesFiveHourEmpty():
    found = codexbar.parse(json.dumps([entry()]), "codex")
    assert found == codexbar.Usage("prolite", codexbar.EMPTY_WINDOW,
                                   codexbar.Window(9.0, "2026-11-04T09:56:53Z"))


def test_Parse_BothWindows_MapsThemByLength():
    primary = {"windowMinutes": 300, "usedPercent": 5, "resetsAt": "2026-11-03T01:10:00Z"}
    found = codexbar.parse(json.dumps([entry("claude", primary=primary)]), "claude")
    assert found.five_hour == codexbar.Window(5.0, "2026-11-03T01:10:00Z")
    assert found.seven_day.pct == 9.0


def test_Parse_NeverKeepsTheAccountIdentity():
    assert SECRET not in repr(codexbar.parse(json.dumps([entry()]), "codex"))


def test_Parse_OtherProvider_ReturnsNone():
    assert codexbar.parse(json.dumps([entry("claude")]), "codex") is None


def test_Parse_NotJson_ReturnsNone():
    assert codexbar.parse("oops", "codex") is None
    assert codexbar.parse(None, "codex") is None


def test_Read_FailedRun_ReturnsNone():
    run = lambda argv, env, cwd: subprocess.CompletedProcess(argv, 127, "", "")
    assert codexbar.read("codex", "codexbar", run, {}, Path("/")) is None


def test_Read_AsksForJsonOfTheProvider():
    seen = []

    def run(argv, env, cwd):
        seen.append(argv)
        return subprocess.CompletedProcess(argv, 0, json.dumps([entry()]), "")

    assert codexbar.read("codex", "codexbar", run, {}, Path("/")).seven_day.pct == 9.0
    assert seen == [["codexbar", "usage", "--json", "--provider", "codex"]]
