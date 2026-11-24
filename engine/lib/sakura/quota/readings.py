"""Live quota readings: a probe for Claude, the Codex rollout logs for Codex (CodexBar only as a fallback).

A reading comes from the archive when its newest snapshot is younger than `max_age`; otherwise the source is
asked. When the source fails the newest snapshot is returned marked stale, so a failing probe never hides an account.
A Codex reading from the logs is as current as the logs can be: its time is the last Codex request, so it is never
stale for being old, only an expired window empties it.
"""

from __future__ import annotations

import os
import sqlite3
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.quota import codex_logs, codexbar, snapshots
from sakura.quota.targets import CLAUDE, Target
from sakura.usage.archive import calibration, quota
from sakura.usage.archive.settings import Source

CODEX = "codex"
PROBE, CODEX_LOG, CODEXBAR = "probe", "codex-log", "codexbar"
DEFAULT_MAX_AGE = timedelta(minutes=15)
Window = codexbar.Window


@dataclass(frozen=True)
class Reading:
    """The quota of one account at `snapshot.ts`; `live` is True when this call asked the source."""

    snapshot: quota.Snapshot
    source: str
    live: bool
    stale: bool

    @property
    def five_hour(self) -> Window:
        return Window(self.snapshot.five_hour_pct, self.snapshot.five_hour_resets_at)

    @property
    def seven_day(self) -> Window:
        return Window(self.snapshot.seven_day_pct, self.snapshot.seven_day_resets_at)


class Collector:
    """Reads the quota of targets; every process and clock is injected."""

    def __init__(self, conn: sqlite3.Connection, run: quota.Runner, clock: Callable[[], datetime], claude_bin: str,
                 codexbar_bin: str = codexbar.DEFAULT_BIN, max_age: timedelta = DEFAULT_MAX_AGE) -> None:
        self._conn, self._run, self._clock = conn, run, clock
        self._claude_bin, self._codexbar_bin, self._max_age = claude_bin, codexbar_bin, max_age

    def read(self, target: Target, refresh: bool = False) -> Reading | None:
        """The reading of `target`; None when it has no snapshot and its source gave nothing."""
        held = snapshots.latest(self._conn, target.key)
        if held and not refresh and self._clock() - calibration.parse_moment(held.ts) <= self._max_age:
            return Reading(held, CODEX_LOG if target.provider == CODEX else PROBE, False, False)
        fresh = self._ask(target)
        if fresh is not None:
            return Reading(fresh[0], fresh[1], True, False)
        return Reading(held, PROBE if target.provider == CLAUDE else CODEX_LOG, False, True) if held else None

    def _ask(self, target: Target) -> tuple[quota.Snapshot, str] | None:
        if target.provider == CLAUDE:
            found = self._probe(target)
            return None if found is None else (found, PROBE)
        return self._codex(target) if target.provider == CODEX else None

    def _codex(self, target: Target) -> tuple[quota.Snapshot, str] | None:
        limits = codex_logs.latest_limits(target.config_dir)
        if limits is not None:
            return quota.Snapshot(target.key, *limits, None), CODEX_LOG
        found = self._codexbar(target) if _is_default_codex(target) else None
        return None if found is None else (found, CODEXBAR)

    def _probe(self, target: Target) -> quota.Snapshot | None:
        source = Source(CLAUDE, target.id, target.config_dir / ADAPTERS[CLAUDE].transcript_subdir, target.config_dir)
        try:
            return quota.probe(source, self._claude_bin, self._run, self._clock)
        except quota.QuotaProbeError:
            return None

    def _codexbar(self, target: Target) -> quota.Snapshot | None:
        with tempfile.TemporaryDirectory(prefix="sakura-quota-") as workdir:
            usage = codexbar.read(CODEX, self._codexbar_bin, self._run, dict(os.environ), Path(workdir))
        if usage is None or usage.seven_day.pct is None:
            return None
        return quota.Snapshot(target.key, self._clock().isoformat(), usage.plan, None, usage.five_hour.pct,
                              usage.five_hour.resets_at, usage.seven_day.pct, usage.seven_day.resets_at, None)


def _is_default_codex(target: Target) -> bool:
    """CodexBar reads the default CODEX_HOME only, so any other Codex directory cannot be asked through it."""
    default = Path(ADAPTERS[CODEX].default_dir).expanduser()
    return target.config_dir.expanduser().resolve() == default.resolve()
