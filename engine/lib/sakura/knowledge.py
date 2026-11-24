"""Knowledge folders: what the person wants every agent to read, shared across projects or kept for one.

Two host settings name them: `knowledge.shared` (personal layer) and `knowledge.project` (host layers). Every cage
may read them and none may write them: a change reaches them only through a proposal the person accepts
(`proposals.py`). A folder is refused, naming the setting, when an agent could use it to reach something it must not:
it is, holds or lies inside the home, a secret, an engine folder or a place a role can write, it holds a credential
file, or a link in it leaves it.
"""

from __future__ import annotations

import fnmatch
import os
from collections.abc import Iterable, Mapping
from pathlib import Path

SHARED, PROJECT = "shared", "project"
NAMES = (SHARED, PROJECT)
MAX_ENTRIES = 20000
# Files named like these hold credentials wherever they lie in a folder: environment files, package and Git login
# files, SSH and TLS keys.
CREDENTIAL_NAMES = (".env*", ".npmrc", ".pypirc", ".netrc", ".git-credentials", "id_rsa*", "id_dsa*", "id_ecdsa*",
                    "id_ed25519*", "*.pem", "*.p12", "*.pfx", "*.key")


class KnowledgeRefused(ValueError):
    """Raised, with a one-line reason naming the setting, when a knowledge folder is not acceptable."""


def configured(values: Mapping, home: Path, forbidden: Iterable[Path]) -> dict[str, Path]:
    """The folders `values["knowledge"]` names, as real paths by name (`shared`, `project`); a name left empty is absent.

    `forbidden` is every secret, engine and writable folder (real paths): no folder may be, hold or lie inside one.
    Raises KnowledgeRefused.
    """
    named = values.get("knowledge") or {}
    avoided = tuple(forbidden)
    return {name: _checked(f"knowledge.{name}", named[name], home, avoided) for name in NAMES if named.get(name)}


def _checked(key: str, raw: object, home: Path, forbidden: tuple[Path, ...]) -> Path:
    folder = Path(os.path.realpath(_expanded(key, raw, home)))
    if not folder.is_dir():
        raise KnowledgeRefused(f"{key}: {folder} is not a folder")
    real_home = Path(os.path.realpath(home))
    if folder == real_home or folder in real_home.parents:
        raise KnowledgeRefused(f"{key}: {folder} is the home folder or holds it")
    if any(_overlaps(folder, other) for other in forbidden):
        raise KnowledgeRefused(f"{key}: {folder} is, holds or lies inside a secret or engine folder, or a place a "
                               f"role can write")
    _require_plain_content(key, folder)
    return folder


def _expanded(key: str, raw: object, home: Path) -> Path:
    if not isinstance(raw, str):
        raise KnowledgeRefused(f"{key} must be an absolute path")
    path = home / raw[2:] if raw.startswith("~/") else Path(raw)
    if not path.is_absolute():
        raise KnowledgeRefused(f"{key} must be an absolute path (a leading ~/ is allowed); got {raw!r}")
    return path


def _overlaps(folder: Path, other: Path) -> bool:
    return folder == other or folder in other.parents or other in folder.parents


def _require_plain_content(key: str, folder: Path) -> None:
    """Refuse a folder with a credential file in it, or a link that leaves it; the walk is bounded."""
    count = 0
    for current, names, files in os.walk(folder):
        for name in (*names, *files):
            count += 1
            if count > MAX_ENTRIES:
                raise KnowledgeRefused(f"{key}: {folder} holds more than {MAX_ENTRIES} entries")
            path = Path(current) / name
            if name in files and any(fnmatch.fnmatch(name, pattern) for pattern in CREDENTIAL_NAMES):
                raise KnowledgeRefused(f"{key}: {path} looks like a credential file")
            if path.is_symlink() and not _stays_inside(folder, path):
                raise KnowledgeRefused(f"{key}: the link {path} leaves the folder")


def _stays_inside(folder: Path, link: Path) -> bool:
    target = Path(os.path.realpath(link))
    return target == folder or folder in target.parents
