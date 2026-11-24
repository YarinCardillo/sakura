"""The engine's read-only checkout of a committed revision, where the person's `sakura stack up|down` runs.

`stack.up` and `stack.down` run outside every sandbox (Docker needs its socket), and what they do is decided by files
in the repository (compose files, scripts, a Makefile). They are the person's own action: no role, tool or request
reaches them. The command runs in a checkout of the revision the person named that the engine makes in its own home,
with the tracked files read-only, so what runs is that committed revision and nothing a workspace holds besides it.
The checkout stays until the next run replaces it, so the stack's bind mounts keep their files; its folder has the
repository's name, so a compose project keeps the name it has in the repository. A revision with a committed symlink
that resolves outside the checkout is refused.
"""

from __future__ import annotations

import os
import shutil
import stat
from pathlib import Path

from sakura import engine_home
from sakura.flow import gitutil

CHECKOUT_DIR_NAME = "stack"


class StackCheckoutError(RuntimeError):
    """The revision may not run: a committed symlink of it resolves outside the checkout."""


def checkout_dir(repo: Path) -> Path:
    """Where the stack command runs: the engine's checkout of the revision, named like the repository."""
    return engine_home.project_dir(repo) / CHECKOUT_DIR_NAME / repo.name


def checkout(repo: Path, revision: str) -> Path:
    """Replace the engine's stack checkout of `repo` with a detached checkout of `revision`, tracked files read-only.

    Raises StackCheckoutError when a symlink of the checkout resolves outside it.
    """
    path = checkout_dir(repo)
    if path.exists():
        try:
            gitutil.git(repo, "worktree", "remove", "--force", str(path))
        except gitutil.GitError:
            shutil.rmtree(path, ignore_errors=True)
            gitutil.git(repo, "worktree", "prune")
    engine_home.ensure_private(path.parent)
    gitutil.git(repo, "-c", "core.hooksPath=/dev/null", "worktree", "add", "--detach", str(path), revision)
    root = os.path.realpath(path)
    for name in gitutil.git(path, "ls-files", "-z").split("\0"):
        target = path / name
        if name and target.is_symlink() and not _inside(os.path.realpath(target), root):
            raise StackCheckoutError(f"{name} in the checkout of {revision[:12]} resolves outside it; the stack does not run")
        if name and target.is_file() and not target.is_symlink():
            target.chmod(target.stat().st_mode & ~(stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH))
    return path


def _inside(path: str, root: str) -> bool:
    return path == root or path.startswith(root + os.sep)
