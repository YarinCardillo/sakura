"""Cockpit trust settings, read only from the person's own file and checked before anything runs."""

from __future__ import annotations

import os
import re
import stat
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

DEFAULT_PATH = "~/.config/sakura/cockpit.toml"
DEFAULT_PORT = 8787
MAX_PORT = 65535
KNOWN_KEYS = ("port", "allow_logins", "allow_nodes", "tailscale")
LOGIN_PATTERN = re.compile(r"[A-Za-z0-9._@+-]{1,64}")
NODE_PATTERN = re.compile(r"[A-Za-z0-9._-]{1,64}")
OTHERS_WRITE = stat.S_IWGRP | stat.S_IWOTH
ROOT_UID = 0


class CockpitRefused(Exception):
    """Raised when the cockpit must not start or serve: untrusted settings, helper or address."""


@dataclass(frozen=True)
class CockpitSettings:
    """Who may use the cockpit and which Tailscale CLI answers for it."""

    port: int
    allow_logins: frozenset[str]
    allow_nodes: frozenset[str]
    tailscale: Path


def load(path: Path | None = None, *, registered_repos: Iterable[Path]) -> CockpitSettings:
    """Read and validate the trusted cockpit file; the default is under the home the process sees.

    Raises CockpitRefused for an unreadable file, an unknown key, a malformed value, or a Tailscale CLI that is
    not an absolute, executable, owner-controlled file outside every registered repository.
    """
    source = Path(path) if path is not None else Path(DEFAULT_PATH).expanduser()
    data = _read(source)
    unknown = sorted(set(data) - set(KNOWN_KEYS))
    if unknown:
        raise CockpitRefused(f"{source}: unknown setting {unknown[0]!r}")
    return CockpitSettings(
        port=_port(data.get("port", DEFAULT_PORT)),
        allow_logins=_names(data, "allow_logins", LOGIN_PATTERN),
        allow_nodes=_names(data, "allow_nodes", NODE_PATTERN),
        tailscale=_tailscale(data.get("tailscale"), tuple(registered_repos)),
    )


def trusted_tailscale(value: str, registered_repos: Iterable[Path]) -> Path:
    """`value` as the Tailscale CLI the cockpit may run: an absolute, executable, owner-controlled file outside every repository.
    Raises CockpitRefused otherwise."""
    return _tailscale(value, tuple(registered_repos))


def _read(path: Path) -> dict:
    try:
        return tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise CockpitRefused(f"cannot read {path}: {error}") from error


def _port(value: object) -> int:
    if type(value) is not int or not 1 <= value <= MAX_PORT:
        raise CockpitRefused(f"port must be an integer from 1 to {MAX_PORT}; got {value!r}")
    return value


def _names(data: dict, key: str, pattern: re.Pattern) -> frozenset[str]:
    values = data.get(key, [])
    if not isinstance(values, list) or any(
            not isinstance(item, str) or not pattern.fullmatch(item) for item in values):
        raise CockpitRefused(f"{key} must be a list of names matching {pattern.pattern}")
    return frozenset(values)


def _tailscale(value: object, repos: tuple[Path, ...]) -> Path:
    if not isinstance(value, str) or not os.path.isabs(value):
        raise CockpitRefused(f"tailscale must be an absolute path; got {value!r}")
    path = Path(value)
    _require_trusted_file(path)
    for candidate in (Path(os.path.abspath(path)), path.resolve()):
        if any(candidate.is_relative_to(Path(repo).resolve()) for repo in repos):
            raise CockpitRefused(f"tailscale {path} is inside a registered repository")
    return path


def _require_trusted_file(path: Path) -> None:
    try:
        info = path.stat()
    except OSError:
        info = None
    if info is None or not stat.S_ISREG(info.st_mode) or not os.access(path, os.X_OK):
        raise CockpitRefused(f"tailscale {path} is not an executable file")
    if info.st_mode & OTHERS_WRITE:
        raise CockpitRefused(f"tailscale {path} is writable by its group or by others")
    if info.st_uid not in (ROOT_UID, os.geteuid()):
        raise CockpitRefused(f"tailscale {path} is owned by neither root nor the current user")
