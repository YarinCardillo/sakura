import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura.usage.archive import quota, settings, store

NOW = datetime(2026, 10, 28, 12, 0, tzinfo=timezone.utc)
CLOCK = lambda: NOW
SOURCE = settings.Source("claude", "work", Path("/h/w/projects"), Path("/h/w"))
PRIVATE = ("someone@example.invalid", "org-id-123", "Some Org Name")
AUTH = {"loggedIn": True, "authMethod": "claude.ai", "subscriptionType": "max",
        "email": PRIVATE[0], "orgId": PRIVATE[1], "orgName": PRIVATE[2]}


def event_line(status="allowed", five=0.1, seven=0.32, overage=False):
    info = {"status": status, "resetsAt": 1793211600, "rateLimitType": "five_hour", "overageStatus": "rejected",
            "isUsingOverage": overage,
            "unifiedWindows": {"five_hour": {"utilization": five, "resetsAt": 1793211600, "status": status},
                               "seven_day": {"utilization": seven, "resetsAt": 1793466000, "status": "allowed"}}}
    return json.dumps({"type": "rate_limit_event", "rate_limit_info": info, "session_id": "s"})


def stream(*lines):
    return "\n".join(['{"type":"system"}', *lines, '{"type":"result","is_error":false}']) + "\n"


class FakeRunner:
    def __init__(self, probe_stdout, probe_code=0, auth=AUTH):
        self.calls, self.probe_stdout, self.probe_code, self.auth = [], probe_stdout, probe_code, auth

    def __call__(self, argv, env, cwd):
        self.calls.append((argv, dict(env), cwd))
        if argv[1:3] == ["auth", "status"]:
            return subprocess.CompletedProcess(argv, 0, json.dumps(self.auth), "")
        return subprocess.CompletedProcess(argv, self.probe_code, self.probe_stdout, "")


LEAKY_ENV = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL",
             "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX")


def iso(epoch):
    return datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def test_probe_parses_both_windows_and_plan():
    snapshot = quota.probe(SOURCE, "claude", FakeRunner(stream(event_line())), CLOCK)

    assert snapshot == quota.Snapshot("work", "2026-10-28T12:00:00+00:00", "max", "allowed", 10.0,
                                      iso(1793211600), 32.0, iso(1793466000), 0)


def test_probe_stamps_the_time_the_reading_returned():
    order = []
    later = datetime(2026, 10, 28, 12, 1, 30, tzinfo=timezone.utc)
    runner = FakeRunner(stream(event_line()))

    def run(argv, env, cwd):
        order.append("run")
        return runner(argv, env, cwd)

    def clock():
        order.append("clock")
        return later

    snapshot = quota.probe(SOURCE, "claude", run, clock)

    assert snapshot.ts == later.isoformat()
    assert order == ["run", "run", "clock"]


def test_probe_rounds_to_a_tenth_of_a_percent():
    snapshot = quota.probe(SOURCE, "claude", FakeRunner(stream(event_line(five=0.1234, seven=0.9996))), CLOCK)

    assert (snapshot.five_hour_pct, snapshot.seven_day_pct) == (12.3, 100.0)


def test_probe_uses_the_last_event():
    runner = FakeRunner(stream(event_line(seven=0.1), event_line(seven=0.2)))

    assert quota.probe(SOURCE, "claude", runner, CLOCK).seven_day_pct == 20.0


def test_probe_argv_is_minimal_and_isolated(monkeypatch):
    for name in LEAKY_ENV:
        monkeypatch.setenv(name, "must-not-leak")
    runner = FakeRunner(stream(event_line()))

    quota.probe(SOURCE, "/bin/claude-x", runner, CLOCK)

    (auth_argv, _, _), (argv, env, cwd) = runner.calls
    assert auth_argv == ["/bin/claude-x", "auth", "status", "--json"]
    assert argv[0] == "/bin/claude-x" and "-p" in argv
    for flag, value in [("--output-format", "stream-json"), ("--model", "haiku"), ("--tools", ""),
                        ("--system-prompt", "Reply ok."), ("--setting-sources", "")]:
        assert argv[argv.index(flag) + 1] == value
    for flag in ("--verbose", "--no-session-persistence", "--strict-mcp-config", "--disable-slash-commands"):
        assert flag in argv
    assert env["CLAUDE_CONFIG_DIR"] == "/h/w"
    for name in LEAKY_ENV:
        assert all(name not in call_env for _, call_env, _ in runner.calls)
    assert cwd != Path.cwd() and not cwd.exists()


def test_probe_of_the_default_config_dir_leaves_claude_config_dir_unset(monkeypatch, tmp_path):
    # Claude Code keys its keychain login by CLAUDE_CONFIG_DIR: setting it to the default dir looks logged out.
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/elsewhere")
    default = settings.Source("claude", "main", tmp_path / ".claude" / "projects", tmp_path / ".claude")
    runner = FakeRunner(stream(event_line()))

    quota.probe(default, "claude", runner, CLOCK)

    assert all("CLAUDE_CONFIG_DIR" not in env for _, env, _ in runner.calls)


def test_probe_keeps_only_subscription_type(tmp_path):
    snapshot = quota.probe(SOURCE, "claude", FakeRunner(stream(event_line())), CLOCK)
    conn = store.open_db(tmp_path / "usage.db")

    quota.record(conn, snapshot)

    dump = "\n".join(conn.iterdump()) + repr(snapshot)
    assert not any(secret in dump for secret in PRIVATE)
    assert conn.execute("SELECT subscription_type FROM quota_snapshots").fetchone() == ("max",)


def test_probe_on_rejected_limit_still_records(tmp_path):
    runner = FakeRunner(stream(event_line(status="rejected", five=1.0)), probe_code=1)
    conn = store.open_db(tmp_path / "usage.db")

    snapshot = quota.probe(SOURCE, "claude", runner, CLOCK)
    quota.record(conn, snapshot)
    quota.record(conn, snapshot)

    assert conn.execute("SELECT status, five_hour_pct FROM quota_snapshots").fetchall() == [("rejected", 100.0)]


def test_probe_without_event_returns_none():
    assert quota.probe(SOURCE, "claude", FakeRunner(stream()), CLOCK) is None
    assert quota.probe(SOURCE, "claude", FakeRunner("not json\n{broken"), CLOCK) is None


def test_probe_survives_an_unusable_auth_status():
    runner = FakeRunner(stream(event_line()), auth="logged out")

    assert quota.probe(SOURCE, "claude", runner, CLOCK).subscription_type is None


def test_probe_without_a_config_dir_is_skipped():
    runner = FakeRunner(stream(event_line()))

    assert quota.probe(settings.Source("claude", "w", Path("/x")), "claude", runner, CLOCK) is None
    assert runner.calls == []


def test_probe_reports_a_launch_failure():
    def broken(argv, env, cwd):
        return subprocess.CompletedProcess(argv, quota.MISSING_BINARY_EXIT, "", "secret stderr text")

    with pytest.raises(quota.QuotaProbeError) as error:
        quota.probe(SOURCE, "claude", broken, CLOCK)

    assert "secret" not in str(error.value)


def test_subprocess_runner_turns_launch_failures_into_results(tmp_path):
    missing = quota.subprocess_runner([str(tmp_path / "absent")], {}, tmp_path)
    ran = quota.subprocess_runner([sys.executable, "-c", "print('hi')"], {}, tmp_path)

    assert missing.returncode == quota.MISSING_BINARY_EXIT
    assert (ran.returncode, ran.stdout.strip()) == (0, "hi")


def stored_overage(tmp_path, line):
    conn = store.open_db(tmp_path / "usage.db")
    quota.record(conn, quota.probe(SOURCE, "claude", FakeRunner(stream(line)), CLOCK))
    return conn.execute("SELECT overage_in_use FROM quota_snapshots").fetchone()[0]


def test_snapshot_records_overage_in_use(tmp_path):
    assert stored_overage(tmp_path, event_line(overage=True)) == 1


def test_snapshot_records_overage_not_in_use_as_zero(tmp_path):
    assert stored_overage(tmp_path, event_line(overage=False)) == 0


@pytest.mark.parametrize("value", [None, "yes", 1])
def test_snapshot_overage_unknown_when_missing_or_not_a_boolean(tmp_path, value):
    info = json.loads(event_line())
    del info["rate_limit_info"]["isUsingOverage"]
    if value is not None:
        info["rate_limit_info"]["isUsingOverage"] = value

    assert stored_overage(tmp_path, json.dumps(info)) is None


def test_record_names_its_columns_so_a_later_column_cannot_break_a_lagging_writer(tmp_path):
    conn = store.open_db(tmp_path / "usage.db")
    conn.execute("ALTER TABLE quota_snapshots ADD COLUMN added_by_a_later_version TEXT")

    quota.record(conn, quota.probe(SOURCE, "claude", FakeRunner(stream(event_line())), CLOCK))

    assert conn.execute("SELECT COUNT(*) FROM quota_snapshots").fetchone() == (1,)
