"""Per-worktree installation and removal of kit hooks."""

from __future__ import annotations

import errno
import json
import os
import shlex
import stat
from pathlib import Path

from sakura import beneath, repository
from sakura.flow import gitutil

CLAUDE_SETTINGS = Path(".claude/settings.local.json")
CODEX_HOOKS = Path(".codex/hooks.json")
HOOKS_FOLDER = Path(".sakura/git-hooks")
HOOK_NAMES = ("pre-commit", "pre-push")
# Every path the engine writes into a checkout, and the folders on the way: each must be absent or a real folder
# (a regular file for the configs), in the tracked tree and on disk, never a link (review 14, finding 1).
INSTALL_FOLDERS = (".claude", ".codex", ".sakura", str(HOOKS_FOLDER))
INSTALL_FILES = (str(CLAUDE_SETTINGS), str(CODEX_HOOKS))
LINK_OR_GITLINK = ("120000", "160000")
MAX_CONFIG_BYTES = 1 << 20
CONFIG_MODE = 0o644


class InstallRefused(gitutil.GitError):
    """A checkout the engine will not write its hook files into: a link or a non-regular entry where they go."""


def install(repo: Path) -> None:
    """Install Git and agent hooks for one worktree and exclude its local overlay.

    This writes the repository's Git config (the worktree-config switch and the hook path) and its exclude file:
    the person's own command, never the engine's (see `install_files`).
    """
    hooks = install_files(repo)
    _git(repo, "config", "extensions.worktreeConfig", "true")
    _git(repo, "config", "--worktree", "core.hooksPath", str(hooks))
    _exclude(repo)


def install_files(repo: Path) -> Path:
    """Write the hook links and the agent hook configs inside the worktree and return the hooks folder.

    It changes nothing outside the worktree: no Git config, no exclude file. The engine uses this for its task
    workspaces and hands the hook path to each command through the environment (`GIT_CONFIG_*`). The checkout is
    untrusted (a role may have committed anything), so every write goes through folders opened without following a
    link below `repo`, each config is read bounded from a single-name regular file and replaced by a rename, and a
    checkout with a link or a non-regular entry where these files go is refused (InstallRefused) before anything is
    written.
    """
    _require_plain_install_paths(repo)
    kit = _kit_root()
    try:
        for name in HOOK_NAMES:
            beneath.replace_symlink(repo, f"{HOOKS_FOLDER}/{name}", str(kit / "git-hooks" / name))
        _write_agent_configs(repo, kit)
    except OSError as error:
        if error.errno != errno.ELOOP and not isinstance(error, (beneath.NotRegular, beneath.FileTooLarge)):
            raise
        raise InstallRefused(f"{repo}: the engine writes no hook file through {error}") from error
    return repo / HOOKS_FOLDER


def uninstall(repo: Path) -> None:
    """Remove this worktree's Git hook path without deleting user configuration."""
    _git(repo, "config", "--worktree", "--unset-all", "core.hooksPath", allow_failure=True)


def _exclude(repo: Path) -> None:
    raw = Path(_git(repo, "rev-parse", "--git-path", "info/exclude"))
    path = raw if raw.is_absolute() else repo / raw
    path.parent.mkdir(parents=True, exist_ok=True)
    entry = ".sakura/local.toml"
    lines = path.read_text().splitlines() if path.exists() else []
    if entry not in lines:
        path.write_text("\n".join([*lines, entry, ""]))


def claude_hook_settings() -> dict:
    """The `hooks` object `install` writes into `.claude/settings.local.json`, for roles that pass it inline."""
    return _claude_config(_kit_root())["hooks"]


def _kit_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _claude_config(kit: Path) -> dict:
    command = str(kit / "bin/sakura")
    return {"hooks": {event: [{"matcher": "*", "hooks": [{"type": "command", "command": shlex.join([command, "hook", "claude", event])}]}] for event in ("PreToolUse", "PostToolUse")}}


def _require_plain_install_paths(repo: Path) -> None:
    """Raise InstallRefused when a path the engine writes, or a folder on the way, is a link or not the kind of entry
    it must be, in the tracked tree or on disk."""
    names = (*INSTALL_FOLDERS, *INSTALL_FILES)
    for entry in filter(None, gitutil.git(repo, "ls-files", "-s", "-z", "--", *names).split("\0")):
        name = entry.split("\t", 1)[1]
        if entry.split(" ", 1)[0] in LINK_OR_GITLINK and name in names:
            raise InstallRefused(f"{repo}: {name} is a committed link; the engine writes no hook file through it")
    for name in names:
        try:
            mode = os.lstat(repo / name).st_mode
        except FileNotFoundError:
            continue
        kind, expected = ("folder", stat.S_ISDIR) if name in INSTALL_FOLDERS else ("file", stat.S_ISREG)
        if not expected(mode):
            raise InstallRefused(f"{repo}: {name} is a link or not a plain {kind}; the engine writes no hook file "
                                 "through it")


def _write_agent_configs(repo: Path, kit: Path) -> None:
    command = str(kit / "bin/sakura")
    codex = {"hooks": {event: [{"hooks": [{"type": "command", "command": shlex.join([command, "hook", "codex", event])}]}] for event in ("PreToolUse", "PostToolUse")}}
    _write_json(repo, str(CLAUDE_SETTINGS), _claude_config(kit))
    _write_json(repo, str(CODEX_HOOKS), codex)


def _write_json(repo: Path, relative: str, update: dict) -> None:
    """Merge `update` into the JSON file `relative` below `repo`, read bounded and replaced by a rename."""
    try:
        raw = beneath.read_capped(repo, relative, MAX_CONFIG_BYTES)
    except FileNotFoundError:
        raw = b""
    current = json.loads(raw) if raw else {}
    path = repo / relative
    if not isinstance(current, dict):
        raise ValueError(f"{path} must hold an object")
    hooks = current.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise ValueError(f"{path} hooks must be an object")
    for event, groups in update["hooks"].items():
        existing = hooks.setdefault(event, [])
        if not isinstance(existing, list):
            raise ValueError(f"{path} {event} hooks must be a list")
        for group in groups:
            if group not in existing:
                existing.append(group)
    beneath.write_bytes(repo, relative, (json.dumps(current, indent=2) + "\n").encode(), CONFIG_MODE)


def _git(repo: Path, *args: str, allow_failure: bool = False) -> str:
    try:
        done = gitutil.run(repo, *args, person_config=True)
    except gitutil.GitError as error:
        raise repository.RepositoryError(str(error)) from error
    if done.returncode and not allow_failure:
        raise repository.RepositoryError(done.stderr.strip() or "git command failed")
    return done.stdout.strip()
