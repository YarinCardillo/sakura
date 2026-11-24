import json
import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import cli
from sakura.usage.archive import command, settings, store

NOW = datetime(2026, 10, 28, 12, 0, tzinfo=timezone.utc)
PRIVATE = ("someone@example.invalid", "org-id-123")
EVENT = {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "unifiedWindows": {
    "five_hour": {"utilization": 0.1, "resetsAt": 1793211600, "status": "allowed"},
    "seven_day": {"utilization": 0.32, "resetsAt": 1793466000, "status": "allowed"}}}}


def quota_runner(stdout, code=0):
    def run(argv, env, cwd):
        if argv[1:3] == ["auth", "status"]:
            return subprocess.CompletedProcess(argv, 0, json.dumps(
                {"subscriptionType": "max", "email": PRIVATE[0], "orgId": PRIVATE[1]}), "")
        return subprocess.CompletedProcess(argv, code, stdout, "")
    return run


def no_gh(argv):
    return subprocess.CompletedProcess(argv, 4, "", "gh: not logged in\n")


def config_for(tmp_path, plans=(), weights=None):
    root = tmp_path / "claude"
    root.mkdir()
    (tmp_path / "cfg").mkdir()  # a missing config dir is skipped, not probed
    return settings.Settings(
        db=tmp_path / "state" / "usage.db", sources=(settings.Source("claude", "work", root, tmp_path / "cfg"),),
        pr_repos=(), pr_days=7, plans=plans, plan_weights=weights or {})


def archive(config, runner):
    return command.archive(config, no_gh, NOW, runner)


def rows(config, sql):
    conn = store.open_db(config.db)
    try:
        return conn.execute(sql).fetchall()
    finally:
        conn.close()


def test_archive_records_a_quota_snapshot(tmp_path, capsys):
    config = config_for(tmp_path)

    code = archive(config, quota_runner(json.dumps(EVENT) + "\n"))

    out = capsys.readouterr().out.splitlines()
    assert code == 0
    assert out[1] == "quota work: week 32.0% (resets 2026-10-31T17:00:00Z), 5h 10.0%, plan max"
    assert rows(config, "SELECT account, subscription_type, status, seven_day_pct FROM quota_snapshots") == [
        ("work", "max", "allowed", 32.0)]
    assert not any(secret in "".join(out) for secret in PRIVATE)


def test_archive_quota_failure_still_archives(tmp_path, capsys):
    config = config_for(tmp_path)

    code = archive(config, quota_runner("no events here\n"))

    captured = capsys.readouterr()
    assert code == 0
    assert captured.err == "quota work: skipped (no rate limit event)\n"
    assert rows(config, "SELECT COUNT(*) FROM quota_snapshots") == [(0,)]
    assert captured.out.splitlines()[0].startswith("claude work:")


def test_archive_reports_a_probe_that_could_not_run(tmp_path, capsys):
    config = config_for(tmp_path)

    code = archive(config, lambda argv, env, cwd: subprocess.CompletedProcess(argv, 127, "", "stderr text"))

    assert code == 0
    assert capsys.readouterr().err == "quota work: skipped (probe could not run)\n"


def test_archive_probe_storage_failure_skips_the_account_and_still_refreshes_prs(tmp_path, capsys, monkeypatch):
    config = config_for(tmp_path)
    config = settings.Settings(config.db, config.sources, ("acme/acme",), 7)

    def broken(conn, snapshot):
        raise sqlite3.OperationalError("database is locked: secret detail")

    monkeypatch.setattr(command.quota, "record", broken)
    ok_page = subprocess.CompletedProcess([], 0, json.dumps({"data": {"search": {"nodes": [],
                                          "pageInfo": {"hasNextPage": False, "endCursor": None}}}}), "")

    code = command.archive(config, lambda argv: ok_page, NOW, quota_runner(json.dumps(EVENT) + "\n"))

    captured = capsys.readouterr()
    assert code == 0
    assert "quota work: skipped (OperationalError)\n" in captured.err
    assert "secret detail" not in captured.err
    assert "prs acme/acme:" in captured.out + captured.err


def seeded(tmp_path, plans=(), weights=None):
    config = config_for(tmp_path, plans, weights)
    conn = store.open_db(config.db)
    start = NOW - timedelta(hours=3)
    for minutes, pct in ((0, 10.0), (30, 12.0)):
        conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct, five_hour_resets_at,"
                     " seven_day_pct, seven_day_resets_at)"
                     " VALUES ('work', ?, 'max', 'allowed', 5.0, NULL, ?, '2026-10-31T17:00:00Z')",
                     ((start + timedelta(minutes=minutes)).isoformat(), pct))
    conn.execute("INSERT INTO turns VALUES ('claude', 'k1', 'work', 's', NULL, ?, NULL, NULL, 'claude-opus-5-5',"
                 " 0, 0, 0, 200, NULL)", ((start + timedelta(minutes=10)).isoformat(),))
    conn.execute("INSERT INTO turns VALUES ('claude', 'k0', 'work', 's', NULL, ?, NULL, NULL, 'claude-opus-5-5',"
                 " 0, 0, 0, 100, NULL)", ((NOW - timedelta(hours=2)).isoformat(),))
    conn.execute("INSERT INTO limit_events VALUES ('work', 'e1', ?, 'five_hour', NULL)", (NOW.isoformat(),))
    conn.commit()
    conn.close()
    return config


def test_quota_command_prints_snapshot_and_ratios(tmp_path, capsys):
    plans = (settings.Plan("work", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc), True),)
    config = seeded(tmp_path, plans)

    code = command.quota_view(config, False)

    text = capsys.readouterr().out
    assert code == 0
    assert any(line.split() == ["work", "max20x", "2026-01-01", "estimated"] for line in text.splitlines())
    assert any(line.split()[:4] == ["work", "max20x", "opus", "500.0"] for line in text.splitlines())
    assert any(line.split() == ["work", "max20x", "five_hour", "1", "1500", "1500", "1500"]
               for line in text.splitlines())
    assert "12.0" in text and "allowed" in text


def test_quota_command_json_has_the_same_facts(tmp_path, capsys):
    config = seeded(tmp_path, (settings.Plan("work", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc)),))

    command.quota_view(config, True)

    data = json.loads(capsys.readouterr().out)
    assert data["snapshots"][0]["seven_day_pct"] == 12.0
    assert data["plans"] == [{"account": "work", "plan": "max20x", "since": "2026-01-01T00:00:00+00:00",
                              "estimated": False}]
    assert data["ratios"][0]["weighted_per_pct"] == 500.0
    assert (data["ratios"][0]["window"], data["ratios"][0]["spread"]) == ("seven_day", [500.0, 500.0])
    event = data["limit_events"][0]
    assert (event["plan"], event["window"], event["windows"]) == ("max20x", "five_hour", 1)
    assert (event["median"], event["min"], event["max"]) == (1500.0, 1500.0, 1500.0)
    assert event["detail"] == [{"first_hit": NOW.isoformat(), "resets_at": None, "tokens": 1500.0}]


def test_quota_command_on_an_empty_archive(tmp_path, capsys):
    assert command.quota_view(config_for(tmp_path), False) == 0
    assert capsys.readouterr().out


def test_estimate_command(tmp_path, capsys):
    plans = (settings.Plan("work", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc)),)
    config = seeded(tmp_path, plans)

    code = command.estimate(config, "work", {"opus": 1000.0}, False, NOW)

    assert code == 0
    assert capsys.readouterr().out == "about 2.0% of the week (max20x calibration)\n"


def test_estimate_command_mentions_an_estimated_period_and_supports_json(tmp_path, capsys):
    plans = (settings.Plan("work", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc), True),)
    config = seeded(tmp_path, plans)

    command.estimate(config, "work", {"opus": 1000.0}, True, NOW)

    data = json.loads(capsys.readouterr().out)
    assert data["pct"] == 2.0 and "estimated plan period" in data["basis"]


def test_estimate_command_without_calibration_fails_with_a_message(tmp_path, capsys):
    code = command.estimate(config_for(tmp_path), "work", {"opus": 1.0}, False, NOW)

    captured = capsys.readouterr()
    assert code == 1 and captured.out == ""
    assert "no estimate" in captured.err


def test_cli_routes_quota_and_estimate(monkeypatch, tmp_path):
    config = settings.Settings(tmp_path / "u.db", (), (), 60)
    calls = []
    monkeypatch.setattr(settings, "load", lambda: config)
    monkeypatch.setattr(command, "quota_view", lambda cfg, as_json: calls.append(("quota", as_json)) or 0)
    monkeypatch.setattr(command, "estimate",
                        lambda cfg, account, tokens, as_json, now: calls.append((account, tokens, as_json)) or 0)

    assert cli.main(["usage", "quota", "--json"]) == 0
    assert cli.main(["usage", "quota"]) == 0
    assert cli.main(["usage", "estimate", "--account", "work", "--tokens", "opus=1000,sonnet=2.5"]) == 0
    assert calls == [("quota", True), ("quota", False), ("work", {"opus": 1000.0, "sonnet": 2.5}, False)]


def test_cli_rejects_a_malformed_token_spec(capsys):
    with pytest.raises(SystemExit):
        cli.main(["usage", "estimate", "--account", "work", "--tokens", "opus"])
