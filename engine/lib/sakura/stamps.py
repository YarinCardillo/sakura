"""Test attestations stored outside a worktree."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from sakura import engine_home


def path(repo: Path, tree: str, suite: str, commands: list[str]) -> Path:
    """Return the immutable location of a passing test stamp."""
    return engine_home.kit_dir(repo) / "stamps" / f"{tree}.{suite}.{configuration(commands)}"


def record(repo: Path, tree: str, suite: str, commands: list[str], details: dict | None = None) -> None:
    """Record a passing suite for a Git tree, with optional JSON details."""
    target = path(repo, tree, suite, commands)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(details) + "\n" if details is not None else "passed\n")


def read(repo: Path, tree: str, suite: str, commands: list[str]) -> dict | None:
    """Return the JSON stamp details, or None when the stamp is absent, legacy or unparseable."""
    target = path(repo, tree, suite, commands)
    try:
        details = json.loads(target.read_text())
    except (OSError, ValueError):
        return None
    return details if isinstance(details, dict) else None


def has(repo: Path, tree: str, suite: str, commands: list[str]) -> bool:
    """Return whether a passing suite stamp exists for a tree."""
    return path(repo, tree, suite, commands).is_file()


def configuration(commands: list[str]) -> str:
    """Return a stable digest for the configured suite command list."""
    encoded = json.dumps(commands, separators=(",", ":"), ensure_ascii=True).encode()
    return hashlib.sha256(encoded).hexdigest()
