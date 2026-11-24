"""`sakura usage archive` and `sakura usage report --by-pr`."""

from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
import time
from collections.abc import Callable, Mapping
from dataclasses import replace
from datetime import UTC, datetime, timedelta

from sakura.accounts import labels
from sakura.accounts import registry as account_registry
from sakura.accounts.providers import ADAPTERS
from sakura.budget import account as budget_account
from sakura.budget import (
    alerts,
    flow_cost,
    plan_file,
    preflight,
    weekly,
    weekly_render,
    weekly_window,
)
from sakura.budget import settings as budget_settings
from sakura.flow import config as flow_config
from sakura.flow import notify, registry
from sakura.flow.config import FlowConfig
from sakura.usage.archive import (
    calibration,
    prs,
    quota,
    quota_report,
    report,
    repos,
    scan,
    store,
)
from sakura.usage.archive import settings as archive_settings
from sakura.usage.archive.settings import Settings

AlertLines = Callable[[sqlite3.Connection, Settings, datetime], list[str]]

GH_TIMEOUT_SECONDS = 120
GH_ATTEMPTS = 3
GH_RETRY_PAUSE_SECONDS = 5
MISSING_BINARY_EXIT = 127
TIMEOUT_EXIT = 124


def gh_runner(argv: list[str]) -> subprocess.CompletedProcess:
    """Run `gh`, retrying a failed call (GitHub answers searches with transient 502s and empty bodies)."""
    done = _run_gh(argv)
    for _ in range(GH_ATTEMPTS - 1):
        if done.returncode in (0, MISSING_BINARY_EXIT):
            break
        time.sleep(GH_RETRY_PAUSE_SECONDS)
        done = _run_gh(argv)
    return done


def _run_gh(argv: list[str]) -> subprocess.CompletedProcess:
    """One `gh` call with stdin closed; a launch failure or a timeout becomes a failed result, never an exception."""
    try:
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=GH_TIMEOUT_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, TIMEOUT_EXIT, "", "gh timed out")
    except OSError as error:
        return subprocess.CompletedProcess(argv, MISSING_BINARY_EXIT, "", f"gh could not run: {error.strerror or error}")


def archive(settings: Settings, run: prs.Runner, now: datetime, quota_run: quota.Runner = quota.subprocess_runner,
            waker: Callable[[], list[str]] | None = None, alerter: AlertLines | None = None,
            weekly_writer: AlertLines | None = None) -> int:
    """Scan every source into the archive, probe plan quotas, evaluate the early warnings on the fresh snapshots,
    write the weekly report page of a window that just reset, wake the flows whose budget wait is over (they decide
    on the same snapshots), then refresh pull requests; a second concurrent run exits 0.

    `waker`, `alerter` and `weekly_writer` return the lines to print; they default to `sakura flow wake`, the
    account alerts and the weekly pages.
    """
    with store.archive_lock(settings.db) as held:
        if not held:
            print("usage archive already running")
            return 0
        started = time.monotonic()
        conn = store.open_db(settings.db)
        try:
            reachable = replace(settings, sources=_reachable(settings.sources))
            _scan_sources(conn, reachable)
            _probe_quotas(conn, reachable, quota_run)
            _refresh_alerts(conn, settings, now, alerter or _alert_lines)
            _write_weekly(conn, settings, now, weekly_writer or _weekly_lines)
            _wake_flows(waker or _wake_waiting_flows)
            _refresh_pull_requests(conn, settings, run, now)
        finally:
            conn.close()
        print(f"archive done in {time.monotonic() - started:.1f} s")
    return 0


def report_by_pr(settings: Settings, repo: str | None, days: int | None) -> int:
    """Print token usage per pull request, newest first."""
    conn = store.open_db(settings.db)
    try:
        since = None if days is None else _utc_now() - timedelta(days=days)
        print(report.render(report.by_pr(conn, repo, since)))
    finally:
        conn.close()
    return 0


def quota_view(settings: Settings, as_json: bool) -> int:
    """Print the latest snapshots, plan periods, calibration ratios and limit events."""
    conn = store.open_db(settings.db)
    try:
        data = quota_report.build(conn, settings)
    finally:
        conn.close()
    print(json.dumps(data, indent=2) if as_json else quota_report.render(data, labels.load_or_empty()))
    return 0


def weekly_view(settings: Settings, week: str | None, account: str | None, as_json: bool, now: datetime) -> int:
    """Print where the quota of a weekly window went: the one starting in ISO `week`, else the last completed one.

    A window still running is printed as a partial week, up to `now`.

    `account` defaults to `budget.account` of the personal flow config. Exit 1 when the account has no such window.
    """
    cfg = flow_config.load_personal()
    account = account or budget_account.resolve(budget_settings.load(cfg.values).account, settings.sources,
                                                   disabled=settings.disabled)
    conn = store.open_db(settings.db)
    try:
        window = weekly_window.pick_window(conn, account, week, now)
        if window is None:
            print(f"no completed weekly window for {account}" + (f" starting in {week}" if week else ""), file=sys.stderr)
            return 1
        report = weekly.build(conn, settings, window.start, window.end, account=account,
                              flow_repos=registry.repos(), lane_roots=weekly.lane_roots())
    finally:
        conn.close()
    partial = window.end > now
    if as_json:
        print(json.dumps({**weekly.to_json(report), "partial_until": now.isoformat() if partial else None}, indent=2))
    else:
        header = f"week in progress, up to {now:%Y-%m-%d %H:%M} UTC\n" if partial else ""
        print(header + weekly_render.text(report, registry=labels.load_or_empty()))
    return 0


def estimate(settings: Settings, account: str, tokens: Mapping[str, float], as_json: bool, now: datetime) -> int:
    """Print the weekly percentage a weighted token mix costs; exit 1 when no calibration applies."""
    conn = store.open_db(settings.db)
    try:
        ratios = calibration.compute(conn, settings.plans)
    finally:
        conn.close()
    result = calibration.estimate(ratios, settings.plans, settings.plan_weights, account, now, tokens)
    if result is None:
        basis = preflight.no_calibration(settings, account, now)
        reason = basis if basis != preflight.NO_CALIBRATION else "no calibration or plan period applies"
        print(f"no estimate for {account}: {reason}", file=sys.stderr)
        return 1
    print(json.dumps({"pct": round(result.pct, 1), "basis": result.basis}) if as_json
          else f"about {result.pct:.1f}% of the week ({result.basis})")
    return 0


def estimate_plan(settings: Settings, account: str | None, plan_text: str, as_json: bool, now: datetime,
                  cfg: FlowConfig) -> int:
    """Print what a Markdown plan is expected to cost as a share of the week, and whether it fits above the
    reserve; exit 1 when no percentage can be computed. `account` defaults to `budget.account` of `cfg`, and an
    empty one to the engine's own Claude account.

    Raises ValueError when the plan has no task.
    """
    budget = budget_settings.load(cfg.values)
    account = account or budget.account
    result = flow_cost.for_tasks(cfg, plan_file.parse(plan_text), replace(budget, account=account), now, settings)
    if as_json:
        print(json.dumps(preflight.summary(result, budget)))
    else:
        _print_preflight(result, budget, sys.stdout if result.week_pct else sys.stderr)
    return 0 if result.week_pct else 1


def _print_preflight(result: preflight.Preflight, budget: budget_settings.BudgetSettings, stream) -> None:
    print(preflight.text(result), file=stream)
    reserve = budget_settings.effective_reserve(result.reserve_pct, budget.reserve_week_pct)
    verdict = f"fits above the {reserve:.0f}% reserve" if result.fits else f"does not fit: {result.reason}"
    print(verdict, file=stream)


def _alert_lines(conn: sqlite3.Connection, settings: Settings, now: datetime) -> list[str]:
    """Evaluate and store the account alerts of every enabled account, push the new ones, return what changed.

    Needs no repository: the budget thresholds and the notify and gates settings come from the personal flow
    config, the accounts and their own reserves from the account registry. With no usage.toml the budget is
    off and nothing is evaluated.
    """
    if not archive_settings.config_exists():
        return []
    cfg = flow_config.load_personal()
    registry_view = account_registry.load_subscriptions()
    accounts = _alert_accounts(registry_view)
    found = alerts.evaluate(conn, budget_settings.load(cfg.values), now, settings.plans, accounts)
    synced = alerts.sync(conn, found, now, notify.build_alert_pusher(cfg), registry_view,
                         evaluated=[account.key for account in accounts])
    return _alert_report(synced, registry_view)


def _alert_accounts(accounts: account_registry.Registry) -> list[alerts.AlertAccount]:
    return [alerts.AlertAccount(ADAPTERS[account.provider].snapshot_key(account.id), account.provider,
                                account.reserve_week_pct)
            for account in accounts.accounts if account.enabled]


def _alert_report(result: alerts.SyncResult, accounts: account_registry.Registry) -> list[str]:
    """One line per alert pushed this run: new ones, and earlier ones retried (not those waiting for an ntfy topic).

    Each line names the account by its display name, so the alerts of two accounts never read the same.
    """
    opened = {alert.id for alert in result.opened}
    pushed = [f"alert {'opened' if alert.id in opened else 'push retried'}: "
              f"{labels.display_name(alert.account, accounts)}: {alert.title} (push: {status})"
              for alert, status in zip(result.attempted, result.pushes)
              if alert.id in opened or status != notify.NO_TOPIC]
    return pushed + [f"alert resolved: {identifier}" for identifier in result.resolved]


def _refresh_alerts(conn: sqlite3.Connection, settings: Settings, now: datetime, alerter: AlertLines) -> None:
    """Print the alert changes; any failure is one line on stderr (the class only, the text may hold a topic or
    a path) and never fails the archive run, as with the flow wake."""
    try:
        for line in alerter(conn, settings, now):
            print(line)
    except Exception as error:  # noqa: BLE001 (the wake and the pull requests after it must still run)
        print(f"alerts: skipped ({type(error).__name__})", file=sys.stderr)


def _weekly_lines(conn: sqlite3.Connection, settings: Settings, now: datetime) -> list[str]:
    """Write the weekly report page of each account's last completed window that has none yet.

    Needs no repository: the page directory comes from the personal flow config. With no usage.toml nothing is
    written.
    """
    if not archive_settings.config_exists():
        return []
    values = flow_config.load_personal().values
    return weekly.write_pages(conn, settings, now, values, flow_repos=registry.repos(), lane_roots=weekly.lane_roots(),
                              registry=labels.load_or_empty())


def _write_weekly(conn: sqlite3.Connection, settings: Settings, now: datetime, writer: AlertLines) -> None:
    """Print the pages written; any failure is one line on stderr (the class only) and never stops the archive run."""
    try:
        for line in writer(conn, settings, now):
            print(line)
    except Exception as error:  # noqa: BLE001 (the wake and the pull requests after it must still run)
        print(f"weekly report: skipped ({type(error).__name__})", file=sys.stderr)


def _wake_waiting_flows() -> list[str]:
    from sakura.flow import cli, wake

    return wake.wake(cli.resume_detached, server_running=cli.tmux_server_running)


def _wake_flows(waker: Callable[[], list[str]]) -> None:
    """Resume what the fresh snapshots allow; a failure is one line on stderr and never fails the archive run."""
    try:
        for line in waker():
            print(f"flow {line}")
    except (OSError, sqlite3.Error, registry.RegistryError) as error:
        print(f"flow wake: skipped ({type(error).__name__}: {error})", file=sys.stderr)


def _reachable(sources: tuple[archive_settings.Source, ...]) -> tuple[archive_settings.Source, ...]:
    """The sources whose config directory exists; each missing one is skipped with one line, never an error."""
    reachable = []
    for source in sources:
        if source.config_dir is not None and not source.config_dir.is_dir():
            print(f"{source.provider} {source.account}: config dir missing, skipped", file=sys.stderr)
        else:
            reachable.append(source)
    return tuple(reachable)


def _probe_quotas(conn, settings: Settings, run: quota.Runner) -> None:
    for source in settings.sources:
        if source.provider != "claude" or source.config_dir is None:
            continue
        try:
            line = _probe_one(conn, source, settings.claude_bin, run)
        except quota.QuotaProbeError as error:
            print(f"quota {source.account}: skipped ({error})", file=sys.stderr)
        except (OSError, sqlite3.Error) as error:
            print(f"quota {source.account}: skipped ({type(error).__name__})", file=sys.stderr)
        else:
            print(line)


def _probe_one(conn, source, claude_bin: str, run: quota.Runner) -> str:
    snapshot = quota.probe(source, claude_bin, run)
    if snapshot is None:
        raise quota.QuotaProbeError("no rate limit event")
    quota.record(conn, snapshot)
    return _quota_line(snapshot)


def _quota_line(snapshot: quota.Snapshot) -> str:
    week = _percent(snapshot.seven_day_pct)
    return (f"quota {snapshot.account}: week {week} (resets {snapshot.seven_day_resets_at or '-'}), "
            f"5h {_percent(snapshot.five_hour_pct)}, plan {snapshot.subscription_type or '-'}")


def _percent(value: float | None) -> str:
    return "-" if value is None else f"{value:.1f}%"


def _scan_sources(conn, settings: Settings) -> None:
    for source in settings.sources:
        stats = scan.scan_source(conn, source)
        print(f"{source.provider} {source.account}: {stats.files_seen} files, "
              f"{stats.files_read} read, {stats.turns_written} turns")
    repos.resolve_new_cwds(conn)


def _refresh_pull_requests(conn, settings: Settings, run: prs.Runner, now: datetime) -> None:
    for repo in settings.pr_repos:
        try:
            count = prs.refresh(conn, [repo], settings.pr_days, now, run)
        except prs.PullRequestError as error:
            reason = str(error).removeprefix(f"{repo}: ")
            print(f"prs {repo}: skipped ({reason})", file=sys.stderr)
            continue
        print(f"prs {repo}: {count}")


def _utc_now() -> datetime:
    return datetime.now(UTC)
