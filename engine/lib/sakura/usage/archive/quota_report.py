"""The `sakura usage quota` view: latest snapshots, plan periods, calibration ratios and limit events."""

from __future__ import annotations

import sqlite3
import statistics
from collections import defaultdict
from datetime import datetime, time, timedelta, timezone

from sakura.accounts import labels
from sakura.accounts.registry import Registry
from sakura.usage.archive import calibration
from sakura.usage.archive import settings as settings_module
from sakura.usage.archive.report import format_table
from sakura.usage.archive.settings import ExtraUsage, Settings

FIVE_HOUR = "five_hour"
FIVE_HOURS = timedelta(hours=5)
WEEK = timedelta(days=7)
EVENT_DAYS_BEFORE = 2
EVENT_DAYS_AFTER = 1
SNAPSHOT_FIELDS = ("account", "ts", "subscription_type", "status", "five_hour_pct", "five_hour_resets_at",
                   "seven_day_pct", "seven_day_resets_at")
LATEST_SNAPSHOTS = f"""
SELECT {', '.join(SNAPSHOT_FIELDS)} FROM quota_snapshots s
WHERE ts = (SELECT MAX(ts) FROM quota_snapshots WHERE account = s.account) ORDER BY account
"""


def build(conn: sqlite3.Connection, settings: Settings) -> dict:
    """Every fact the quota view shows, as plain data (the JSON output and the source of the text)."""
    analysis = calibration.analyze(conn, settings.plans)
    return {
        "snapshots": [dict(zip(SNAPSHOT_FIELDS, row)) for row in conn.execute(LATEST_SNAPSHOTS)],
        "plans": [{"account": p.account, "plan": p.plan, "since": p.since.isoformat(), "estimated": p.estimated}
                  for p in sorted(settings.plans, key=lambda p: (p.account, p.since))],
        "ratios": [ratio._asdict() for ratio in analysis.ratios],
        "unexplained": [{"account": account, "plan": plan, "intervals": count}
                        for (account, plan), count in sorted(analysis.unexplained.items())],
        "limit_events": _limit_events(conn, settings.plans),
        "extra_usage": [_extra_usage(conn, purchase) for purchase in settings.extra_usage],
    }


def render(data: dict, registry: Registry = labels.NO_ACCOUNTS) -> str:
    """The aligned text sections of `build` output; accounts are shown by their label from `registry`."""
    def name(account_key: str) -> str:
        return labels.display_name(account_key, registry)

    sections = [
        _section("snapshots", ("account", "taken", "plan", "status", "week %", "week resets", "5h %", "5h resets"),
                 [(name(s["account"]), s["ts"], _text(s["subscription_type"]), _text(s["status"]),
                   _text(s["seven_day_pct"]), _text(s["seven_day_resets_at"]), _text(s["five_hour_pct"]),
                   _text(s["five_hour_resets_at"]))
                  for s in data["snapshots"]]),
        _section("plan periods", ("account", "plan", "since", "note"),
                 [(name(p["account"]), p["plan"], p["since"][:10], "estimated" if p["estimated"] else "")
                  for p in data["plans"]]),
        _section("calibration", ("account", "plan", "class", "weighted per 1%", "intervals", "% covered"),
                 [(name(r["account"]), r["plan"], r["model_class"], f"{r['weighted_per_pct']:.1f}", str(r["intervals"]),
                   f"{r['pct']:.1f}") for r in data["ratios"]]),
        _section("unexplained rises (no local tokens)", ("account", "plan", "intervals"),
                 [(name(u["account"]), u["plan"], str(u["intervals"])) for u in data["unexplained"]]),
        _section("limit events (weighted tokens before the first hit of each window)",
                 ("account", "plan", "window", "windows", "median", "min", "max"),
                 [(name(e["account"]), e["plan"], e["window"], str(e["windows"]), f"{e['median']:.0f}", f"{e['min']:.0f}",
                   f"{e['max']:.0f}") for e in data["limit_events"]]),
        _section("extra usage", ("account", "date", "amount", "note", "nearest limit event", "local turns that day"),
                 [(name(x["account"]), x["date"], f"{x['amount']:.2f} {x['currency']}", x["note"],
                   _nearest_text(x["nearest_limit_event"]), str(x["local_turns"])) for x in data["extra_usage"]]),
    ]
    return "\n\n".join(sections)


def _section(title: str, header: tuple[str, ...], rows: list[tuple[str, ...]]) -> str:
    return f"{title}\n" + (format_table(header, rows) if rows else "none")


def _nearest_text(event: dict | None) -> str:
    return "none" if event is None else f"{event['ts']} {event['window']}"


def _text(value: object) -> str:
    return "-" if value is None else str(value)


def _limit_events(conn: sqlite3.Connection, plans: tuple[settings_module.Plan, ...]) -> list[dict]:
    """Per account, plan and window kind: the weighted tokens used before the first hit of each distinct window.

    A limit window is (account, window, resets_at); a hit without a reset time is a window of its own.
    """
    indexes: dict[str, calibration.TurnIndex] = {}
    grouped: dict[tuple[str, str, str], list[dict]] = defaultdict(list)
    for account, window, resets_at, first_hit in _first_hits(conn):
        index = indexes.setdefault(account, calibration.TurnIndex(conn, account))
        tokens = sum(index.between(_window_start(window, first_hit, resets_at), first_hit).values())
        plan = settings_module.plan_at(plans, account, first_hit) or calibration.UNKNOWN_PLAN
        grouped[(account, plan, window)].append(
            {"first_hit": first_hit.isoformat(), "resets_at": resets_at, "tokens": tokens})
    return [_limit_summary(*key, detail) for key, detail in sorted(grouped.items())]


def _first_hits(conn: sqlite3.Connection) -> list[tuple[str, str, str | None, datetime]]:
    """(account, window, resets_at, first hit time) per distinct limit window, in time order."""
    first: dict[tuple, datetime] = {}
    for index, (account, ts, window, resets_at) in enumerate(
            conn.execute("SELECT account, ts, window, resets_at FROM limit_events ORDER BY ts")):
        key = (account, window, resets_at if resets_at is not None else f"no reset {index}")
        first.setdefault(key, calibration.parse_moment(ts))
    return [(account, window, None if str(resets).startswith("no reset ") else resets, moment)
            for (account, window, resets), moment in first.items()]


def _limit_summary(account: str, plan: str, window: str, detail: list[dict]) -> dict:
    tokens = [window_detail["tokens"] for window_detail in detail]
    return {"account": account, "plan": plan, "window": window, "windows": len(detail),
            "median": statistics.median(tokens), "min": min(tokens), "max": max(tokens), "detail": detail}


def _window_start(window: str, first_hit: datetime, resets_at: str | None) -> datetime:
    """Where the limit window began: its reset time minus its length, or the first hit minus its length."""
    length = FIVE_HOURS if window == FIVE_HOUR else WEEK
    return (calibration.parse_moment(resets_at) if resets_at else first_hit) - length


def _extra_usage(conn: sqlite3.Connection, purchase: ExtraUsage) -> dict:
    """One purchase with the closest limit event around it and the account's local turns that UTC day."""
    turns = conn.execute("SELECT COUNT(*) FROM turns WHERE provider = 'claude' AND account = ?"
                         " AND strftime('%Y-%m-%d', ts) = ?", (purchase.account, purchase.on.isoformat())).fetchone()[0]
    return {"account": purchase.account, "date": purchase.on.isoformat(), "amount": purchase.amount,
            "currency": purchase.currency, "note": purchase.note,
            "nearest_limit_event": _nearest_event(conn, purchase), "local_turns": turns}


def _nearest_event(conn: sqlite3.Connection, purchase: ExtraUsage) -> dict | None:
    """The account's limit event nearest to midday of the purchase date, within the allowed day range."""
    first, last = (purchase.on + timedelta(days=offset) for offset in (-EVENT_DAYS_BEFORE, EVENT_DAYS_AFTER))
    midday = datetime.combine(purchase.on, time(12), timezone.utc)
    found = [(abs(calibration.parse_moment(ts) - midday), ts, window) for ts, window in conn.execute(
        "SELECT ts, window FROM limit_events WHERE account = ? ORDER BY ts", (purchase.account,))
        if first <= calibration.parse_moment(ts).date() <= last]
    if not found:
        return None
    _, ts, window = min(found)
    return {"ts": ts, "window": window}
