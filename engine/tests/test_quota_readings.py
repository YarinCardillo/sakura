import json
from datetime import timedelta
from pathlib import Path

import pytest
from quota_support import NOW, FakeRunner, clock, codexbar_output, event

from sakura.quota import readings
from sakura.quota.targets import Target
from sakura.usage.archive import quota, store

WORK = Target("claude", "work", "Work", Path("/h/w"), True)


@pytest.fixture
def conn(tmp_path):
    return store.open_db(tmp_path / "usage.db")


def collector(conn, run, **kwargs):
    return readings.Collector(conn, run, clock, "claude", **kwargs)


def test_Read_NothingStored_ProbesAndReturnsALiveReading(conn):
    run = FakeRunner({"/h/w": event(seven=0.94)})
    found = collector(conn, run).read(WORK)
    assert (found.seven_day.pct, found.live, found.stale, found.source) == (94.0, True, False, "probe")
    assert run.probed() == ["/h/w"]


def test_Read_FreshSnapshotInArchive_DoesNotProbe(conn):
    quota.record(conn, quota.Snapshot("work", (NOW - timedelta(minutes=5)).isoformat(), "max", None, 5.0,
                                      "2026-11-03T01:00:00Z", 40.0, "2026-11-03T07:00:00Z", 0))
    run = FakeRunner()
    found = collector(conn, run).read(WORK)
    assert (found.seven_day.pct, found.live, found.stale) == (40.0, False, False)
    assert run.calls == []


def test_Read_OldSnapshot_IsRefreshed(conn):
    quota.record(conn, quota.Snapshot("work", (NOW - timedelta(minutes=30)).isoformat(), "max", None, 5.0,
                                      "2026-11-03T01:00:00Z", 40.0, "2026-11-03T07:00:00Z", 0))
    found = collector(conn, FakeRunner({"/h/w": event(seven=0.5)})).read(WORK)
    assert (found.seven_day.pct, found.live) == (50.0, True)


def test_Read_RefreshFlag_ProbesEvenWhenFresh(conn):
    quota.record(conn, quota.Snapshot("work", NOW.isoformat(), "max", None, 5.0, "2026-11-03T01:00:00Z", 40.0,
                                      "2026-11-03T07:00:00Z", 0))
    assert collector(conn, FakeRunner({"/h/w": event(seven=0.5)})).read(WORK, refresh=True).seven_day.pct == 50.0


def test_Read_ProbeFails_ReturnsTheOldSnapshotMarkedStale(conn):
    quota.record(conn, quota.Snapshot("work", (NOW - timedelta(hours=2)).isoformat(), "max", None, 5.0,
                                      "2026-11-03T01:00:00Z", 40.0, "2026-11-03T07:00:00Z", 0))
    found = collector(conn, FakeRunner()).read(WORK)
    assert (found.seven_day.pct, found.live, found.stale) == (40.0, False, True)


def test_Read_ProbeFailsAndNothingStored_ReturnsNone(conn):
    assert collector(conn, FakeRunner()).read(WORK) is None


def test_Read_DefaultCodex_UsesCodexBar(conn, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".codex").mkdir()
    target = Target("codex", "personal", "Personal", tmp_path / ".codex", True)
    found = collector(conn, FakeRunner(codexbar=codexbar_output(week=9))).read(target)
    assert (found.seven_day.pct, found.source, found.snapshot.account) == (9.0, "codexbar", "codex:personal")


def test_Read_NonDefaultCodexDirectory_IsNotAskedThroughCodexBar(conn, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    other = tmp_path / "other"
    other.mkdir()
    run = FakeRunner(codexbar=codexbar_output())
    assert collector(conn, run).read(Target("codex", "second", "Second", other, True)) is None
    assert run.calls == []


def codex_home(tmp_path, week):
    home = tmp_path / ".codex"
    rollout = home / "sessions/2026/11/02/rollout-a.jsonl"
    rollout.parent.mkdir(parents=True)
    limits = {"plan_type": "prolite", "primary": None, "secondary": {
        "used_percent": week, "window_minutes": 10080, "resets_at": 1793689200}}
    rollout.write_text(json.dumps({"timestamp": "2026-11-02T17:55:00Z", "type": "event_msg",
                                   "payload": {"type": "token_count", "rate_limits": limits}}) + "\n")
    return home


def test_Read_Codex_PrefersTheRolloutLogsOverCodexBar(conn, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    run = FakeRunner(codexbar=codexbar_output(week=50))
    target = Target("codex", "personal", "Personal", codex_home(tmp_path, 9.0), True)
    found = collector(conn, run).read(target)
    assert (found.seven_day.pct, found.source, found.stale, found.live) == (9.0, "codex-log", False, True)
    assert run.calls == []


def test_Read_CodexLogsAreOld_IsNotStaleBecauseOfAge(conn, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    target = Target("codex", "personal", "Personal", codex_home(tmp_path, 9.0), True)
    first = collector(conn, FakeRunner()).read(target)
    again = collector(conn, FakeRunner()).read(target)
    assert not first.stale and not again.stale and again.snapshot.ts == "2026-11-02T17:55:00+00:00"
