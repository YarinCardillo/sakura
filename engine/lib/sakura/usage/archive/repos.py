"""Repository names of working directories, read from the filesystem only (no subprocess)."""

from __future__ import annotations

import sqlite3
from datetime import datetime, timezone
from pathlib import Path

UPSERT_CWD = """
INSERT INTO cwds (cwd, repo, resolved_at) VALUES (?, ?, ?)
ON CONFLICT (cwd) DO UPDATE SET repo = excluded.repo, resolved_at = excluded.resolved_at
"""


def repo_name(cwd: str) -> str | None:
    """Directory name of the main repository containing `cwd`, following linked worktrees; None if unknown."""
    start = Path(cwd)
    try:
        if not start.is_dir():
            return None
        for directory in (start, *start.parents):
            found = _repo_at(directory)
            if found is not None:
                return found
    except OSError:
        return None
    return None


def resolve_new_cwds(conn: sqlite3.Connection) -> int:
    """Record the repository of every archived cwd; cwds without a repository yet are tried again.

    Returns how many cwds were added or gained a repository.
    """
    known = {cwd for (cwd,) in conn.execute("SELECT cwd FROM cwds")}
    pending = [cwd for (cwd,) in conn.execute(
        "SELECT DISTINCT cwd FROM turns WHERE cwd IS NOT NULL AND cwd NOT IN (SELECT cwd FROM cwds WHERE repo IS NOT NULL)")]
    now = datetime.now(timezone.utc).isoformat()
    changes = [(cwd, repo, now) for cwd in pending if (repo := repo_name(cwd)) or cwd not in known]
    with conn:
        conn.executemany(UPSERT_CWD, changes)
    return len(changes)


def _repo_at(directory: Path) -> str | None:
    marker = directory / ".git"
    if marker.is_dir():
        return directory.name
    if marker.is_file():
        return _worktree_main(directory, marker)
    return None


def _worktree_main(directory: Path, marker: Path) -> str | None:
    text = marker.read_text(encoding="utf-8", errors="replace").strip()
    if not text.startswith("gitdir:"):
        return None
    gitdir = directory / text[len("gitdir:"):].strip()
    try:
        common = (gitdir / (gitdir / "commondir").read_text(encoding="utf-8").strip()).resolve()
    except OSError:
        return None
    return common.parent.name or None
