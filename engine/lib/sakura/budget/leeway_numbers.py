"""An account's limits as Leeway reports them, in the shape the budget guard reads: one reader of usage for every app.

Leeway's `status` gives each visible account its weekly and five-hour percentages, their resets and the time of the
reading; the guard judges the reserve on those, so it never disagrees with the numbers the person sees in the apps.
"""

from __future__ import annotations

from collections.abc import Sequence

from sakura.budget.guard import Snapshot
from sakura.usage.archive.calibration import parse_moment


def row_of(accounts: Sequence[dict], name: str) -> dict | None:
    """The status row of the account `name`: by id, else by label when exactly one account has it."""
    by_id = [row for row in accounts if row["id"] == name]
    by_label = [row for row in accounts if row["label"] == name]
    return by_id[0] if by_id else (by_label[0] if len(by_label) == 1 else None)


def snapshot_of(accounts: Sequence[dict], name: str) -> Snapshot | None:
    """The guard's snapshot for account `name`, or None when Leeway has no reading of it (the archive then answers)."""
    row = row_of(accounts, name)
    if row is None or not row.get("as_of"):
        return None
    week, five = row["seven_day"], row["five_hour"]
    return Snapshot(row["as_of"], week["pct"], _moment(week["resets_at"]), five["pct"], _moment(five["resets_at"]))


def _moment(text: str | None):
    return parse_moment(text) if text else None
