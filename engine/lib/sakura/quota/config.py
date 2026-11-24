"""Owner rules of the quota service, read from the optional host file `~/.config/sakura/quota.toml`.

Which accounts may serve which repository, and which model a role gets, are the person's rules, so they live in
this file and not in code. Without the file every discovered account serves every repository.

    port = 8799
    codex_model = ""            # empty: Codex keeps the model of its own config

    [roles]                     # model[:effort] for a role, replacing the built-in tier
    planner = "opus"

    [[projects]]                # first matching project wins; accounts are listed by preference
    name = "work"
    paths = ["~/work/app"]
    claude = ["work", "personal"]
    codex = ["personal"]

    [default]                   # repositories no project matches
    claude = ["personal"]
    codex = ["personal"]
"""

from __future__ import annotations

import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_PATH = "~/.config/sakura/quota.toml"
DEFAULT_PORT = 8799
MAX_PORT = 65535
ROLES = ("builder", "reviewer", "explorer", "planner", "evaluator")
KNOWN_KEYS = frozenset({"port", "codex_model", "roles", "projects", "default"})
PROJECT_KEYS = frozenset({"name", "paths", "claude", "codex"})


class QuotaConfigError(ValueError):
    """The quota config is malformed; the message names the offending key."""


@dataclass(frozen=True)
class Allowed:
    """Account ids allowed per provider, by preference; None means every discovered account."""

    claude: tuple[str, ...] | None = None
    codex: tuple[str, ...] | None = None


@dataclass(frozen=True)
class Project:
    name: str
    paths: tuple[Path, ...]
    allowed: Allowed


@dataclass(frozen=True)
class QuotaConfig:
    port: int = DEFAULT_PORT
    codex_model: str = ""
    roles: Mapping[str, str] = field(default_factory=dict)
    projects: tuple[Project, ...] = ()
    default: Allowed = Allowed()

    def allowed_for(self, repo: Path) -> tuple[str, Allowed]:
        """The name of the project that owns `repo` (`default` when none) and the accounts it allows."""
        resolved = _resolve(repo)
        for project in self.projects:
            if any(resolved == root or root in resolved.parents for root in project.paths):
                return project.name, project.allowed
        return "default", self.default


def default_path(home: Path | None = None) -> Path:
    return (home or Path.home()) / DEFAULT_PATH.removeprefix("~/")


def load(path: Path, home: Path | None = None) -> QuotaConfig:
    """The config at `path`, the defaults when the file does not exist. Raises QuotaConfigError."""
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return QuotaConfig()
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise QuotaConfigError(f"{path}: cannot read quota config: {error}") from error
    unknown = raw.keys() - KNOWN_KEYS
    if unknown:
        raise QuotaConfigError(f"{path}: unknown key {sorted(unknown)[0]}")
    base = home or Path.home()
    return QuotaConfig(_port(raw.get("port", DEFAULT_PORT)), _text("codex_model", raw.get("codex_model", "")),
                       _roles(raw.get("roles", {})), _projects(raw.get("projects", []), base),
                       _allowed("default", raw.get("default", {})))


def _projects(raw: object, home: Path) -> tuple[Project, ...]:
    if not isinstance(raw, list):
        raise QuotaConfigError("projects must be an array of tables")
    return tuple(_project(entry, home) for entry in raw)


def _project(entry: object, home: Path) -> Project:
    if not isinstance(entry, dict) or entry.keys() - PROJECT_KEYS:
        raise QuotaConfigError(f"a project may only have the keys {', '.join(sorted(PROJECT_KEYS))}")
    name = _text("projects.name", entry.get("name"))
    paths = _strings(f"projects.{name}.paths", entry.get("paths"))
    return Project(name, tuple(_resolve(_expand(p, home)) for p in paths), _allowed(f"projects.{name}", entry))


def _allowed(where: str, table: object) -> Allowed:
    if not isinstance(table, dict):
        raise QuotaConfigError(f"{where} must be a table")
    pick = lambda provider: _strings(f"{where}.{provider}", table[provider]) if provider in table else None
    return Allowed(pick("claude"), pick("codex"))


def _roles(raw: object) -> dict[str, str]:
    if not isinstance(raw, dict) or not set(raw) <= set(ROLES):
        raise QuotaConfigError(f"roles may only name {', '.join(ROLES)}")
    return {role: _text(f"roles.{role}", value) for role, value in raw.items()}


def _port(value: object) -> int:
    if type(value) is not int or not 0 < value <= MAX_PORT:
        raise QuotaConfigError(f"port must be an integer from 1 to {MAX_PORT}; got {value!r}")
    return value


def _strings(key: str, value: object) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise QuotaConfigError(f"{key} must be a list of non-empty strings")
    return tuple(value)


def _text(key: str, value: object) -> str:
    if not isinstance(value, str) or (not value and key != "codex_model"):
        raise QuotaConfigError(f"{key} must be a non-empty string")
    return value


def _expand(value: str, home: Path) -> Path:
    return home / value[2:] if value.startswith("~/") else Path(value)


def _resolve(path: Path) -> Path:
    try:
        return path.resolve()
    except (OSError, RuntimeError):
        return path
