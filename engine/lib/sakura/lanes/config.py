"""Lane groups read from ~/.config/sakura/lanes.toml."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path

DEFAULT_PATH = "~/.config/sakura/lanes.toml"
TEXT_FIELDS = ("session", "branch", "base", "up", "down", "status", "agent")
PATH_FIELDS = ("repo", "worktree_root")


@dataclass(frozen=True)
class LaneGroup:
    """One lane group: where its repository and worktrees live and how its lanes run."""

    name: str
    repo: Path
    session: str
    worktree_root: Path
    branch: str
    base: str
    setup: tuple[str, ...]
    up: str
    down: str
    status: str
    agent: str


class LaneConfigError(ValueError):
    """Raised when lanes.toml is missing, malformed or a group cannot be chosen."""


def load(path: Path | str | None = None) -> dict[str, LaneGroup]:
    """Return the lane groups of `path` (default: the personal lanes.toml) by name."""
    source = Path(path or DEFAULT_PATH).expanduser()
    try:
        table = tomllib.loads(source.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise LaneConfigError(f"cannot read lane config {source}: {error}") from error
    return {name: _parse_group(name, fields, source) for name, fields in table.items()}


def select(groups: dict[str, LaneGroup], name: str | None) -> LaneGroup:
    """Return the named group, or the only one when `name` is omitted."""
    if name is None:
        if len(groups) == 1:
            return next(iter(groups.values()))
        raise LaneConfigError(f"{len(groups)} lane groups configured; pass --group ({', '.join(sorted(groups))})")
    if name not in groups:
        raise LaneConfigError(f"unknown lane group {name!r} ({', '.join(sorted(groups))})")
    return groups[name]


def _parse_group(name: str, fields: object, source: Path) -> LaneGroup:
    if not isinstance(fields, dict):
        raise LaneConfigError(f"{source}: [{name}] must be a table")
    values: dict = {"name": name}
    for key in PATH_FIELDS:
        values[key] = Path(_require(fields, key, str, name)).expanduser()
    for key in TEXT_FIELDS:
        values[key] = _require(fields, key, str, name)
    setup = _require(fields, "setup", list, name)
    if not all(isinstance(item, str) for item in setup):
        raise LaneConfigError(f"lane group {name}: setup must be a list of strings")
    values["setup"] = tuple(str(Path(item).expanduser()) if item.startswith("~") else item for item in setup)
    return LaneGroup(**values)


def _require(fields: dict, key: str, kind: type, group: str):
    value = fields.get(key)
    if not isinstance(value, kind):
        raise LaneConfigError(f"lane group {group}: {key} is required and must be a {kind.__name__}")
    return value
