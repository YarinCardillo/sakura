"""Early warnings about the week, kept in the usage archive for the phone push and for status pages that read the open rows.

Account alerts are derived from the quota snapshots by the archive run (`evaluate`, then `sync`), for every
enabled account of the registry:

- `pace`: the weekly percentage projected at the reset passes `100 - reserve_week_pct` (the account's own
  reserve, else the global one). It reads the snapshot trend (`projection.project_week`), never local tokens, so
  use outside Claude Code is part of it.
- `outside`: a UTC day whose weekly rise local turns do not explain by more than `outside_share_alert`.
- `overage`: the latest snapshot that says so reports extra usage in use.

A Codex account (`codex:<name>`, snapshots from the rollout scan) gets `pace` only: `outside` needs a calibration
Codex does not have, and its snapshots never report overage. A stored title never names the account: the push renders
`<display name>: <title>` from the registry at push time, so a renamed label shows on alerts already open.

`request` alerts belong to one flow: opened by the budget gate and guard when a flow asks or waits, resolved when it
is released, resumes or finishes. They are pushed by the flow's own notifier, so `sync` neither pushes nor resolves them.

An alert id is stable per window (`kind:account:window`), so an unchanged alert is updated, not pushed again. A push
that did not go out (ntfy down, no topic yet) leaves `pushed_at` empty and is retried on every run while the alert is
open; a night-mode deferral counts as pushed, and a request alert is marked pushed when written. Every percentage
carries its basis and, where one exists, a band.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Collection, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import NamedTuple, Protocol

from sakura.accounts import labels
from sakura.accounts.registry import Registry
from sakura.budget.account import CLAUDE
from sakura.budget.settings import BudgetSettings, effective_reserve
from sakura.flow.notify import DEFERRED, PUSHED
from sakura.usage import projection
from sakura.usage.archive import calibration
from sakura.usage.archive.settings import Plan

PACE, OUTSIDE, OVERAGE, REQUEST = "pace", "outside", "overage", "request"
ACCOUNT_KINDS = (PACE, OUTSIDE, OVERAGE)
PUSH_EVENT = "budget_alert"
DELIVERED = (PUSHED, DEFERRED)
NO_WINDOW = "-"
FULL_PERCENT = 100
OUTSIDE_LOOKBACK_DAYS = 2
# Plan percentages have 1% resolution, so a rise of one point is as likely rounding as use outside Claude Code.
MIN_OUTSIDE_COST_PCT = 2.0
RESOLUTION_PCT = 1.0
LATEST_WEEK = ("SELECT ts, seven_day_pct, seven_day_resets_at FROM quota_snapshots WHERE account = ?"
               " AND seven_day_pct IS NOT NULL ORDER BY julianday(ts) DESC LIMIT 1")
LATEST_OVERAGE = ("SELECT ts, overage_in_use, seven_day_pct, seven_day_resets_at FROM quota_snapshots"
                  " WHERE account = ? AND overage_in_use IS NOT NULL ORDER BY julianday(ts) DESC LIMIT 1")
UNPUSHED = ("SELECT id, account, kind, title, reason, since, url FROM budget_alerts WHERE resolved_at IS NULL"
            " AND pushed_at IS NULL AND kind IN ({kinds}) ORDER BY since, id")
UPSERT = ("INSERT INTO budget_alerts (id, account, kind, title, reason, since, resolved_at, url, pushed_at)"
          " VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?) ON CONFLICT (id) DO UPDATE SET title = excluded.title,"
          " reason = excluded.reason, resolved_at = NULL, url = excluded.url")


@dataclass(frozen=True)
class Alert:
    """One open warning; `since` is when it was first seen true, kept by the archive across updates."""

    id: str
    account: str
    kind: str
    title: str
    reason: str
    since: str
    url: str | None = None


class AlertAccount(NamedTuple):
    """An enabled account to evaluate: its snapshot key, its provider, and its own reserve (None: the global one)."""

    key: str
    provider: str
    reserve_week_pct: float | None = None


class SyncResult(NamedTuple):
    """The alerts that opened for the first time, the ids resolved, and what each push did.

    `attempted` are the alerts a push was tried for (the new ones and the earlier ones that never went out);
    `pushes` is aligned with it.
    """

    opened: tuple[Alert, ...]
    resolved: tuple[str, ...]
    pushes: tuple[str, ...]
    attempted: tuple[Alert, ...] = ()


class Pusher(Protocol):
    def push(self, event: str, title: str, message: str) -> str: ...


def evaluate(conn: sqlite3.Connection, settings: BudgetSettings, now: datetime, plans: Sequence[Plan],
             accounts: Sequence[AlertAccount]) -> list[Alert]:
    """The account alerts that are true now for each of `accounts` (the enabled accounts of the registry).

    `plans` are the plan periods of the usage config; without them the plan family the probe reported is used.
    Every account gets its pace alert against its own reserve; Claude accounts also get overage and outside use.
    """
    claude = [account for account in accounts if account.provider == CLAUDE]
    found = [alert for account in accounts for alert in _pace(conn, _reserve(account, settings), now, account.key)]
    found += _outside(conn, settings, now, plans, {account.key for account in claude})
    return found + [alert for account in claude for alert in _overage(conn, settings, now, account.key)]


def sync(conn: sqlite3.Connection, found: Sequence[Alert], now: datetime, pusher: Pusher | None = None,
         registry: Registry = labels.NO_ACCOUNTS, evaluated: Collection[str] | None = None) -> SyncResult:
    """Write `found` as the open account alerts: new ones are inserted, known ones are updated silently (a resolved
    one that is true again in its own window reopens without a second push), and open account alerts no longer in
    `found` get `resolved_at`, but only those of the account keys in `evaluated` (None: every account), so the open
    alerts of an account that was not evaluated (disabled, removed) stay as they are. Every open account alert not yet pushed is pushed now, so a push that failed is
    retried; one that went out (or was deferred by night mode) is marked `pushed_at`. Request alerts are never touched.
    A push is titled with the account's display name from `registry` (the bare key when it is not in it).
    """
    known = {row[0] for row in conn.execute(
        f"SELECT id FROM budget_alerts WHERE kind IN ({','.join('?' * len(ACCOUNT_KINDS))})", ACCOUNT_KINDS)}
    opened = tuple(alert for alert in found if alert.id not in known)
    for alert in found:
        _upsert(conn, alert)
    resolved = _resolve_absent(conn, {alert.id for alert in found}, _stamp(now), evaluated)
    conn.commit()
    attempted, pushes = _push_unpushed(conn, pusher, now, registry) if pusher else ((), ())
    return SyncResult(opened, resolved, pushes, attempted)


def open_request(conn: sqlite3.Connection, account: str, repo: str, task: str, title: str, reason: str,
                 now: datetime) -> None:
    """Open, or update the numbers of, the request alert of a flow in the account's current weekly window."""
    window = _window(conn, account)
    _upsert(conn, Alert(f"{REQUEST}:{account}:{repo}:{task}:{window}", account, REQUEST, title, reason, _stamp(now)),
            pushed_at=_stamp(now))
    conn.commit()


def resolve_requests(conn: sqlite3.Connection, account: str, repo: str, task: str, now: datetime) -> int:
    """Close the open request alerts of a flow in any window; the number closed."""
    prefix = f"{REQUEST}:{account}:{repo}:{task}:"
    closed = conn.execute("UPDATE budget_alerts SET resolved_at = ? WHERE kind = ? AND resolved_at IS NULL"
                          " AND substr(id, 1, ?) = ?", (_stamp(now), REQUEST, len(prefix), prefix)).rowcount
    conn.commit()
    return closed


def _reserve(account: AlertAccount, settings: BudgetSettings) -> float:
    return effective_reserve(account.reserve_week_pct, settings.reserve_week_pct)


def _pace(conn: sqlite3.Connection, reserve: float, now: datetime, account: str) -> list[Alert]:
    """The pace alert of `account`; an unknown projection (too few or stale snapshots) neither opens nor resolves one."""
    projected = projection.project_week(conn, account, now)
    latest = conn.execute(LATEST_WEEK, (account,)).fetchone()
    ceiling = FULL_PERCENT - reserve
    if projected is None or latest is None:
        return _still_open_pace(conn, account, now)
    if projected <= ceiling:
        return []
    current, reset = latest[1], latest[2]
    reason = (f"The week stands at {current:.0f}% (plan snapshot). The trend of the last 24 h points to about "
              f"{projected:.0f}% when it resets at {reset}, above the {ceiling:.0f}% ceiling (reserve "
              f"{reserve:.0f}%). Basis: snapshot trend, no token calibration, so use outside "
              f"Claude Code counts; band {current:.0f}-{projected:.0f}% (the low edge is what is already used).")
    return [Alert(f"{PACE}:{account}:{reset}", account, PACE,
                  f"Week on pace for {projected:.0f}% at its reset", reason, _stamp(now))]


def _still_open_pace(conn: sqlite3.Connection, account: str, now: datetime) -> list[Alert]:
    """The open pace alert of a week that is not over yet, as it is: an unknown projection is not a clear one."""
    rows = conn.execute("SELECT id, title, reason, since FROM budget_alerts WHERE kind = ? AND account = ?"
                        " AND resolved_at IS NULL", (PACE, account)).fetchall()
    return [Alert(identifier, account, PACE, title, reason, since) for identifier, title, reason, since in rows
            if calibration.parse_moment(identifier[len(f"{PACE}:{account}:"):]) > now]


def _outside(conn: sqlite3.Connection, settings: BudgetSettings, now: datetime, plans: Sequence[Plan],
             accounts: Collection[str]) -> list[Alert]:
    today = now.astimezone(timezone.utc).date()
    window = (today - timedelta(days=OUTSIDE_LOOKBACK_DAYS - 1), today)
    shares = calibration.outside_share(conn, plans, window)
    return [_outside_alert(settings, share, now) for share in shares
            if share.account in accounts and _is_over(share, settings)]


def _is_over(share: calibration.OutsideShare, settings: BudgetSettings) -> bool:
    if share.share is None or share.share <= settings.outside_share_alert:
        return False
    return share.rise * share.share >= MIN_OUTSIDE_COST_PCT


def _outside_alert(settings: BudgetSettings, share: calibration.OutsideShare, now: datetime) -> Alert:
    cost = share.rise * share.share
    low, high = max(cost - RESOLUTION_PCT, 0.0), cost + RESOLUTION_PCT
    gap = (f" A further {share.unattributed:.0f}% rose across a gap of snapshots (unattributed, not counted)."
           if share.unattributed else "")
    reason = (f"{share.day} (UTC): the week rose {share.rise:.0f}% (plan snapshots); local turns explain "
              f"{share.local_pct:.0f}% ({_local_basis(share)}); {cost:.0f}% ({share.share:.0%}) "
              f"came from outside Claude Code, above the {settings.outside_share_alert:.0%} alert; band "
              f"{low:.0f}-{high:.0f}% (1% snapshot resolution).{gap}")
    return Alert(f"{OUTSIDE}:{share.account}:{share.day}", share.account, OUTSIDE,
                 f"Outside use took {cost:.0f}% of your week on {share.day}", reason, _stamp(now))


def _local_basis(share: calibration.OutsideShare) -> str:
    return "calibrated token ratios of the plan" if share.local_pct else "no local turns"


def _overage(conn: sqlite3.Connection, settings: BudgetSettings, now: datetime, account: str) -> list[Alert]:
    row = conn.execute(LATEST_OVERAGE, (account,)).fetchone()
    if row is None or row[1] != 1:
        return []
    ts, _, pct, reset = row
    week = f"the week stands at {pct:.0f}% (measured, no estimate)" if pct is not None else "it has no weekly figure"
    reason = (f"The plan snapshot of {ts} reports extra usage (overage) in use; {week}. "
              f"Extra usage is billed on top of the plan.")
    return [Alert(f"{OVERAGE}:{account}:{reset or NO_WINDOW}", account, OVERAGE,
                  "Extra usage is being billed", reason, _stamp(now))]


def _window(conn: sqlite3.Connection, account: str) -> str:
    latest = conn.execute(LATEST_WEEK, (account,)).fetchone()
    return latest[2] if latest else NO_WINDOW


def _upsert(conn: sqlite3.Connection, alert: Alert, pushed_at: str | None = None) -> None:
    """Insert or update an alert; `pushed_at` only counts for a new row, an update never moves it."""
    conn.execute(UPSERT, (alert.id, alert.account, alert.kind, alert.title, alert.reason, alert.since, alert.url,
                          pushed_at))


def _push_unpushed(conn: sqlite3.Connection, pusher: Pusher, now: datetime,
                   registry: Registry) -> tuple[tuple[Alert, ...], tuple[str, ...]]:
    """Push every open account alert that has not gone out; mark the ones the pusher delivered or deferred."""
    rows = conn.execute(UNPUSHED.format(kinds=",".join("?" * len(ACCOUNT_KINDS))), ACCOUNT_KINDS).fetchall()
    attempted = tuple(Alert(*row) for row in rows)
    pushes = tuple(pusher.push(PUSH_EVENT, _push_title(alert, registry), alert.reason) for alert in attempted)
    delivered = [(_stamp(now), alert.id) for alert, status in zip(attempted, pushes) if status in DELIVERED]
    conn.executemany("UPDATE budget_alerts SET pushed_at = ? WHERE id = ?", delivered)
    conn.commit()
    return attempted, pushes


def _push_title(alert: Alert, registry: Registry) -> str:
    return f"{labels.display_name(alert.account, registry)}: {alert.title}"


def _resolve_absent(conn: sqlite3.Connection, present: set[str], stamp: str,
                    evaluated: Collection[str] | None) -> tuple[str, ...]:
    open_rows = conn.execute(
        f"SELECT id, account FROM budget_alerts WHERE resolved_at IS NULL AND kind IN ({','.join('?' * len(ACCOUNT_KINDS))})"
        " ORDER BY id", ACCOUNT_KINDS)
    gone = tuple(identifier for identifier, account in open_rows
                 if identifier not in present and (evaluated is None or account in evaluated))
    conn.executemany("UPDATE budget_alerts SET resolved_at = ? WHERE id = ?", [(stamp, identifier) for identifier in gone])
    return gone


def _stamp(moment: datetime) -> str:
    return moment.isoformat(timespec="seconds")
