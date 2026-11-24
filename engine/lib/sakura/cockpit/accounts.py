"""The accounts block of the snapshot: what the dashboard needs for labels and settings, read-only.

Nothing here opens an account directory, a credential or a path into the document; the numbers come from the
usage archive, which is opened read-only and may be absent, empty or busy (the numbers are then None and 0).
"""

from __future__ import annotations

import os
import sqlite3
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta

from sakura.accounts import goose, registry
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.accounts.registry import Account, AccountsConfigError
from sakura.budget.settings import effective_reserve
from sakura.usage.archive import settings

# The snapshot is served every few seconds: a busy archive gives empty numbers instead of a slow page.
BUSY_TIMEOUT_SECONDS = 1
# Only the recent rows are sorted: the (account, ts) index bounds the scan, however long the history is. The
# cutoff is ISO text, which orders correctly against both the `Z` and the `+00:00` forms stored by prefix.
QUOTA_WINDOW = timedelta(days=8)
LATEST_WEEK = ("SELECT seven_day_pct, seven_day_resets_at FROM quota_snapshots WHERE account = ? AND ts >= ?"
               " AND seven_day_pct IS NOT NULL ORDER BY julianday(ts) DESC LIMIT 1")
LATEST_FIVE_HOUR = ("SELECT five_hour_pct FROM quota_snapshots WHERE account = ? AND ts >= ?"
                    " AND five_hour_pct IS NOT NULL ORDER BY julianday(ts) DESC LIMIT 1")
PROBLEM_UNREADABLE = "accounts.toml could not be read, no accounts listed; run sakura accounts"
OPEN_ALERTS = "SELECT account, COUNT(*) FROM budget_alerts WHERE resolved_at IS NULL GROUP BY account"


@dataclass(frozen=True)
class AccountRow:
    """One account as the dashboard sees it; `week_resets_at` is the archive's own text.

    `key` is the snapshot key alerts and quota rows are stored under (`codex:<id>` for Codex), so a caller
    never rebuilds the adapter's prefix rule. `reserve_week_pct_own` is the account's own reserve (None when the
    global one applies, which `reserve_week_pct` then holds); `plans` are (plan, since) newest first, `since`
    as a date when the period starts at midnight UTC, else an ISO time; `is_writable` is True only when an
    accounts.toml exists, the one case the write actions apply.
    """

    provider: str
    id: str
    key: str
    label: str
    state: str
    enabled: bool
    week_pct: float | None
    week_resets_at: str | None
    five_hour_pct: float | None
    reserve_week_pct: float | None
    open_alerts: int
    reserve_week_pct_own: float | None = None
    plans: tuple[tuple[str, str], ...] = ()
    is_writable: bool = True
    provider_config: dict | None = None
    readiness: str | None = None
    reason: str | None = None


def collect(global_reserve: float | None, now: datetime, env: Mapping[str, str] = os.environ) -> tuple[list[AccountRow], list[str]]:
    """The accounts ordered by provider and id, and the problem lines for what could not be read.

    `global_reserve` is the personal `budget.reserve_week_pct`, used for an account without its own; quota rows
    older than `QUOTA_WINDOW` before `now` are ignored. A registry that does not load gives no accounts and one
    fixed problem line (the error text names paths and goes to every allowed node), never an error.
    """
    try:
        loaded = registry.load(env=env)
    except AccountsConfigError:
        return [], [PROBLEM_UNREADABLE]
    since = (now.astimezone(UTC) - QUOTA_WINDOW).isoformat()
    accounts = sorted(loaded.accounts, key=lambda account: (account.provider, account.id))
    conn = _open_archive(env) if accounts else None
    try:
        alerts = _open_alerts(conn)
        writable = not (loaded.implicit or loaded.legacy)
        return [_row(account, conn, since, alerts, global_reserve, writable) for account in accounts], []
    finally:
        if conn is not None:
            conn.close()


def _open_archive(env: Mapping[str, str]) -> sqlite3.Connection | None:
    try:
        path = settings.db_path(env=env)
        return sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=BUSY_TIMEOUT_SECONDS)
    except (settings.UsageConfigError, sqlite3.Error):
        return None


def _open_alerts(conn: sqlite3.Connection | None) -> dict[str, int]:
    try:
        return dict(conn.execute(OPEN_ALERTS)) if conn is not None else {}
    except sqlite3.Error:
        return {}


def _row(account: Account, conn: sqlite3.Connection | None, since: str, alerts: dict[str, int],
         global_reserve: float | None, writable: bool) -> AccountRow:
    key = ADAPTERS[account.provider].snapshot_key(account.id)
    if account.provider == GOOSE:
        upstream = {"provider": account.goose_provider} if account.goose_provider else None
        return AccountRow(account.provider, account.id, key, account.label, registry.state_of(account), account.enabled,
                          None, None, None, None, 0, provider_config=upstream, **goose.readiness(account))
    week_pct, week_resets_at = _latest(conn, LATEST_WEEK, key, since, (None, None))
    (five_hour_pct,) = _latest(conn, LATEST_FIVE_HOUR, key, since, (None,))
    reserve = effective_reserve(account.reserve_week_pct, global_reserve)
    return AccountRow(account.provider, account.id, key, account.label, registry.state_of(account), account.enabled,
                      week_pct, week_resets_at, five_hour_pct, reserve, alerts.get(key, 0),
                      account.reserve_week_pct, _plans(account), writable)


def _plans(account: Account) -> tuple[tuple[str, str], ...]:
    newest_first = sorted(account.plans, key=lambda period: period.since, reverse=True)
    return tuple((period.plan, _since_text(period.since)) for period in newest_first)


def _since_text(moment: datetime) -> str:
    if moment == moment.replace(hour=0, minute=0, second=0, microsecond=0):
        return moment.date().isoformat()
    return moment.isoformat().replace("+00:00", "Z")


def _latest(conn: sqlite3.Connection | None, sql: str, key: str, since: str, none: tuple) -> tuple:
    try:
        found = conn.execute(sql, (key, since)).fetchone() if conn is not None else None
    except sqlite3.Error:
        found = None
    return found if found else none
