"""Find candidate account directories by structure alone: stat and list entries, never open a file."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura.accounts.providers import ADAPTERS, SUBSCRIPTION_PROVIDERS
from sakura.accounts.registry import Account, Registry

DEFAULT_ID = "default"
FALLBACK_ID = "account"
MAX_ID_LENGTH = 40
NOT_SLUG = re.compile(r"[^a-z0-9-]")


@dataclass(frozen=True)
class Candidate:
    provider: str
    config_dir: Path
    reason: str


@dataclass(frozen=True)
class Proposal:
    """A candidate with the id `accounts add` would suggest, and the registered account that owns it, if any."""

    candidate: Candidate
    proposed_id: str
    registered_as: str | None


def detect(home: Path, env: Mapping[str, str], dot_directories: bool = True) -> list[Candidate]:
    """Candidate directories per provider: the env var, the default, then dot-directories under `home` by name.

    A directory counts only when it holds the provider's transcript subdirectory. Symlinks are resolved,
    and a directory reached twice is reported once with its first reason. A dot-directory is only a guess
    (another tool can hold a `projects/` or `sessions/` too), so its reason says it needs a check, and
    `dot_directories=False` leaves the guesses out.
    """
    found: dict[tuple[str, Path], Candidate] = {}
    dot_dirs = list_dot_directories(home) if dot_directories else []
    for provider in SUBSCRIPTION_PROVIDERS:
        adapter = ADAPTERS[provider]
        sources = [(f"${adapter.env_var}", env.get(adapter.env_var)),
                   (f"default {adapter.default_dir}", str(_default_dir(provider, home)))]
        sources += [(f"dot-directory with {adapter.transcript_subdir}/ (needs a check)", str(path)) for path in dot_dirs]
        for reason, raw in sources:
            candidate = _candidate(provider, reason, raw)
            if candidate and (provider, candidate.config_dir) not in found:
                found[(provider, candidate.config_dir)] = candidate
    return list(found.values())


def implicit_accounts(env: Mapping[str, str]) -> tuple[Account, ...]:
    """The enabled accounts a host with no configuration runs on, under their proposed ids.

    Only the env var and the default directory of each provider count; dot-directory guesses stay proposals
    of `accounts detect`, so another tool's `projects/` or `sessions/` is never scanned or probed.
    """
    home = Path(env.get("HOME") or os.path.expanduser("~"))
    proposals = propose(detect(home, env, dot_directories=False), Registry((), implicit=True), home)
    return tuple(Account(p.candidate.provider, p.proposed_id, p.proposed_id, p.candidate.config_dir, True, None, (), ())
                 for p in proposals)


def propose(candidates: list[Candidate], registry: Registry, home: Path) -> list[Proposal]:
    """Attach a proposed id and the owning registry account to every candidate.

    A provider's default directory (under `home`) is proposed as `default`, however it was reached.
    The implicit accounts of an unconfigured registry are detection results, not registered ones, and are ignored.
    """
    registered = () if registry.implicit else registry.accounts
    owners = {(a.provider, a.config_dir.resolve()): a.id for a in registered}
    taken = {(a.provider, a.id) for a in registered}
    proposals = []
    for candidate in candidates:
        proposed = _unique(_base_id(candidate, home), candidate.provider, taken)
        taken.add((candidate.provider, proposed))
        owner = owners.get((candidate.provider, candidate.config_dir))
        proposals.append(Proposal(candidate, proposed, owner))
    return proposals


def list_dot_directories(home: Path) -> list[Path]:
    """List immediate dot-directories by name without opening their contents."""
    try:
        with os.scandir(home) as entries:
            return sorted((Path(entry.path) for entry in entries if entry.name.startswith(".") and _is_directory(entry)),
                          key=lambda path: path.name)
    except OSError:
        return []


def _is_directory(entry: os.DirEntry) -> bool:
    # An entry that cannot be statted (symlink loop, permission) is no candidate, never a failure.
    try:
        return entry.is_dir()
    except OSError:
        return False


def _default_dir(provider: str, home: Path) -> Path:
    return (home / ADAPTERS[provider].default_dir.removeprefix("~/")).resolve()


def _candidate(provider: str, reason: str, raw: str | None) -> Candidate | None:
    if not raw:
        return None
    try:
        path = Path(raw).resolve()
        if not (path / ADAPTERS[provider].transcript_subdir).is_dir():
            return None
    except (OSError, RuntimeError):
        # Unreadable (Python 3.12 re-raises EACCES from is_dir) or a symlink loop (RuntimeError before 3.13).
        return None
    return Candidate(provider, path, reason)


def _base_id(candidate: Candidate, home: Path) -> str:
    if candidate.config_dir == _default_dir(candidate.provider, home):
        return DEFAULT_ID
    slug = NOT_SLUG.sub("-", candidate.config_dir.name.lstrip(".").lower()).lstrip("-")[:MAX_ID_LENGTH]
    return slug or FALLBACK_ID


def _unique(base: str, provider: str, taken: set[tuple[str, str]]) -> str:
    proposed, number = base, 1
    while (provider, proposed) in taken:
        number += 1
        suffix = f"-{number}"
        proposed = base[:MAX_ID_LENGTH - len(suffix)] + suffix
    return proposed
