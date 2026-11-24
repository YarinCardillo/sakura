"""The latest Codex plan limits, read from the rollout logs under a Codex home (`<home>/sessions`).

Codex prints its 5-hour and weekly limits on every `token_count` line, so the newest such line is the account's
current reading. Only the limit fields are read (`records.codex_rate_limits`), nothing else of the log.
"""

from __future__ import annotations

import heapq
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.usage.archive import records

NEWEST_FILES = 6
TAIL_BYTES = 512 * 1024
ROLLOUT_PATTERN = "rollout-*.jsonl"


def latest_limits(config_dir: Path) -> records.RateLimits | None:
    """The newest limits line among the most recently written rollouts of the Codex home, None without one."""
    sessions = config_dir / ADAPTERS["codex"].transcript_subdir
    try:
        files = heapq.nlargest(NEWEST_FILES, sessions.rglob(ROLLOUT_PATTERN), key=_mtime)
    except OSError:
        return None
    found = [limits for path in files if (limits := _last_limits(path)) is not None]
    return max(found, key=lambda limits: limits.ts, default=None)


def _last_limits(path: Path) -> records.RateLimits | None:
    try:
        with open(path, "rb") as handle:
            handle.seek(0, 2)
            handle.seek(max(handle.tell() - TAIL_BYTES, 0))
            lines = handle.read().splitlines()
    except OSError:
        return None
    return next((limits for line in reversed(lines) if (limits := records.codex_rate_limits(line))), None)


def _mtime(path: Path) -> float:
    try:
        return path.stat().st_mtime
    except OSError:
        return 0.0
