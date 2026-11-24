"""A numbered log of JSON lines that several processes append to without losing or doubling a line.

Every line is `{"seq": n, ...}` with `n` counting from 1. An append takes an exclusive lock on `<path>.lock`, cuts a
last line a crash left unfinished, and writes the whole line with one call, so a reader sees whole lines only.
"""

from __future__ import annotations

import fcntl
import json
import os
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

FILE_MODE = 0o600


class LogFull(ValueError):
    """An append would take the log past the entries or bytes its caller allows."""


class LogBroken(ValueError):
    """A log larger than its readers parse whole: it is never read shortened, as a tail left out loses lines."""


@dataclass(frozen=True)
class Allowance:
    """What one class of a log's writers may hold, counted apart from the other classes: entries and bytes on disk."""

    entries: int
    bytes: int


def append(path: Path, record: dict, *, max_entries: int | None = None, max_bytes: int | None = None,
           room: Callable[[], None] | None = None) -> int:
    """Append `record` as the next numbered line and return its number (1 for the first). `path`'s folder must exist.

    With `max_entries` or `max_bytes` (the whole file's size after the line), an append that would pass either writes
    nothing and raises LogFull. `room` runs under the lock before the line is written, so a check of the record's own
    allowance (`require_room`) and the append are one step; whatever it raises refuses the append.
    """
    lock = path.with_name(path.name + ".lock")
    with os.fdopen(os.open(lock, os.O_RDWR | os.O_CREAT, FILE_MODE), "r+") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        number = _cut_unfinished_line(path) + 1
        line = json.dumps({"seq": number, **record}, separators=(",", ":")) + "\n"
        if room is not None:
            room()
        _require_room(path, number, len(line.encode()), max_entries, max_bytes)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, FILE_MODE)
        with os.fdopen(descriptor, "ab") as stream:
            stream.write(line.encode())
            stream.flush()
            os.fsync(stream.fileno())
    return number


def read(path: Path, after: int = 0, max_bytes: int | None = None) -> list[dict]:
    """The entries numbered above `after`, oldest first; lines that do not parse are skipped, nothing is created.

    With `max_bytes` only the first that many bytes of the file are parsed, so a read costs no more than the cap
    however large the file is.
    """
    return [entry for entry in _entries(path, max_bytes) if entry["seq"] > after]


def read_whole(path: Path, read_bytes: int, after: int = 0) -> list[dict]:
    """`read` of a log of at most `read_bytes` bytes; raises LogBroken for a larger one, which is never read shortened."""
    _require_whole(path, read_bytes)
    return read(path, after, max_bytes=read_bytes)


def require_room(path: Path, record: dict, same_class: Callable[[dict], bool], allowance: Allowance,
                 read_bytes: int) -> None:
    """Raise LogFull when appending `record` would take the lines of its class (the entries `same_class` holds for)
    past `allowance`, LogBroken for a log over `read_bytes`. The caller holds a lock every writer of the log takes,
    so the count and the append are one step."""
    entries = read_whole(path, read_bytes)
    own = [entry for entry in entries if same_class(entry)]
    used = sum(line_bytes(entry) for entry in own)
    added = line_bytes({"seq": max((entry["seq"] for entry in entries), default=0) + 1, **record})
    if len(own) + 1 > allowance.entries or used + added > allowance.bytes:
        raise LogFull(f"{len(own)} entries and {used} bytes of this kind; the limits are {allowance.entries} and "
                      f"{allowance.bytes}")


def line_bytes(entry: dict) -> int:
    """What `entry` takes on disk as a line of the log (compact JSON and a newline, as `append` writes it)."""
    return len(json.dumps(entry, separators=(",", ":")).encode()) + 1


def last_seq(path: Path) -> int:
    """The number of the last entry, 0 for a missing or empty log."""
    return max((entry["seq"] for entry in _entries(path)), default=0)


def _cut_unfinished_line(path: Path) -> int:
    """Drop bytes after the last newline and return the highest entry number left."""
    try:
        data = path.read_bytes()
    except FileNotFoundError:
        return 0
    complete = data[:data.rfind(b"\n") + 1]
    if len(complete) != len(data):
        os.truncate(path, len(complete))
    return max((entry["seq"] for entry in _parse(complete)), default=0)


def _require_room(path: Path, number: int, added: int, max_entries: int | None, max_bytes: int | None) -> None:
    if max_entries is not None and number > max_entries:
        raise LogFull(f"the log holds {number - 1} entries; the limit is {max_entries}")
    if max_bytes is not None:
        size = path.stat().st_size if path.exists() else 0
        if size + added > max_bytes:
            raise LogFull(f"the log holds {size} bytes; the limit is {max_bytes}")


def _require_whole(path: Path, read_bytes: int) -> None:
    try:
        size = path.stat().st_size
    except FileNotFoundError:
        return
    if size > read_bytes:
        raise LogBroken(f"its history is {size} bytes, more than the {read_bytes} read whole")


def _entries(path: Path, max_bytes: int | None = None) -> list[dict]:
    try:
        with open(path, "rb") as stream:
            return read_stream(stream, max_bytes)
    except FileNotFoundError:
        return []


def read_stream(stream: BinaryIO, max_bytes: int | None = None) -> list[dict]:
    """Parse complete numbered entries, up to the existing byte cap when provided."""
    data = stream.read() if max_bytes is None else stream.read(max_bytes)
    return _parse(data[:data.rfind(b"\n") + 1])


def _parse(data: bytes) -> list[dict]:
    entries = []
    for line in data.splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if isinstance(entry, dict) and isinstance(entry.get("seq"), int):
            entries.append(entry)
    return entries
