"""The accounts to read: the registry's enabled accounts plus the Orca-managed Claude profiles on this Mac.

Discovery is by structure only (stat and list, never open a credential file). A directory the registry does not
own is a target with `registered=False`; an Orca profile that turns out to be an account already listed is merged
with it later, from the quota it reports.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.accounts.registry import Registry

ORCA_PROFILES = "Library/Application Support/orca/claude-profiles/*/home"
CLAUDE_MARKER = ".claude.json"
ORCA_ID_PREFIX = "orca-"
ORCA_ID_LENGTH = 8
CLAUDE = "claude"


@dataclass(frozen=True)
class Target:
    """One account directory to read; `key` is its snapshot key in the usage archive."""

    provider: str
    id: str
    label: str
    config_dir: Path
    registered: bool

    @property
    def key(self) -> str:
        return ADAPTERS[self.provider].snapshot_key(self.id)


def discover(registry: Registry, home: Path, extra_dirs: Iterable[Path] = ()) -> list[Target]:
    """Registered enabled accounts whose directory exists, then unregistered Claude directories (Orca profiles
    under `home`, then `extra_dirs`), each directory once."""
    targets = [Target(a.provider, a.id, a.label, a.config_dir, True)
               for a in registry.accounts if a.enabled and a.config_dir.is_dir()]
    seen = {(t.provider, _real(t.config_dir)) for t in targets}
    for directory in [*_orca_profiles(home), *extra_dirs]:
        if (CLAUDE, _real(directory)) not in seen and _is_claude_dir(directory):
            seen.add((CLAUDE, _real(directory)))
            targets.append(_unregistered(directory, targets))
    return targets


def _orca_profiles(home: Path) -> list[Path]:
    return sorted(home.glob(ORCA_PROFILES))


def _is_claude_dir(directory: Path) -> bool:
    return (directory / CLAUDE_MARKER).is_file()


def _unregistered(directory: Path, taken: list[Target]) -> Target:
    base = ORCA_ID_PREFIX + (directory.parent.name or directory.name)[:ORCA_ID_LENGTH]
    used = {target.id for target in taken if target.provider == CLAUDE}
    account_id, number = base, 1
    while account_id in used:
        number += 1
        account_id = f"{base}-{number}"
    return Target(CLAUDE, account_id, account_id, directory, False)


def _real(path: Path) -> Path:
    try:
        return path.resolve()
    except (OSError, RuntimeError):
        return path
