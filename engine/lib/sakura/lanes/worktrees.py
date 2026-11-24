"""Git and command plumbing for lanes: create, inspect and remove a lane's worktree."""

from __future__ import annotations

import os
import shlex
import subprocess
from pathlib import Path

from sakura.flow import gitutil
from sakura.flow.process import child_environment
from sakura.flow.render import strip_controls
from sakura.lanes.config import LaneGroup
from sakura.lanes.errors import LaneConflict, LaneError
from sakura.lanes.identity import LaneIdentity

OUTPUT_TAIL = 400
SETUP_MARKER = "sakura-setup-done"


def run_command(command: str, cwd: Path, what: str) -> None:
    """Run a configured command line (split like a shell would, but without one) in `cwd`; LaneError if it fails.

    The command sees no tmux identity and a private tmux directory: it is repository-configured and must not reach the person's server.
    """
    argv = shlex.split(command)
    if not argv:
        raise LaneError(f"{what} command is empty")
    try:
        done = subprocess.run(argv, cwd=cwd, env=child_environment(os.environ), capture_output=True, text=True,
                              errors="replace", stdin=subprocess.DEVNULL, check=False)
    except OSError as error:
        raise LaneError(f"{what} command {argv[0]!r} could not start: {strip_controls(str(error))}") from error
    if done.returncode:
        raise LaneError(f"{what} command {argv[0]!r} exited {done.returncode}: {_tail(done.stderr or done.stdout)}")


def create(identity: LaneIdentity, group: LaneGroup, from_ref: str | None) -> bool:
    """Make the lane's worktree if it is absent; True when created now, False when it was already there."""
    if identity.worktree.exists():
        _require_registered(identity, group)
        return False
    if _branch_exists(group.repo, identity.branch):
        git(group.repo, "worktree", "add", str(identity.worktree), identity.branch)
    else:
        start = _resolve_start(group, from_ref or group.base)
        git(group.repo, "worktree", "add", "-b", identity.branch, str(identity.worktree), start)
    return True


def setup_done(identity: LaneIdentity) -> bool:
    """Return whether the lane's setup finished: its marker file sits in the worktree's git dir."""
    return _setup_marker(identity).exists()


def mark_setup_done(identity: LaneIdentity) -> None:
    """Record that every setup command succeeded, so a later open does not run them again."""
    _setup_marker(identity).touch()


def rollback(identity: LaneIdentity, group: LaneGroup, branch_was_new: bool) -> None:
    """Undo `create` for a worktree made moments ago whose setup failed."""
    git(group.repo, "worktree", "remove", "--force", str(identity.worktree))
    if branch_was_new:
        git(group.repo, "branch", "-D", identity.branch)


def branch_exists(group: LaneGroup, identity: LaneIdentity) -> bool:
    """Return whether the lane's branch already exists in the group repository."""
    return _branch_exists(group.repo, identity.branch)


def remove(identity: LaneIdentity, group: LaneGroup) -> None:
    """Remove the clean worktree (git itself refuses when it is not clean)."""
    git(group.repo, "worktree", "remove", str(identity.worktree))


def pending_work(identity: LaneIdentity, group: LaneGroup, check_merged: bool) -> list[str]:
    """Return why closing would lose work (empty when safe); a git failure raises instead of passing."""
    if not identity.worktree.exists():
        return []
    reasons = []
    if git(identity.worktree, "status", "--porcelain").stdout.strip():
        reasons.append("uncommitted changes in the worktree")
    reasons += _unpushed(identity, group)
    if check_merged and not _is_ancestor(group.repo, identity.branch, group.base):
        reasons.append(f"branch {identity.branch} is not merged into {group.base}")
    return reasons


def git(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess:
    """Run `git -C cwd args` without a shell; with `check`, a failure raises LaneError carrying git's message."""
    try:
        done = gitutil.run(cwd, *args)
    except (OSError, gitutil.GitError) as error:
        raise LaneError(f"git {args[0]} could not run: {strip_controls(str(error))}") from error
    if check and done.returncode:
        raise LaneError(f"git {args[0]} failed: {_tail(done.stderr or done.stdout)}")
    return done


def _setup_marker(identity: LaneIdentity) -> Path:
    return Path(git(identity.worktree, "rev-parse", "--absolute-git-dir").stdout.strip()) / SETUP_MARKER


def _unpushed(identity: LaneIdentity, group: LaneGroup) -> list[str]:
    upstream = git(identity.worktree, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}", check=False)
    reference = upstream.stdout.strip() if upstream.returncode == 0 else group.base
    ahead = git(identity.worktree, "rev-list", "--count", f"{reference}..HEAD").stdout.strip()
    if ahead == "0":
        return []
    return [f"{ahead} commit(s) not on {reference}"]


def _is_ancestor(repo: Path, branch: str, base: str) -> bool:
    done = git(repo, "merge-base", "--is-ancestor", branch, base, check=False)
    if done.returncode not in (0, 1):
        raise LaneError(f"git merge-base failed: {_tail(done.stderr)}")
    return done.returncode == 0


def _branch_exists(repo: Path, branch: str) -> bool:
    return git(repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}", check=False).returncode == 0


def _resolve_start(group: LaneGroup, ref: str) -> str:
    if ref.startswith("-"):
        raise LaneError(f"invalid --from {ref!r}: a ref cannot start with -")
    done = git(group.repo, "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}", check=False)
    if done.returncode:
        raise LaneError(f"unknown --from ref {strip_controls(ref)!r} in {group.repo}")
    return done.stdout.strip()


def _require_registered(identity: LaneIdentity, group: LaneGroup) -> None:
    listing = git(group.repo, "worktree", "list", "--porcelain").stdout.splitlines()
    known = {Path(row.removeprefix("worktree ")).resolve() for row in listing if row.startswith("worktree ")}
    if identity.worktree not in known:
        raise LaneConflict(f"{identity.worktree} exists but is not a worktree of {group.repo}")


def _tail(text: str) -> str:
    return strip_controls(text.strip())[-OUTPUT_TAIL:]
