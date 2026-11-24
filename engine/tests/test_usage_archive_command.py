import dataclasses
import json
import shutil
import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import engine_home
from sakura import cli
from sakura.accounts import registry as account_registry
from sakura.budget import alerts
from sakura.flow import notify
from sakura.usage import command as usage_command
from sakura.usage.archive import command, report, settings, store

FIXTURES = Path(__file__).parent / "fixtures" / "usage" / "archive"
PAGE = json.loads((FIXTURES / "gh_search_page.json").read_text())
NOW = datetime(2026, 10, 27, 12, 0, tzinfo=timezone.utc)
REPO = "acme/acme"


def ok_runner(argv):
    return subprocess.CompletedProcess(argv, 0, json.dumps(PAGE), "")


def failing_runner(argv):
    return subprocess.CompletedProcess(argv, 4, "", "gh: not logged in\n")


def config_for(tmp_path):
    shutil.copytree(FIXTURES / "claude", tmp_path / "claude")
    shutil.copytree(FIXTURES / "codex", tmp_path / "codex")
    return settings.Settings(
        db=tmp_path / "state" / "usage.db",
        sources=(settings.Source("claude", "personal", tmp_path / "claude"),
                 settings.Source("codex", "personal", tmp_path / "codex")),
        pr_repos=(REPO,), pr_days=7)


def count(config, table):
    conn = store.open_db(config.db)
    try:
        return conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    finally:
        conn.close()


def test_archive_end_to_end_on_fixture_roots(tmp_path, capsys):
    config = config_for(tmp_path)

    code = command.archive(config, ok_runner, NOW)

    lines = capsys.readouterr().out.splitlines()
    assert code == 0
    assert lines[0] == "claude personal: 1 files, 1 read, 2 turns"
    assert lines[1] == "codex personal: 1 files, 1 read, 3 turns"
    assert lines[2] == f"prs {REPO}: 3"
    assert lines[3].startswith("archive done in ") and lines[3].endswith(" s")
    assert (count(config, "turns"), count(config, "pull_requests"), count(config, "cwds")) == (5, 3, 2)


def test_archive_wakes_the_waiting_flows_after_the_quota_probes(tmp_path, capsys, monkeypatch):
    order = []
    monkeypatch.setattr(command, "_probe_quotas", lambda conn, settings, run: order.append("probe"))

    def waker():
        order.append("wake")
        return ["demo: resumed"]

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=waker) == 0

    assert order == ["probe", "wake"]
    assert "flow demo: resumed" in capsys.readouterr().out


def test_archive_evaluates_alerts_after_the_probes_and_before_the_wake(tmp_path, capsys, monkeypatch):
    order = []
    monkeypatch.setattr(command, "_probe_quotas", lambda conn, settings, run: order.append("probe"))

    def alerter(conn, settings, now):
        order.append("alerts")
        return ["alert opened: Week on pace for 117% at its reset (push: pushed)"]

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=lambda: order.append("wake") or [],
                           alerter=alerter) == 0

    assert order == ["probe", "alerts", "wake"]
    assert "alert opened: Week on pace for 117% at its reset (push: pushed)" in capsys.readouterr().out


def test_archive_survives_failing_alerts(tmp_path, capsys):
    def alerter(conn, settings, now):
        raise sqlite3.OperationalError("database is locked")

    assert command.archive(config_for(tmp_path), ok_runner, NOW, alerter=alerter) == 0

    captured = capsys.readouterr()
    assert "alerts: skipped (OperationalError)" in captured.err and "locked" not in captured.err
    assert f"prs {REPO}: 3" in captured.out


def test_archive_wakes_flows_and_refreshes_prs_when_the_alerts_raise_anything(tmp_path, capsys):
    woken = []

    def alerter(conn, settings, now):
        raise TypeError("unsupported format string passed to NoneType.__format__")

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=lambda: woken.append(1) or [],
                           alerter=alerter) == 0

    captured = capsys.readouterr()
    assert woken == [1] and f"prs {REPO}: 3" in captured.out
    assert "alerts: skipped (TypeError)" in captured.err and "unsupported" not in captured.err


# Budget paths run only on configured accounts: usage.toml holds legacy account keys (no zero-config default).
ACCOUNT_KEYS = '[[sources]]\nprovider = "claude"\naccount = "personal"\nroot = "/nowhere/.claude/projects"\n'


CODEX_KEY = '[[sources]]\nprovider = "codex"\naccount = "personal"\nroot = "/nowhere/.codex/sessions"\n'


def write_usage_toml(tmp_path, flow_toml="", codex=False):
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(ACCOUNT_KEYS + (CODEX_KEY if codex else ""))
    if flow_toml:
        engine_home.config_file().write_text(flow_toml)


def overage_snapshot(conn):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                 " VALUES ('personal', ?, 'max', 'allowed', 10.0, NULL, 61.0, '2026-10-30T12:00:00+00:00', 1)",
                 (NOW.isoformat(),))
    conn.commit()


def test_the_alert_lines_without_a_usage_config_are_empty(tmp_path):
    config = config_for(tmp_path)
    conn = store.open_db(config.db)
    overage_snapshot(conn)
    assert command._alert_lines(conn, config, NOW) == []
    assert conn.execute("SELECT COUNT(*) FROM budget_alerts").fetchone() == (0,)


def test_the_alert_lines_write_the_alert_and_say_the_push_was_skipped_without_a_topic(tmp_path):
    write_usage_toml(tmp_path, '[budget]\naccount = "personal"\n')
    config = config_for(tmp_path)
    conn = store.open_db(config.db)
    overage_snapshot(conn)

    lines = command._alert_lines(conn, config, NOW)

    assert lines == ["alert opened: personal: Extra usage is being billed (push: skipped (no ntfy topic configured))"]
    assert conn.execute("SELECT kind, resolved_at FROM budget_alerts").fetchall() == [("overage", None)]
    assert command._alert_lines(conn, config, NOW) == [], "an unchanged alert says nothing"


def labelled_registry():
    work = account_registry.Account("claude", "a", "Work", Path("/x"), True, None, (), ())
    return account_registry.Registry(accounts=(work,), implicit=False)


def test_the_alert_lines_name_a_retried_push_but_not_the_wait_for_a_topic():
    alert = alerts.Alert("overage:a:-", "a", "overage", "Extra usage is being billed", "r", "2026-10-28T14:00:00+00:00")
    retried = alerts.SyncResult((), (), ("failed",), (alert,))
    assert command._alert_report(retried, labelled_registry()) == [
        "alert push retried: Work: Extra usage is being billed (push: failed)"]
    waiting = alerts.SyncResult((), (), (notify.NO_TOPIC,), (alert,))
    assert command._alert_report(waiting, labelled_registry()) == []
    opened = alerts.SyncResult((alert,), (), (notify.NO_TOPIC,), (alert,))
    assert command._alert_report(opened, labelled_registry()) == [
        f"alert opened: Work: Extra usage is being billed (push: {notify.NO_TOPIC})"]


def test_the_alert_lines_report_a_resolved_alert(tmp_path):
    write_usage_toml(tmp_path, '[budget]\naccount = "personal"\n')
    config = config_for(tmp_path)
    conn = store.open_db(config.db)
    overage_snapshot(conn)
    command._alert_lines(conn, config, NOW)
    conn.execute("UPDATE quota_snapshots SET overage_in_use = 0")
    conn.commit()

    assert command._alert_lines(conn, config, NOW) == ["alert resolved: overage:personal:2026-10-30T12:00:00+00:00"]


def test_the_alert_step_does_not_need_a_resolvable_budget_account(tmp_path):
    write_usage_toml(tmp_path)
    config = config_for(tmp_path)
    conn = store.open_db(config.db)
    overage_snapshot(conn)

    assert command._alert_lines(conn, config, NOW) == [
        "alert opened: personal: Extra usage is being billed (push: skipped (no ntfy topic configured))"]


def test_archive_survives_a_failing_wake(tmp_path, capsys):
    def waker():
        raise OSError("registry unreadable")

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=waker) == 0

    captured = capsys.readouterr()
    assert "flow wake: skipped (OSError: registry unreadable)" in captured.err
    assert f"prs {REPO}: 3" in captured.out


def test_archive_second_run_writes_nothing_new(tmp_path, capsys):
    config = config_for(tmp_path)
    command.archive(config, ok_runner, NOW)
    capsys.readouterr()

    command.archive(config, ok_runner, NOW)

    lines = capsys.readouterr().out.splitlines()
    assert lines[0] == "claude personal: 1 files, 0 read, 0 turns"
    assert lines[1] == "codex personal: 1 files, 0 read, 0 turns"
    assert (count(config, "turns"), count(config, "pull_requests")) == (5, 3)


def test_archive_lock_held_exits_zero(tmp_path, capsys):
    config = config_for(tmp_path)

    with store.archive_lock(config.db):
        code = command.archive(config, ok_runner, NOW)

    assert code == 0
    assert capsys.readouterr().out == "usage archive already running\n"
    assert not config.db.exists() or count(config, "turns") == 0


def test_archive_pr_failure_still_archives_transcripts(tmp_path, capsys):
    config = config_for(tmp_path)

    code = command.archive(config, failing_runner, NOW)

    captured = capsys.readouterr()
    assert code == 0
    assert captured.err == f"prs {REPO}: skipped (gh: not logged in)\n"
    assert count(config, "turns") == 5 and count(config, "pull_requests") == 0


def test_report_by_pr_prints_linked_rows(tmp_path, capsys):
    config = config_for(tmp_path)
    command.archive(config, ok_runner, NOW)
    capsys.readouterr()

    code = command.report_by_pr(config, None, None)

    lines = capsys.readouterr().out.splitlines()
    assert code == 0
    rows = [line.split() for line in lines[1:-1]]
    assert [row[0] for row in rows] == [f"{REPO}#12", f"{REPO}#11"]
    pr11 = rows[1]
    assert pr11[1:4] == ["feature/x", "merged", "1"]
    assert pr11[4:10] == ["1", "3", "100", "50", "25", "-"]
    assert pr11[10:] == ["4", "120", "30"]
    assert lines[-1] == "linked turns 4 of 4 on branches, PRs with usage 2 of 3"


def test_report_by_pr_filters_by_repo(tmp_path, capsys):
    config = config_for(tmp_path)
    command.archive(config, ok_runner, NOW)
    capsys.readouterr()

    command.report_by_pr(config, "other/repo", None)

    assert capsys.readouterr().out.splitlines()[-1].endswith("PRs with usage 0 of 0")


def test_cli_bare_usage_unchanged(monkeypatch):
    seen = []
    monkeypatch.setattr(usage_command, "run", lambda as_json: seen.append(as_json) or 0)

    assert cli.main(["usage", "--json"]) == 0
    assert cli.main(["usage"]) == 0
    assert seen == [True, False]


def test_cli_routes_archive_and_report(monkeypatch, tmp_path):
    config = settings.Settings(tmp_path / "u.db", (), (), 60)
    calls = []
    monkeypatch.setattr(settings, "load", lambda: config)
    monkeypatch.setattr(command, "archive", lambda cfg, run, now: calls.append(("archive", cfg)) or 0)
    monkeypatch.setattr(command, "report_by_pr", lambda cfg, repo, days: calls.append((repo, days)) or 0)

    assert cli.main(["usage", "archive"]) == 0
    assert cli.main(["usage", "report", "--by-pr", "--repo", REPO, "--days", "9"]) == 0
    assert calls == [("archive", config), (REPO, 9)]


def test_report_by_pr_created_since_filters_pull_requests(tmp_path):
    config = config_for(tmp_path)
    command.archive(config, ok_runner, NOW)
    conn = store.open_db(config.db)

    result = report.by_pr(conn, None, datetime(2026, 10, 21, tzinfo=timezone.utc))

    assert [row[1] for row in result.rows] == [12]
    assert result.pull_requests == 2


def test_gh_runner_closes_stdin(monkeypatch):
    seen = {}

    def fake_run(argv, **kwargs):
        seen.update(kwargs)
        return subprocess.CompletedProcess(argv, 0, "{}", "")

    monkeypatch.setattr(subprocess, "run", fake_run)

    command.gh_runner(["gh", "api"])

    assert seen["stdin"] == subprocess.DEVNULL


def test_gh_runner_turns_any_os_error_into_a_failed_result(monkeypatch):
    def broken(argv, **kwargs):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(subprocess, "run", broken)

    done = command.gh_runner(["gh", "api"])

    assert done.returncode != 0 and "Permission denied" in done.stderr


def test_archive_with_an_unrunnable_gh_still_exits_zero(tmp_path, capsys, monkeypatch):
    config = config_for(tmp_path)

    def broken(argv, **kwargs):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(subprocess, "run", broken)

    code = command.archive(config, command.gh_runner, NOW)

    assert code == 0
    assert f"prs {REPO}: skipped (" in capsys.readouterr().err
    assert count(config, "turns") == 5


def test_gh_runner_retries_a_failed_call(monkeypatch):
    results = [(1, "gh: HTTP 502"), (1, "unexpected end of JSON input"), (0, "")]
    calls, pauses = [], []

    def flaky(argv, **kwargs):
        calls.append(argv)
        code, stderr = results[len(calls) - 1]
        return subprocess.CompletedProcess(argv, code, "{}" if code == 0 else "", stderr)

    monkeypatch.setattr(subprocess, "run", flaky)
    monkeypatch.setattr(command.time, "sleep", pauses.append)

    done = command.gh_runner(["gh", "api"])

    assert done.returncode == 0 and len(calls) == 3
    assert pauses == [command.GH_RETRY_PAUSE_SECONDS] * 2


def test_gh_runner_gives_up_after_three_attempts(monkeypatch):
    calls = []

    def failing(argv, **kwargs):
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 1, "", "gh: HTTP 502")

    monkeypatch.setattr(subprocess, "run", failing)
    monkeypatch.setattr(command.time, "sleep", lambda seconds: None)

    done = command.gh_runner(["gh", "api"])

    assert done.returncode == 1 and "HTTP 502" in done.stderr and len(calls) == command.GH_ATTEMPTS


def test_gh_runner_does_not_retry_a_missing_binary(monkeypatch):
    calls = []

    def missing(argv, **kwargs):
        calls.append(argv)
        raise FileNotFoundError(2, "No such file or directory")

    monkeypatch.setattr(subprocess, "run", missing)

    done = command.gh_runner(["gh", "api"])

    assert done.returncode == command.MISSING_BINARY_EXIT and len(calls) == 1


# -- the estimate of a Markdown plan ---------------------------------------------------------------

PLAN_TEXT = """# Plan

### Task 1: Parser

**Files:** Create `lib/a.py`, `lib/b.py`.

Class: mechanical

### Task 2: Wiring

**Files:** Modify `lib/c.py`.
"""


def calibrated_config(tmp_path, monkeypatch, snapshot_minutes_ago=5):
    from datetime import timedelta

    from sakura.budget import history
    from sakura.usage.archive import calibration

    config = settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7,
                               plans=(settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc)),))
    conn = store.open_db(config.db)
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES"
                 " ('a', ?, 'max', 'allowed', 20.0, '2026-10-27T17:00:00Z', 30.0, '2026-11-03T08:00:00Z')",
                 ((NOW - timedelta(minutes=snapshot_minutes_ago)).isoformat(),))
    conn.commit()
    conn.close()
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window="seven_day": [
        calibration.Ratio("a", "max20x", name, 1000.0, 6, 6.0, window) for name in ("haiku", "sonnet", "opus")])
    samples = [history.Sample("build", None, name, 1, 0, 0, 1000.0) for name in ("haiku", "sonnet") for _ in range(5)]
    samples += [history.Sample(role, None, "sonnet", 0, 0, 0, 1000.0)
                for role in ("planner", "plan_reviewer", "code_reviewer", "evaluator", "reporter")]
    monkeypatch.setattr(history, "samples", lambda conn, flow_repos=(): samples)
    return config


def flow_config_of(tmp_path, account="a"):
    from sakura.flow import config as flow_config

    return flow_config.load(tmp_path, home=tmp_path / "nohome", overrides={"budget": {"account": account}})


def test_estimate_from_a_markdown_plan(tmp_path, monkeypatch, capsys):
    config = calibrated_config(tmp_path, monkeypatch)

    code = command.estimate_plan(config, "a", PLAN_TEXT, False, NOW, flow_config_of(tmp_path))

    lines = capsys.readouterr().out.splitlines()
    assert code == 0
    assert lines[0].startswith("about ") and "% of your week (" in lines[0] and "% of the 5-hour window; " in lines[0]
    assert "max20x calibration" in lines[0]
    assert lines[1] == "fits above the 25% reserve"


def test_estimate_from_a_markdown_plan_names_the_account_reserve(tmp_path, monkeypatch, capsys):
    config = dataclasses.replace(calibrated_config(tmp_path, monkeypatch), reserves={"a": 10.0})

    command.estimate_plan(config, "a", PLAN_TEXT, False, NOW, flow_config_of(tmp_path))

    assert capsys.readouterr().out.splitlines()[1] == "fits above the 10% reserve"


def test_estimate_from_a_markdown_plan_as_json(tmp_path, monkeypatch, capsys):
    config = calibrated_config(tmp_path, monkeypatch)

    command.estimate_plan(config, "a", PLAN_TEXT, True, NOW, flow_config_of(tmp_path))

    data = json.loads(capsys.readouterr().out)
    assert data["fits"] is True and len(data["week_pct"]) == 3 and data["level"] in ("ok", "warn", "ask")
    assert data["text"].startswith("about ")


def test_estimate_from_a_markdown_plan_names_a_stale_quota(tmp_path, monkeypatch, capsys):
    config = calibrated_config(tmp_path, monkeypatch, snapshot_minutes_ago=120)

    command.estimate_plan(config, "a", PLAN_TEXT, False, NOW, flow_config_of(tmp_path))

    assert "does not fit: quota unknown" in capsys.readouterr().out.splitlines()[1]


def test_estimate_from_a_markdown_plan_without_history_exits_1(tmp_path, monkeypatch, capsys):
    from sakura.budget import history

    config = calibrated_config(tmp_path, monkeypatch)
    monkeypatch.setattr(history, "samples", lambda conn, flow_repos=(): [])

    code = command.estimate_plan(config, "a", PLAN_TEXT, False, NOW, flow_config_of(tmp_path))

    assert code == 1
    assert "no percent estimate; no history" in capsys.readouterr().err


def test_estimate_from_a_markdown_plan_defaults_to_the_budget_account(tmp_path, monkeypatch, capsys):
    config = calibrated_config(tmp_path, monkeypatch)

    assert command.estimate_plan(config, None, PLAN_TEXT, False, NOW, flow_config_of(tmp_path)) == 0


def test_estimate_from_a_markdown_plan_without_an_account_uses_the_engines_own(tmp_path, monkeypatch, capsys):
    config = calibrated_config(tmp_path, monkeypatch)
    home = tmp_path / "isolated-home"
    own = settings.Source("claude", "a", home / ".claude" / "projects", home / ".claude")
    config = dataclasses.replace(config, sources=(own,))

    code = command.estimate_plan(config, None, PLAN_TEXT, False, NOW, flow_config_of(tmp_path, account=""))

    assert code == 0
    assert "of your week" in capsys.readouterr().out


def test_estimate_from_a_markdown_plan_with_an_unresolvable_account_says_the_quota_is_unknown(tmp_path, monkeypatch,
                                                                                            capsys):
    config = calibrated_config(tmp_path, monkeypatch)

    code = command.estimate_plan(config, None, PLAN_TEXT, False, NOW, flow_config_of(tmp_path, account=""))

    assert code == 1
    assert "quota unknown: budget.account is empty" in capsys.readouterr().err


def test_usage_estimate_plan_flag_reads_the_file(tmp_path, monkeypatch, capsys):
    calibrated_config(tmp_path, monkeypatch)
    plan_path = tmp_path / "plan.md"
    plan_path.write_text(PLAN_TEXT)
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True)
    (home / ".config/sakura/usage.toml").write_text(
        f'db = "{tmp_path / "usage.db"}"\n\n[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "/nowhere/projects"\n'
        f'\n[[plans]]\naccount = "a"\nplan = "max20x"\nsince = "2026-01-01"\n')
    monkeypatch.chdir(tmp_path)

    code = cli.main(["usage", "estimate", "--plan", str(plan_path), "--account", "a"])

    lines = capsys.readouterr().out.splitlines()
    assert code == 0
    assert "of your week" in lines[0] and lines[1].startswith("does not fit: quota unknown")


def test_usage_estimate_needs_tokens_or_a_plan(capsys):
    with pytest.raises(SystemExit):
        cli.main(["usage", "estimate", "--account", "a"])


def test_estimate_plan_without_a_usage_archive_fails(tmp_path, monkeypatch, capsys):
    plan_path = tmp_path / "plan.md"
    plan_path.write_text(PLAN_TEXT)
    monkeypatch.chdir(tmp_path)

    code = cli.main(["usage", "estimate", "--plan", str(plan_path), "--account", "a"])

    assert code == 1
    assert "no usage archive configured" in capsys.readouterr().err


def test_usage_estimate_with_tokens_but_no_account_is_a_usage_error(capsys):
    with pytest.raises(SystemExit) as stopped:
        cli.main(["usage", "estimate", "--tokens", "opus=1000"])

    assert stopped.value.code == 2
    assert "--tokens needs --account" in capsys.readouterr().err


def test_usage_estimate_of_an_unreadable_plan_is_a_usage_error(tmp_path, capsys):
    (tmp_path / "isolated-home/.config/sakura").mkdir(parents=True)
    (tmp_path / "isolated-home/.config/sakura/usage.toml").write_text(ACCOUNT_KEYS)
    with pytest.raises(SystemExit) as stopped:
        cli.main(["usage", "estimate", "--plan", str(tmp_path / "missing.md"), "--account", "a"])

    assert stopped.value.code == 2
    assert "cannot read the plan" in capsys.readouterr().err


def test_archive_writes_the_weekly_page_after_the_alerts_and_before_the_wake(tmp_path, capsys):
    order = []

    def writer(conn, settings, now):
        order.append("weekly")
        return ["weekly report personal: consumi-personal-2026-W43.md"]

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=lambda: order.append("wake") or [],
                           alerter=lambda conn, settings, now: order.append("alerts") or [], weekly_writer=writer) == 0

    assert order == ["alerts", "weekly", "wake"]
    assert "weekly report personal: consumi-personal-2026-W43.md" in capsys.readouterr().out


def test_archive_survives_a_failing_weekly_report(tmp_path, capsys):
    woken = []

    def writer(conn, settings, now):
        raise PermissionError("/private/knowledge/personal/report/sakura is not writable")

    assert command.archive(config_for(tmp_path), ok_runner, NOW, waker=lambda: woken.append(1) or [],
                           weekly_writer=writer) == 0

    captured = capsys.readouterr()
    assert woken == [1] and f"prs {REPO}: 3" in captured.out
    assert "weekly report: skipped (PermissionError)" in captured.err and "private" not in captured.err


def test_the_alert_lines_evaluate_the_codex_account_of_the_registry_too(tmp_path):
    write_usage_toml(tmp_path, '[budget]\naccount = "personal"\n', codex=True)
    config = config_for(tmp_path)
    conn = store.open_db(config.db)
    overage_snapshot(conn)
    for hours_ago, pct in ((4, 40.0), (3, 41.0), (2, 42.0), (1, 43.0), (0.05, 44.0)):
        conn.execute("INSERT INTO quota_snapshots VALUES ('codex:personal', ?, 'prolite', 'allowed', NULL, NULL,"
                     " ?, '2026-10-30T12:00:00+00:00', NULL)", ((NOW - timedelta(hours=hours_ago)).isoformat(), pct))
    conn.commit()

    command._alert_lines(conn, config, NOW)

    assert conn.execute("SELECT account, kind FROM budget_alerts WHERE kind != 'outside' ORDER BY account").fetchall() == [
        ("codex:personal", "pace"), ("personal", "overage")]
