"""Git repository paths used by the kit."""

from __future__ import annotations

import os
from pathlib import Path

from sakura.flow import gitutil

GIT_MARKER = ".git"
# Each resolved folder's answer, valid while the identity of its `.git` entry is the one it was read under.
_common_dirs: dict[str, tuple[tuple[int, int, int, int], Path]] = {}


class RepositoryError(RuntimeError):
    """Raised when a required Git repository property is unavailable."""


def root(path: Path = Path.cwd()) -> Path:
    """Return the Git worktree root containing ``path``."""
    return Path(_git(path, "rev-parse", "--show-toplevel"))


def common_dir(repo: Path) -> Path:
    """Return the absolute common Git directory for a worktree.

    Git is asked once per resolved folder and process: a snapshot resolves the same few folders dozens of times. The
    answer is dropped when the folder's `.git` entry is replaced or rewritten, and a refusal is never remembered.
    """
    key = os.path.realpath(repo)
    identity = _marker_identity(key)
    cached = _common_dirs.get(key)
    if cached and identity and cached[0] == identity:
        return cached[1]
    found = Path(_git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    if identity:
        _common_dirs[key] = (identity, found)
    return found


def branch(repo: Path) -> str:
    """Return the current branch, or an empty string when detached."""
    return _git(repo, "branch", "--show-current")


def _marker_identity(folder: str) -> tuple[int, int, int, int] | None:
    """Device, inode, modification time and size of the folder's `.git` entry; None when it has none (a bare repository,
    a subfolder), which are never remembered."""
    try:
        info = os.lstat(os.path.join(folder, GIT_MARKER))
    except OSError:
        return None
    return info.st_dev, info.st_ino, info.st_mtime_ns, info.st_size


def _git(repo: Path, *args: str) -> str:
    try:
        done = gitutil.run(repo, *args)
    except gitutil.GitError as error:
        raise RepositoryError(str(error)) from error
    if done.returncode:
        raise RepositoryError(done.stderr.strip() or "git command failed")
    return done.stdout.strip()
