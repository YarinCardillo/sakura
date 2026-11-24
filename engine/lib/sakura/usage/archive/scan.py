"""Incremental scan of transcript roots into the archive: a byte offset per file, whole lines only."""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable, Iterator
from dataclasses import astuple, dataclass
from pathlib import Path

from sakura.usage.archive import records
from sakura.usage.archive.quota import INSERT_SNAPSHOT, Snapshot
from sakura.usage.archive.records import LimitEvent, RateLimits, Turn
from sakura.usage.archive.settings import CODEX_ACCOUNT_PREFIX, Source

UPSERT_TURN = """
INSERT INTO turns (provider, event_key, account, session_id, agent_id, ts, cwd, branch, model,
  input_tokens, cache_write_tokens, cache_read_tokens, output_tokens, cost_usd)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (provider, event_key) DO UPDATE SET
  input_tokens = MAX(input_tokens, excluded.input_tokens),
  cache_write_tokens = MAX(cache_write_tokens, excluded.cache_write_tokens),
  cache_read_tokens = MAX(cache_read_tokens, excluded.cache_read_tokens),
  output_tokens = MAX(output_tokens, excluded.output_tokens)
"""
INSERT_LIMIT_EVENT = "INSERT OR IGNORE INTO limit_events VALUES (?, ?, ?, ?, ?)"
UPSERT_FILE = """
INSERT INTO files (path, provider, account, offset, size, mtime_ns, cursor) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (path) DO UPDATE SET offset = excluded.offset, size = excluded.size,
  mtime_ns = excluded.mtime_ns, cursor = excluded.cursor
"""

LineReader = Callable[[bytes, int, dict, str, str], Turn | None]


@dataclass(frozen=True)
class ScanStats:
    files_seen: int
    files_read: int
    turns_written: int


def scan_source(conn: sqlite3.Connection, source: Source) -> ScanStats:
    """Archive the new whole lines of every transcript under the source root."""
    seen = read = written = 0
    for path in transcripts(source.root):
        seen += 1
        count = _scan_file(conn, source, path)
        if count is not None:
            read, written = read + 1, written + count
    return ScanStats(seen, read, written)


def transcripts(root: Path) -> list[Path]:
    """Every transcript file under `root`, sorted; an unreadable root yields none."""
    found: list[Path] = []
    try:
        for path in root.rglob("*.jsonl"):
            found.append(path)
    except OSError:
        pass
    return sorted(found)


def _scan_file(conn: sqlite3.Connection, source: Source, path: Path) -> int | None:
    """Turns written from the file, or None when it was skipped as unchanged or unreadable."""
    try:
        stat = path.stat()
    except OSError:
        return None
    stored = conn.execute("SELECT offset, size, mtime_ns, cursor FROM files WHERE path = ?", (str(path),)).fetchone()
    if stored and (stored[1], stored[2]) == (stat.st_size, stat.st_mtime_ns):
        return None
    offset, cursor = (stored[0], json.loads(stored[3])) if stored else (0, {})
    if stat.st_size < offset:
        offset, cursor = 0, {}
    try:
        turns, events, limits, offset = _read_turns(path, source, offset, cursor)
    except OSError:
        return None
    with conn:
        conn.executemany(UPSERT_TURN, turns)
        conn.executemany(INSERT_LIMIT_EVENT, events)
        if limits:
            conn.execute(INSERT_SNAPSHOT, astuple(Snapshot(CODEX_ACCOUNT_PREFIX + source.account, *limits, None)))
        conn.execute(UPSERT_FILE, (str(path), source.provider, source.account, offset, stat.st_size,
                                   stat.st_mtime_ns, json.dumps(cursor)))
    return len(turns)


def _read_turns(path: Path, source: Source, offset: int,
                cursor: dict) -> tuple[list[Turn], list[LimitEvent], RateLimits | None, int]:
    """Turns, Claude limit events and the last Codex plan limits of the new lines, and the new offset."""
    reader: LineReader = _READERS[source.provider]
    merged: dict[str, Turn] = {}
    events: list[LimitEvent] = []
    limits: RateLimits | None = None
    with open(path, "rb") as handle:
        handle.seek(offset)
        for line, line_offset in _whole_lines(handle, offset):
            turn = reader(line, line_offset, cursor, source.account, path.stem)
            if turn:
                known = merged.get(turn.event_key)
                merged[turn.event_key] = records.merge(known, turn) if known else turn
            elif source.provider == "claude" and (event := records.limit_event(line, source.account)):
                events.append(event)
            if source.provider == "codex":
                limits = records.codex_rate_limits(line) or limits
            offset = line_offset + len(line)
    return list(merged.values()), events, limits, offset


def _whole_lines(handle, start: int) -> Iterator[tuple[bytes, int]]:
    position = start
    for line in handle:
        if not line.endswith(b"\n"):
            return
        yield line, position
        position += len(line)


def _claude_line(line: bytes, _offset: int, _cursor: dict, account: str, stem: str) -> Turn | None:
    return records.claude_turn(line, account, stem)


_READERS: dict[str, LineReader] = {"claude": _claude_line, "codex": records.codex_turn}
