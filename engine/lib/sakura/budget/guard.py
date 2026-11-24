"""The live guard of an account: reserve floors, pacing to the night and holds of the flows already started.

`check` reads the latest quota snapshot, never an estimate, so the guard works before any calibration exists.
A snapshot older than `budget.snapshot_max_age_minutes` is unknown, and unknown asks the owner; it is never zero.
A window whose reset time has passed counts as 0% (its figure is from before the reset); nothing else is invented.

Codex has no probe, so its snapshots are only as fresh as its last run: `check_lower_bound` reads them at any age
as a floor (inside a window the percentage only rises) and never asks.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path
from typing import Literal, NamedTuple

from sakura.budget.settings import BudgetSettings
from sakura.usage import projection
from sakura.usage.archive.calibration import parse_moment

RUN, DEFER, PAUSE, ASK = "run", "defer", "pause", "ask"
# The rule that stopped a run. An owner release lifts that rule alone; every other rule keeps applying.
WEEK_RULE, FIVE_HOUR_RULE, PACING_RULE, STALENESS_RULE = "week", "five_hour", "pacing", "staleness"
# The rule of a wait that pace made (`flow/pace_check.py`); the guard itself never decides it.
PACE_RULE = "pace"
UNKNOWN_RULE = "unknown"
RULE_OF_ACTION = {PAUSE: WEEK_RULE, DEFER: FIVE_HOUR_RULE, ASK: STALENESS_RULE}
FULL_PERCENT = 100
WEEK = timedelta(days=7)
LATEST_SNAPSHOT = "SELECT ts, seven_day_pct, seven_day_resets_at, five_hour_pct, five_hour_resets_at FROM quota_snapshots" \
           " WHERE account = ? ORDER BY ts DESC LIMIT 1"


@dataclass(frozen=True)
class Decision:
    """What the guard says about starting a run: `until` is when to look again (None: only the owner decides).

    `rule` names the rule that decided it (`week`, `five_hour`, `pacing`, `staleness`, or `unknown` for a quota that
    cannot be judged at all); a run has none. `account` is the account the decision is about when the decider knows it
    (pace does), else empty.
    """

    action: Literal["run", "defer", "pause", "ask"]
    until: datetime | None
    reason: str
    rule: str = ""
    account: str = ""


class Held(NamedTuple):
    """The weekly share one started flow holds, and the live week figure at the moment it took the hold."""

    task_key: str
    week_pct: float
    week_at_hold: float | None = None


class Snapshot(NamedTuple):
    """The latest quota snapshot of an account, window resets parsed."""

    ts: str
    week_pct: float | None
    week_resets: datetime | None
    five_pct: float | None
    five_resets: datetime | None


def check(account: str, now: datetime, conn: sqlite3.Connection, settings: BudgetSettings, pending_pct: float,
          urgent: bool, task_key: str | None = None, tail_pct: float = 0.0, lifted: str = "",
          snapshot: "Snapshot | None" = None) -> Decision:
    """Whether a run of `account` may start now.

    `pending_pct` is the weekly share the flow is expected to spend; `task_key` names the flow asking, whose
    own hold is not counted against it. The live week already contains what the flow spent since it took its
    hold, so that part is taken off `pending_pct`, but never below `tail_pct`, the share its later phases still
    need. Order of the rules: unknown quota asks; the week (live figure, plus the holds of other flows, plus
    what the flow has left) at the reserve pauses until the weekly reset; the 5-hour window at its reserve defers
    until its reset; a flow that is not urgent defers to the night when the projected week crosses the reserve
    before the reset; otherwise run.

    `lifted` is the one rule the owner released (see `Decision.rule`): it is skipped, the others still apply. A
    released `staleness` rule accepts a snapshot of any age and judges the other rules on its figures; a missing
    snapshot or weekly figure still asks.

    `snapshot` stands in for the archive's latest snapshot of `account`: the numbers Leeway reports (`leeway_numbers`).
    """
    snapshot = snapshot or _latest(conn, account)
    unknown = _unknown(snapshot, account, now, settings, lifted)
    if unknown:
        return unknown
    week = _live(snapshot.week_pct, snapshot.week_resets, now)
    since = _week_start(snapshot.week_resets, now)
    held = holds(conn, account, task_key, since)
    committed = sum(entry.week_pct for entry in held) + _remaining(conn, task_key, since, week, pending_pct, tail_pct)
    ceiling = FULL_PERCENT - settings.reserve_week_pct
    if week + committed >= ceiling and lifted != WEEK_RULE:
        return Decision(PAUSE, _retry_at(snapshot.week_resets, now),
                        f"week {week:.0f}% + {committed:.0f}% held or pending{_holders(held)} reaches {ceiling:.0f}% "
                        f"(reserve {settings.reserve_week_pct:.0f}%)", WEEK_RULE)
    return (_five_hour(snapshot, now, settings, lifted)
            or _night_pacing(conn, account, now, settings, committed, urgent, lifted) or Decision(RUN, None, ""))


def check_lower_bound(account: str, now: datetime, conn: sqlite3.Connection, settings: BudgetSettings,
                      urgent: bool, lifted: str = "", snapshot: "Snapshot | None" = None) -> Decision:
    """Whether a run on `account` may start, reading its latest snapshot of any age as a lower bound.

    Same reserve rules as `check` but never unknown: no snapshot yet, or a window the snapshot does not report,
    skips that check (a Codex account only gets snapshots by running). Holds and pending shares belong to the
    Claude account, so none count here. `snapshot` stands in for the archive's latest one (the numbers Leeway reports).
    """
    snapshot = snapshot or _latest(conn, account)
    if snapshot is None:
        return Decision(RUN, None, "")
    ceiling = FULL_PERCENT - settings.reserve_week_pct
    week = _live(snapshot.week_pct, snapshot.week_resets, now) if snapshot.week_pct is not None else None
    if week is not None and week >= ceiling and lifted != WEEK_RULE:
        return Decision(PAUSE, _retry_at(snapshot.week_resets, now),
                        f"week at least {week:.0f}% (snapshot of {snapshot.ts}, a lower bound) reaches {ceiling:.0f}% "
                        f"(reserve {settings.reserve_week_pct:.0f}%)", WEEK_RULE)
    return (_five_hour(snapshot, now, settings, lifted)
            or _night_pacing(conn, account, now, settings, 0.0, urgent, lifted) or Decision(RUN, None, ""))


def hold(conn: sqlite3.Connection, task_key: str, account: str, week_pct: float, now: datetime,
         week_at_hold: float | None = None, week_start: datetime | None = None) -> None:
    """Reserve `week_pct` of the week for a started flow, noting the live week figure it started from.

    Taking it again updates the amount but keeps the date and the starting figure, unless the hold dates from before
    `week_start`: the week it was taken in is over, so it starts afresh.
    """
    row = conn.execute("SELECT created_at, week_at_hold FROM budget_holds WHERE task_key = ?", (task_key,)).fetchone()
    if row and (week_start is None or parse_moment(row[0]) >= week_start):
        created_at, week_at_hold = row[0], row[1] if row[1] is not None else week_at_hold
    else:
        created_at = now.isoformat()
    conn.execute("INSERT INTO budget_holds (task_key, account, week_pct, created_at, week_at_hold)"
                 " VALUES (?, ?, ?, ?, ?) ON CONFLICT (task_key) DO UPDATE SET account = excluded.account,"
                 " week_pct = excluded.week_pct, created_at = excluded.created_at,"
                 " week_at_hold = excluded.week_at_hold",
                 (task_key, account, week_pct, created_at, week_at_hold))
    conn.commit()


def week_position(conn: sqlite3.Connection, account: str, now: datetime) -> tuple[float | None, datetime | None]:
    """The live weekly figure of the latest snapshot and when its window began; (None, None) without a weekly figure."""
    snapshot = _latest(conn, account)
    if snapshot is None or snapshot.week_pct is None:
        return None, None
    return _live(snapshot.week_pct, snapshot.week_resets, now), _week_start(snapshot.week_resets, now)


def release(conn: sqlite3.Connection, task_key: str) -> None:
    """Give a flow's hold back; a flow without one changes nothing."""
    conn.execute("DELETE FROM budget_holds WHERE task_key = ?", (task_key,))
    conn.commit()


def holds(conn: sqlite3.Connection, account: str, excluding: str | None = None,
          since: datetime | None = None) -> list[Held]:
    """The holds of the flows of `account`, not counting `excluding`'s own and any taken before `since`.

    A hold from before the start of the current week was spent (or abandoned) in a week that is over.
    """
    rows = conn.execute("SELECT task_key, week_pct, week_at_hold, created_at FROM budget_holds WHERE account = ?"
                        " AND task_key IS NOT ? ORDER BY task_key", (account, excluding)).fetchall()
    return [Held(key, float(pct), at_hold) for key, pct, at_hold, created in rows
            if since is None or parse_moment(created) >= since]


def in_night(now: datetime, night: str) -> bool:
    """Whether `now` (in its own timezone) is inside the `HH:MM-HH:MM` night window, which may cross midnight."""
    start, end = _bounds(night)
    moment = now.time().replace(second=0, microsecond=0, tzinfo=None)
    return start <= moment < end if start < end else moment >= start or moment < end


def next_night_start(now: datetime, night: str) -> datetime:
    """The next start of the night window after `now`, in the timezone of `now`."""
    start, _ = _bounds(night)
    today = now.replace(hour=start.hour, minute=start.minute, second=0, microsecond=0)
    return today if today > now else today + timedelta(days=1)


def _latest(conn: sqlite3.Connection, account: str) -> Snapshot | None:
    row = conn.execute(LATEST_SNAPSHOT, (account,)).fetchone()
    if row is None:
        return None
    return Snapshot(row[0], row[1], _moment(row[2]), row[3], _moment(row[4]))


def _remaining(conn: sqlite3.Connection, task_key: str | None, since: datetime | None, week: float,
               pending_pct: float, tail_pct: float) -> float:
    """What the flow still has to spend: `pending_pct` less the week gained since its hold, never below its tail.

    Without a hold of its own from this week there is no baseline, and all of `pending_pct` counts.
    """
    own = conn.execute("SELECT week_at_hold, created_at FROM budget_holds WHERE task_key = ?", (task_key,)).fetchone()
    if own is None or own[0] is None or (since is not None and parse_moment(own[1]) < since):
        return pending_pct
    return max(min(tail_pct, pending_pct), pending_pct - max(0.0, week - own[0]))


def _week_start(resets: datetime | None, now: datetime) -> datetime | None:
    """When the current weekly window began: a week before its reset, or the reset itself once it has passed."""
    if resets is None:
        return None
    return resets if resets <= now else resets - WEEK


def _holders(held: list[Held]) -> str:
    """` (repo:task 20%, ...)` naming the flows whose holds count, empty when none do."""
    if not held:
        return ""
    return " (holds: " + ", ".join(f"{Path(entry.task_key.rsplit(':', 1)[0]).name}:{entry.task_key.rsplit(':', 1)[1]} "
                                   f"{entry.week_pct:.0f}%" for entry in held) + ")"


def _moment(text: str | None) -> datetime | None:
    return parse_moment(text) if text else None


def _unknown(snapshot: Snapshot | None, account: str, now: datetime, settings: BudgetSettings,
             lifted: str) -> Decision | None:
    """The `ask` for a quota that cannot be trusted, None when it can; a released staleness rule accepts any age."""
    if snapshot is None:
        return Decision(ASK, None, f"quota unknown: no snapshot of {account}", UNKNOWN_RULE)
    age = now - parse_moment(snapshot.ts)
    if age > timedelta(minutes=settings.snapshot_max_age_minutes) and lifted != STALENESS_RULE:
        return Decision(ASK, None, f"quota unknown: the latest snapshot of {account} is from {snapshot.ts} "
                        f"({int(age.total_seconds() // 60)} min old, limit {settings.snapshot_max_age_minutes})",
                        STALENESS_RULE)
    if snapshot.week_pct is None:
        return Decision(ASK, None, f"quota unknown: the latest snapshot of {account} has no weekly figure", UNKNOWN_RULE)
    return None


def _live(pct: float, resets: datetime | None, now: datetime) -> float:
    """The figure of a window: 0 once its reset has passed, as the snapshot predates the reset."""
    return 0.0 if resets is not None and resets <= now else pct


def _retry_at(resets: datetime | None, now: datetime) -> datetime:
    """The reset to wait for; a reset that is unknown or already past means look again at the next check."""
    return resets if resets is not None and resets > now else now


def _five_hour(snapshot: Snapshot, now: datetime, settings: BudgetSettings, lifted: str) -> Decision | None:
    if snapshot.five_pct is None or lifted == FIVE_HOUR_RULE:
        return None
    used = _live(snapshot.five_pct, snapshot.five_resets, now)
    ceiling = FULL_PERCENT - settings.reserve_five_hour_pct
    if used < ceiling:
        return None
    return Decision(DEFER, _retry_at(snapshot.five_resets, now),
                    f"5-hour window {used:.0f}% reaches {ceiling:.0f}% (reserve {settings.reserve_five_hour_pct:.0f}%)",
                    FIVE_HOUR_RULE)


def _night_pacing(conn: sqlite3.Connection, account: str, now: datetime, settings: BudgetSettings, committed: float,
                  urgent: bool, lifted: str) -> Decision | None:
    if urgent or lifted == PACING_RULE or in_night(now, settings.night):
        return None
    projected = projection.project_week(conn, account, now)
    ceiling = FULL_PERCENT - settings.reserve_week_pct
    if projected is None or projected + committed < ceiling:
        return None
    return Decision(DEFER, next_night_start(now, settings.night),
                    f"week projected at {projected:.0f}% by its reset (+{committed:.0f}% held or pending) "
                    f"crosses {ceiling:.0f}%; a flow started with --when-cheap waits for the night {settings.night}",
                    PACING_RULE)


def _bounds(night: str):
    start, end = night.split("-")
    return datetime.strptime(start, "%H:%M").time(), datetime.strptime(end, "%H:%M").time()
