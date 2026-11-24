"""Lane identity: a validated name mapped to its worktree, branch and window marker."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from sakura.lanes.config import LaneGroup

NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")


@dataclass(frozen=True)
class LaneIdentity:
    """Everything a lane command may touch, derived from the group and the lane name."""

    group: str
    name: str
    worktree: Path
    branch: str
    window_marker: str


class LaneNameError(ValueError):
    """Raised when a lane name, or what it maps to, is unsafe to use."""


def resolve(group: LaneGroup, name: str) -> LaneIdentity:
    """Validate `name` and return the lane's identity; raise LaneNameError if unsafe."""
    if not NAME.fullmatch(name):
        raise LaneNameError(f"invalid lane name {name!r}: use 1-40 characters of a-z, 0-9 and - (not first)")
    return LaneIdentity(
        group=group.name,
        name=name,
        worktree=_worktree_under_root(group.worktree_root, name),
        branch=_checked_branch(group.branch.replace("{name}", name)),
        window_marker=f"{group.name}/{name}",
    )


def _worktree_under_root(root: Path, name: str) -> Path:
    resolved_root = root.resolve()
    worktree = (resolved_root / name).resolve()
    if worktree.parent != resolved_root:
        raise LaneNameError(f"worktree for {name!r} resolves outside {resolved_root}: {worktree}")
    return worktree


# What `git check-ref-format --branch` refuses, stated here so naming a lane runs no program: a leading dash, control
# characters, spaces and `~^:?*[\\`, `..`, `@{`, `//`, a component that starts with a dot or ends in `.lock`, a
# trailing slash or dot, and the bare name `@`. A branch this accepts is one Git accepts; the reverse need not hold.
_UNSAFE_BRANCH = re.compile(r"^-|[\x00-\x20\x7f~^:?*\[\\]|\.\.|@\{|//|(^|/)\.|\.lock(/|$)|[/.]$|^@$")


def _checked_branch(branch: str) -> str:
    if not branch or _UNSAFE_BRANCH.search(branch):
        raise LaneNameError(f"invalid branch name {branch!r} from the group's branch template")
    return branch
