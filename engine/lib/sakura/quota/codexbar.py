"""CodexBar as a read-only quota source: `codexbar usage --json --provider <name>`.

Only window percentages, reset times and the plan name are kept; the account identity CodexBar also prints
(email, organisation, ids) is dropped unread.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from sakura.usage.archive import quota, records

DEFAULT_BIN = "codexbar"
FIVE_HOUR_MINUTES = 300
SEVEN_DAY_MINUTES = 10080
WINDOW_KEYS = ("primary", "secondary", "tertiary")


@dataclass(frozen=True)
class Window:
    """One limit window as a percentage (0 to 100) and its reset time (ISO UTC)."""

    pct: float | None
    resets_at: str | None


@dataclass(frozen=True)
class Usage:
    plan: str | None
    five_hour: Window
    seven_day: Window


EMPTY_WINDOW = Window(None, None)


def read(provider: str, binary: str, run: quota.Runner, env: dict[str, str], cwd: Path) -> Usage | None:
    """The usage CodexBar reports for `provider`, None when it is missing, fails or prints nothing usable."""
    argv = [binary, "usage", "--json", "--provider", provider]
    done = run(argv, env, cwd)
    return parse(done.stdout, provider) if done.returncode == 0 else None


def parse(stdout: str | None, provider: str) -> Usage | None:
    """The `provider` entry of CodexBar's JSON output as a `Usage`, None when it is not there."""
    try:
        entries = json.loads(stdout or "")
    except ValueError:
        return None
    for entry in entries if isinstance(entries, list) else []:
        usage = entry.get("usage") if isinstance(entry, dict) and entry.get("provider") == provider else None
        if isinstance(usage, dict):
            return _usage(usage)
    return None


def _usage(usage: dict) -> Usage:
    by_minutes = {}
    for key in WINDOW_KEYS:
        raw = usage.get(key)
        if isinstance(raw, dict):
            by_minutes[raw.get("windowMinutes")] = _window(raw)
    plan = usage.get("loginMethod")
    return Usage(plan if isinstance(plan, str) and plan else None, by_minutes.get(FIVE_HOUR_MINUTES, EMPTY_WINDOW),
                 by_minutes.get(SEVEN_DAY_MINUTES, EMPTY_WINDOW))


def _window(raw: dict) -> Window:
    used = raw.get("usedPercent")
    pct = float(used) if isinstance(used, (int, float)) and not isinstance(used, bool) else None
    resets = raw.get("resetsAt")
    return Window(pct, resets if isinstance(resets, str) and resets else records.epoch_iso(resets))
