from pathlib import Path

import pytest
from quota_support import NOW, FakeRunner, clock, codexbar_output, event

from sakura.quota import readings, status
from sakura.quota.targets import Target
from sakura.usage.archive import store

PERSONAL = Target("claude", "personal", "personal", Path("/h/p"), True)
WORK = Target("claude", "work", "Work", Path("/h/w"), True)
ORCA = Target("claude", "orca-1ace", "orca-1ace", Path("/h/o"), False)


@pytest.fixture
def conn(tmp_path):
    return store.open_db(tmp_path / "usage.db")


def build(conn, targets, run, **kwargs):
    return status.build(targets, readings.Collector(conn, run, clock, "claude"), conn, NOW, **kwargs)


def test_Build_TwoAccounts_ListsPercentagesAndResets(conn):
    run = FakeRunner({"/h/p": event(five=0.1, seven=0.22, week_reset=1793689200 + 400000),
                      "/h/w": event(five=0.05, seven=0.94)})
    document = build(conn, [PERSONAL, WORK], run)
    work = next(a for a in document["accounts"] if a["id"] == "work")
    assert (work["five_hour"]["pct"], work["seven_day"]["pct"]) == (5.0, 94.0)
    assert work["seven_day"]["resets_at"] == "2026-11-03T07:00:00Z"
    assert (work["source"], work["stale"], work["provider"]) == ("probe", False, "claude")
    assert [a["id"] for a in document["accounts"]] == ["personal", "work"]


def test_Build_TwoDirectoriesOfOneAccount_AreOneEntryWithAnAlias(conn):
    run = FakeRunner({"/h/w": event(seven=0.94), "/h/o": event(seven=0.94)})
    document = build(conn, [WORK, ORCA], run)
    assert [a["id"] for a in document["accounts"]] == ["work"]
    assert document["accounts"][0]["aliases"] == ["/h/o"]


def test_Build_DifferentWeeklyResets_AreDifferentAccounts(conn):
    run = FakeRunner({"/h/w": event(seven=0.94), "/h/o": event(seven=0.94, week_reset=1793689200 + 86400)})
    assert len(build(conn, [WORK, ORCA], run)["accounts"]) == 2


def test_Build_ProbeFailsWithNoHistory_StillListsTheAccountAsStale(conn):
    document = build(conn, [WORK], FakeRunner())
    account = document["accounts"][0]
    assert (account["stale"], account["seven_day"]["pct"], account["forecast"]["confidence"]) == (True, None, "none")


def test_Build_LiveReading_IsRecordedAndFeedsHistory(conn):
    document = build(conn, [WORK], FakeRunner({"/h/w": event(seven=0.5)}))
    assert document["history"]["work"] == [[NOW.isoformat(), 50.0]]


def test_Build_CodexAccount_UsesCodexBar(conn, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".codex").mkdir()
    target = Target("codex", "personal", "personal", tmp_path / ".codex", True)
    account = build(conn, [target], FakeRunner(codexbar=codexbar_output(week=9)))["accounts"][0]
    assert (account["provider"], account["source"], account["seven_day"]["pct"]) == ("codex", "codexbar", 9.0)


def test_Build_ExpiredWindow_ShowsNoPercentage(conn):
    from sakura.usage.archive import quota

    quota.record(conn, quota.Snapshot("work", "2026-11-02T20:55:00+00:00", "max", None, 80.0,
                                      "2026-11-02T20:00:00Z", 40.0, "2026-11-03T07:00:00Z", 0))
    account = build(conn, [WORK], FakeRunner())["accounts"][0]
    assert account["five_hour"] == {"pct": None, "resets_at": None}
    assert account["seven_day"]["pct"] == 40.0


def test_Build_NoLeakOfPrivateData(conn):
    text = str(build(conn, [WORK], FakeRunner({"/h/w": event()})))
    assert "@" not in text
