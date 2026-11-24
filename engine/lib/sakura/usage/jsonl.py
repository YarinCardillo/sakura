"""Tolerant readers shared by the provider modules: a bad line or file is skipped, never raised."""

import json
from collections.abc import Iterator
from datetime import datetime, timezone
from pathlib import Path


def read_objects(path: Path) -> Iterator[dict]:
    """Yield the JSON objects of a line-delimited file, skipping anything else."""
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                obj = _parse(line)
                if isinstance(obj, dict):
                    yield obj
    except OSError:
        return


def count(value: object) -> int | None:
    """A non-negative integer count, or None for any other shape."""
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        return value
    return None


def number(value: object) -> float | None:
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return float(value)
    return None


def from_epoch(value: object) -> datetime | None:
    seconds = number(value)
    if seconds is None:
        return None
    try:
        return datetime.fromtimestamp(seconds, timezone.utc)
    except (OverflowError, OSError, ValueError):
        return None


def from_iso(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def _parse(line: str) -> object:
    try:
        return json.loads(line)
    except (ValueError, RecursionError):
        return None
