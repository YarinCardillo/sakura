"""Bounded parsing cache for small regular files."""

from __future__ import annotations

import copy
import errno
import os
import stat
import tomllib
from collections.abc import Callable
from functools import lru_cache
from pathlib import Path
from typing import BinaryIO, TypeVar

_MAX_BYTES = 64 * 1024
_MAX_ENTRIES = 256
_Value = TypeVar("_Value")
_Identity = tuple[int, int, int, int, int]


def load(path: Path, parse: Callable[[BinaryIO], _Value], info: os.stat_result | None = None) -> _Value:
    """Parse a file, reusing the result while its identity, mtime, ctime and size match."""
    info = path.stat() if info is None else info
    if _cacheable(info):
        return _reuse(path, parse, info)
    with path.open("rb") as stream:
        return parse(stream)


def small_bytes(path: Path, info: os.stat_result | None = None) -> bytes | None:
    """Return cached bytes for a small regular file, or None for callers that stream larger files."""
    info = path.stat() if info is None else info
    return _reuse(path, _read_bytes, info) if _cacheable(info) else None


def load_toml(path: Path, info: os.stat_result | None = None) -> dict:
    """Return a private copy of the parsed TOML layer."""
    return copy.deepcopy(load(path, _parse_toml, info))


def _parse_toml(stream: BinaryIO) -> dict:
    text = stream.read().decode().replace("\r\n", "\n").replace("\r", "\n")
    return tomllib.loads(text)


def _read_bytes(stream: BinaryIO) -> bytes | None:
    data = stream.read(_MAX_BYTES + 1)
    return data if len(data) <= _MAX_BYTES else None


def _cacheable(info: os.stat_result) -> bool:
    return stat.S_ISREG(info.st_mode) and info.st_size <= _MAX_BYTES


def _identity(info: os.stat_result) -> _Identity:
    return info.st_dev, info.st_ino, info.st_mtime_ns, info.st_ctime_ns, info.st_size


class _ChangedDuringRead(Exception):
    def __init__(self, value: _Value) -> None:
        self.value = value


def _reuse(path: Path, parse: Callable[[BinaryIO], _Value], info: os.stat_result) -> _Value:
    try:
        return _cached(path, parse, _identity(info))
    except _ChangedDuringRead as changed:
        return changed.value


@lru_cache(maxsize=_MAX_ENTRIES)
def _cached(path: Path, parse: Callable[[BinaryIO], _Value], identity: _Identity) -> _Value:
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NONBLOCK), "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise OSError(errno.EINVAL, "file changed type while being opened", path)
        value = parse(stream)
        after = os.fstat(stream.fileno())
    # Match the descriptor so a path replacement cannot stamp another file's contents with this identity.
    if _identity(before) != identity or _identity(after) != identity:
        raise _ChangedDuringRead(value)
    return value
