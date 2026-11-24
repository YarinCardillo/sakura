"""`sakura usage backfill-codex`: archive every Codex rate-limit sample still on disk in the rollouts.

The archive scan keeps only the last sample of each newly read chunk, so the weekly trend of a Codex account
would start on the day the archive began. A rollout holds every sample; this reads them with
`records.codex_rate_limits` only (nothing else of a line is read or stored) and inserts what the archive lacks.
"""

from __future__ import annotations

import sqlite3
import sys
from collections.abc import Iterator
from contextlib import closing
from dataclasses import astuple, dataclass
from pathlib import Path

from sakura.usage.archive import quota, records, scan, store
from sakura.usage.archive.records import RateLimits
from sakura.usage.archive.settings import CODEX_ACCOUNT_PREFIX, Settings, Source


@dataclass(frozen=True)
class AccountScan:
    """What the rollouts of one Codex account hold: samples by timestamp, and the rollouts the turns scan lags on."""

    account: str
    rollouts: int
    samples: dict[str, RateLimits]
    unread: int


def run(settings: Settings, write: bool) -> int:
    """Print per enabled Codex account what the rollouts hold against the archive; with `write` insert the new
    samples in one transaction under the archive lock. Exit 1 when the lock is held or a dry run has no archive."""
    sources = sorted((source for source in settings.sources if source.provider == "codex"),
                     key=lambda source: source.account)
    if write:
        return _write(settings, sources)
    if not settings.db.exists():
        print(f"no usage archive at {settings.db}", file=sys.stderr)
        return 1
    with closing(_open_read_only(settings.db)) as conn:
        _report(conn, sources)
    return 0


def _write(settings: Settings, sources: list[Source]) -> int:
    with store.archive_lock(settings.db) as held:
        if not held:
            print("usage archive is running, try again when it is done", file=sys.stderr)
            return 1
        with closing(store.open_db(settings.db)) as conn:
            fresh = _report(conn, sources)
            before = conn.total_changes
            with conn:
                conn.executemany(quota.INSERT_SNAPSHOT, fresh)
            print(f"inserted {conn.total_changes - before}")
    return 0


def _open_read_only(path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True, timeout=store.BUSY_TIMEOUT_MS / 1000)


def _report(conn: sqlite3.Connection, sources: list[Source]) -> list[tuple]:
    """Print the summary line then the token-check line of every account; return the snapshot rows to insert."""
    scans = [_scan_account(conn, source) for source in sources]
    fresh: list[tuple] = []
    for found in scans:
        new = _new_samples(conn, found)
        print(_summary(found, len(new)))
        fresh.extend(astuple(quota.Snapshot(found.account, *sample, None)) for sample in new)
    for found in scans:
        print(f"{found.account}: {found.unread} rollouts not fully read into turns")
    return fresh


def _scan_account(conn: sqlite3.Connection, source: Source) -> AccountScan:
    paths = scan.transcripts(source.root)
    samples: dict[str, RateLimits] = {}
    unread = 0
    for path in paths:
        size = _size(path)
        if size is None:
            continue
        unread += _is_unread(conn, path, size)
        for limits in _samples(path):
            samples.setdefault(limits.ts, limits)
    return AccountScan(CODEX_ACCOUNT_PREFIX + source.account, len(paths), samples, unread)


def _size(path: Path) -> int | None:
    try:
        return path.stat().st_size
    except OSError:
        return None


def _is_unread(conn: sqlite3.Connection, path: Path, size: int) -> bool:
    stored = conn.execute("SELECT offset FROM files WHERE path = ?", (str(path),)).fetchone()
    return stored is None or stored[0] < size


def _samples(path: Path) -> Iterator[RateLimits]:
    """The rate limits of every whole line of the rollout; a line still being written is left for a later run."""
    try:
        with open(path, "rb") as handle:
            for line in handle:
                if not line.endswith(b"\n"):
                    return
                if limits := records.codex_rate_limits(line):
                    yield limits
    except OSError:
        return


def _new_samples(conn: sqlite3.Connection, found: AccountScan) -> list[RateLimits]:
    present = {ts for (ts,) in conn.execute("SELECT ts FROM quota_snapshots WHERE account = ?", (found.account,))}
    return [sample for ts, sample in sorted(found.samples.items()) if ts not in present]


def _summary(found: AccountScan, new: int) -> str:
    stamps = sorted(found.samples)
    earliest, latest = (stamps[0], stamps[-1]) if stamps else ("none", "none")
    return (f"{found.account}: {found.rollouts} rollouts, {len(stamps)} samples, {new} new, "
            f"{len(stamps) - new} present, earliest {earliest}, latest {latest}")
