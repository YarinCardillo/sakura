"""What an agent feed needs to know about a log beyond its new entries: whether it is still the log the cursor read, the
whole text of the message being written, and long text cut into parts (`flow/agent_feed.py`)."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path

from sakura.flow import feed, logread, run_log

HEAD_BYTES = 128
TAIL_BYTES = 256
TEXT_PART_CHARS = feed.MAX_TEXT_CHARS
READ_BYTES = 1 << 20
LINE_READ_GROWTH = 4


def describe(log: Path, head_length: int | None, position: int) -> list:
    """What identifies the log the cursor has read up to `position`: `[inode, head length, head hash, tail length, tail
    hash]`, the tail being the up to `TAIL_BYTES` bytes just before `position`. The head is as long as it was when first
    seen (`head_length`; None for the first look), so a log that merely grew keeps its head."""
    fd = os.open(log, logread.OPEN)
    try:
        info = os.fstat(fd)
        length = head_length if head_length is not None else min(info.st_size, HEAD_BYTES)
        tail = min(position, TAIL_BYTES)
        head_hash = _hash(os.pread(fd, length, 0))
        tail_hash = _hash(os.pread(fd, tail, position - tail))
    finally:
        os.close(fd)
    return [info.st_ino, length, head_hash, tail, tail_hash]


def is_other(log: Path, kept: list | None, position: int) -> bool:
    """Whether the log is not the one `kept` (an earlier `describe`, taken at `position`) was made of: another file at the
    path, one rewritten in place (its head, or the bytes just before `position`, differ) or one cut short of them. A rewrite
    of earlier bytes that leaves both the head and the last `TAIL_BYTES` before `position` as they were cannot be told apart
    without reading the whole prefix at every poll, which the cursor is there to avoid."""
    if not kept:
        return False
    if os.stat(log).st_size < position:
        return True
    current = describe(log, kept[1], position)
    return current[:3] != kept[:3] or (len(kept) > 3 and current[3:] != kept[3:])


def _hash(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()[:16]


def live_text(log: Path, start: int, size: int) -> str:
    """The text of the message being written, whose first byte is at `start`, however many deltas it has: the log is read
    on in chunks until its end."""
    tail = run_log.LiveTail(start)
    position = start
    while position < size:
        moved = position
        for offset, end, event in run_log.lines(chunk(log, position), position):
            tail.take(event, offset, end)
            moved = end
        if moved == position:
            break
        position = moved
    return "".join(tail.parts)


def chunk(log: Path, start: int) -> bytes:
    """The next bytes of the log; more than `READ_BYTES` only when one line is longer than that."""
    limit = READ_BYTES
    while True:
        data = logread.read_range(log, start, limit)
        if b"\n" in data or len(data) < limit or limit >= logread.OUTPUT_CAP_BYTES:
            return data
        limit *= LINE_READ_GROWTH


def text_parts(text: str) -> list[str]:
    """`text` cut into parts of at most `TEXT_PART_CHARS` characters (none for an empty text)."""
    return [text[index:index + TEXT_PART_CHARS] for index in range(0, len(text), TEXT_PART_CHARS)]
