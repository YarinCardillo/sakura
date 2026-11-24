"""Text view of the status document."""

from __future__ import annotations

from datetime import datetime

from sakura.usage.archive import calibration
from sakura.usage.archive.report import format_table

HEADER = ("account", "provider", "plan", "5h", "5h resets", "week", "week resets", "at reset", "next round",
          "exhausts", "conf", "source")
MINUTES_PER_HOUR = 60
SECONDS_PER_MINUTE = 60
UNKNOWN = "-"


def status_text(document: dict) -> str:
    """The accounts as an aligned table, times relative to `generated_at`."""
    now = calibration.parse_moment(document["generated_at"])
    rows = [_row(account, now) for account in document["accounts"]]
    return format_table(HEADER, rows) if rows else "no accounts found"


def _row(account: dict, now: datetime) -> tuple[str, ...]:
    five, seven, cast = account["five_hour"], account["seven_day"], account["forecast"]
    name = account["label"] + ("*" if account["stale"] else "")
    return (name, account["provider"], account["plan"] or UNKNOWN, _pct(five["pct"]), _in(five["resets_at"], now),
            _pct(seven["pct"]), _in(seven["resets_at"], now), _pct(cast["seven_day_pct_at_reset"]),
            _pct(cast["next_round_pct"]), _in(cast["exhausts_at"], now), cast["confidence"],
            account["source"] or UNKNOWN)


def _pct(value: float | None) -> str:
    return UNKNOWN if value is None else f"{value:.0f}%"


def _in(moment: str | None, now: datetime) -> str:
    """`in 34h20m` for a future moment, `-` for none."""
    if moment is None:
        return UNKNOWN
    minutes = int((calibration.parse_moment(moment) - now).total_seconds() // SECONDS_PER_MINUTE)
    if minutes < MINUTES_PER_HOUR:
        return f"in {max(minutes, 0)}m"
    return f"in {minutes // MINUTES_PER_HOUR}h{minutes % MINUTES_PER_HOUR:02d}m"
