"""What the contract planner may leave in the task worktree: new flow files only (`flow/flow_paths.py`)."""

from __future__ import annotations

import errno
import hashlib
import os
import stat
from pathlib import Path

from sakura import beneath
from sakura.flow import flow_paths
from sakura.flow.evidence import EXCHANGE_DIR, EXCHANGE_GITIGNORE
from sakura.flow.gitutil import git

INSTALL_ARTIFACTS = (".sakura/git-hooks/", ".claude/settings.local.json", ".codex/hooks.json")
GENERATED_DIRS = {".git", ".venv", "__pycache__", "node_modules", ".pytest_cache", ".mypy_cache", ".ruff_cache"}
ADDED = "A"
UNTRACKED = "??"
# Folders a builder's tools leave behind (bytecode, test and type-check caches, bundler caches); matched as path segments.
TOOL_CACHES = ("__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", "node_modules/.cache")


def uncommitted(path: Path) -> list[str]:
    """Return every changed or untracked path in the worktree, apart from the install artifacts."""
    return [name for _, name in _status_entries(path)]


def uncommitted_beyond_caches(path: Path) -> list[str]:
    """Like `uncommitted`, without the untracked caches a builder's own tools create (`TOOL_CACHES`).

    A tracked file that changed is never a cache, whatever its path.
    """
    return [name for status, name in _status_entries(path) if status != UNTRACKED or not _is_tool_cache(name)]


def _status_entries(path: Path) -> list[tuple[str, str]]:
    """(two-letter status, path) of every changed or untracked path, apart from the install artifacts."""
    entries = git(path, "status", "--porcelain=v1", "-z", "--untracked-files=all").split("\0")
    found, skip_next = [], False
    for entry in entries:
        if skip_next:
            skip_next = False
        elif entry:
            found.append((entry[:2], entry[3:]))
            skip_next = entry[0] in "RC"
    return [(status, name) for status, name in found if not is_install_artifact(name)]


def tracked_changes(path: Path) -> list[str]:
    """Return tracked paths dirty against HEAD, excluding engine installation artifacts."""
    names = git(path, "diff", "--name-only", "--no-renames", "-z", "HEAD").split("\0")
    return sorted(name for name in names if name and not is_install_artifact(name))


def workspace_snapshot(path: Path) -> dict[str, str]:
    """Hash tracked, untracked and non-generated ignored workspace inputs by path."""
    regular = git(path, "ls-files", "--cached", "--others", "--exclude-standard", "-z").split("\0")
    ignored = git(path, "ls-files", "--others", "--ignored", "--exclude-standard", "-z").split("\0")
    engine_owned = _engine_exchange_marker(path)
    names = {name for name in (*regular, *ignored)
             if name and name != engine_owned and not _is_generated(name) and not is_install_artifact(name)}
    return {name: _content_digest(path, name) for name in sorted(names)}


def flow_problems(path: Path, base_commit: str, allowed: list[str]) -> list[str]:
    """`problems` for the contract flows: a new file must pass the host allowlist `allowed` and the hard denials
    (`flow/flow_paths.py`), never the repository's `brakes.test_patterns`. Empty means fine."""
    found = []
    for status, name in _tracked_changes(path, base_commit):
        if status != ADDED:
            found.append(f"existing file {name} was changed ({status}); contract flows may only add files")
        elif flow_paths.refusal(name, allowed):
            found.append(flow_paths.refusal(name, allowed))
    found += [flow_paths.refusal(name, allowed) for name in _untracked(path) if flow_paths.refusal(name, allowed)]
    return found


def added_flow_files(path: Path, base_commit: str, allowed: list[str]) -> list[str]:
    """Every new file that may be a contract flow (`flow_problems`), untracked or added since `base_commit`, sorted;
    nothing is staged."""
    fresh = _untracked(path)
    added = [name for status, name in _tracked_changes(path, base_commit) if status == ADDED]
    return sorted(name for name in {*fresh, *added} if not flow_paths.refusal(name, allowed))


def flow_digest(path: Path, base_commit: str, allowed: list[str]) -> str:
    """One digest of the contract flows in the workspace (every new file `added_flow_files` names, by name and content);
    "" when there are none. What the person's approval of the checks is bound to, with the commands."""
    names = added_flow_files(path, base_commit, allowed)
    if not names:
        return ""
    material = "\0".join(f"{name}\0{_content_digest(path, name)}" for name in names)
    return hashlib.sha256(material.encode()).hexdigest()


def stage(path: Path, base_commit: str, allowed: list[str]) -> list[str]:
    """Stage every new file that may be a contract flow (the host allowlist `allowed` and the hard denials) and return
    all of them (staged now or committed earlier), sorted."""
    fresh = [name for name in _untracked(path) if not flow_paths.refusal(name, allowed)]
    if fresh:
        git(path, "add", "--", *fresh)
    added = [name for status, name in _tracked_changes(path, base_commit)
             if status == ADDED and not flow_paths.refusal(name, allowed)]
    return sorted({*fresh, *added})


def _engine_exchange_marker(path: Path) -> str:
    """`.sakura-role/.gitignore` when the top-level `.sakura-role` is a real folder holding the engine's own
    `.gitignore` (regular file, exact content): the one thing the engine leaves behind after a role run.
    Anything else, under that name or any other, is a workspace change. Empty when it does not apply."""
    folder, marker = path / EXCHANGE_DIR, path / EXCHANGE_DIR / ".gitignore"
    try:
        if not stat.S_ISDIR(os.lstat(folder).st_mode) or not stat.S_ISREG(os.lstat(marker).st_mode):
            return ""
        return f"{EXCHANGE_DIR}/.gitignore" if beneath.read_bytes(path, f"{EXCHANGE_DIR}/.gitignore") == EXCHANGE_GITIGNORE else ""
    except OSError:
        return ""


def _tracked_changes(path: Path, base_commit: str) -> list[tuple[str, str]]:
    lines = git(path, "diff", "--name-status", "--no-renames", "-z", base_commit).split("\0")
    pairs = zip(lines[0::2], lines[1::2])
    return [(status, name) for status, name in pairs if name and not is_install_artifact(name)]


def _untracked(path: Path) -> list[str]:
    names = git(path, "ls-files", "--others", "--exclude-standard", "-z").split("\0")
    return [name for name in names if name and not is_install_artifact(name)]


def is_install_artifact(name: str) -> bool:
    """Whether `name` is a file the engine installs in a worktree (its hooks and their settings), never the lane's own."""
    return any(name == item or (item.endswith("/") and name.startswith(item)) for item in INSTALL_ARTIFACTS)


def _is_tool_cache(name: str) -> bool:
    padded = f"/{name}/"
    return any(f"/{cache}/" in padded for cache in TOOL_CACHES)


def _is_generated(name: str) -> bool:
    return bool(GENERATED_DIRS.intersection(Path(name).parts))


def _content_digest(root: Path, name: str) -> str:
    """The digest of workspace file `name`, a link's target, or a marker: read below `root`, never through a link."""
    try:
        return beneath.digest(root, name)
    except beneath.NotRegular:
        return "not-regular"
    except OSError as error:
        if error.errno != errno.ELOOP:
            return "missing"
    try:
        return "symlink:" + beneath.readlink(root, name)
    except OSError:
        return "missing"
