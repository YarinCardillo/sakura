"""Bounded reads of run logs, and the cap on how much a run may write.

A role's output lands in a log the engine reads back afterwards. The role chooses how much it prints and, if
it reaches the log's folder, what sits at the log's path, so the engine caps what a run may write
(OUTPUT_CAP_BYTES, enforced by `process.monitored_run`) and reads a log through a descriptor opened without
following links or blocking, only if it is a regular file, and never more than READ_LIMIT_BYTES of its tail.
"""

from __future__ import annotations

import os
import stat
from pathlib import Path

OUTPUT_CAP_BYTES = 64 * 1024 * 1024  # total bytes one run may write before the engine terminates it
READ_LIMIT_BYTES = OUTPUT_CAP_BYTES  # the most a read-back returns; a log over it yields its last bytes
CHUNK_BYTES = 1024 * 1024
OPEN = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK  # a FIFO opens at once instead of waiting for a writer


class UnreadableLog(Exception):
    """The path is a link, a FIFO, a folder or anything but a regular file."""


def read_log(path: Path, limit: int | None = None) -> str:
    """The text of `path`, at most its last `limit` bytes (READ_LIMIT_BYTES by default); undecodable bytes are replaced.

    Raises FileNotFoundError when it is absent and UnreadableLog when it is not a regular file.
    """
    limit = READ_LIMIT_BYTES if limit is None else limit
    fd = _open_regular(path)
    try:
        return _tail(fd, os.fstat(fd).st_size, limit).decode(errors="replace")
    finally:
        os.close(fd)


def read_head(path: Path, limit: int) -> str:
    """The first `limit` bytes of `path`, read the same bounded way as `read_log`."""
    fd = _open_regular(path)
    try:
        return os.pread(fd, limit, 0).decode(errors="replace")
    finally:
        os.close(fd)


def read_range(path: Path, start: int, limit: int) -> bytes:
    """At most `limit` bytes of `path` from byte `start` on, read the same bounded way as `read_log`; empty when `start`
    is past the end."""
    fd = _open_regular(path)
    try:
        return os.pread(fd, limit, start)
    finally:
        os.close(fd)


def size_of(path: Path) -> int:
    """The size of the regular file `path` in bytes, read the same bounded way as `read_log`."""
    fd = _open_regular(path)
    try:
        return os.fstat(fd).st_size
    finally:
        os.close(fd)


def _open_regular(path: Path) -> int:
    try:
        fd = os.open(path, OPEN)
    except FileNotFoundError:
        raise
    except OSError as error:
        raise UnreadableLog(f"{path} is not a regular file ({error.strerror})") from None
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise UnreadableLog(f"{path} is not a regular file")
    return fd


def _tail(fd: int, size: int, limit: int) -> bytes:
    """The last `limit` bytes of the first `size` bytes; a log that grows meanwhile is not followed."""
    start = max(0, size - limit)
    chunks = []
    while start + sum(map(len, chunks)) < size:
        chunk = os.pread(fd, min(CHUNK_BYTES, size - start - sum(map(len, chunks))), start + sum(map(len, chunks)))
        if not chunk:
            break
        chunks.append(chunk)
    return b"".join(chunks)
