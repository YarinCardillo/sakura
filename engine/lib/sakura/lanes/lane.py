"""Open and close a manual lane: a worktree plus a tmux window with an agent pane and a stack pane."""

from __future__ import annotations

import contextlib
import fcntl
import shlex
from typing import Callable, Iterator

from sakura.flow.tmux import PANE_MARKER_KEY, Pane, TmuxClient, Window
from sakura.lanes import worktrees
from sakura.lanes.config import LaneGroup
from sakura.lanes.errors import LaneBusy, LaneConflict, LaneError
from sakura.lanes.identity import LaneIdentity

LANE_MARKER_KEY = "@sakura-lane"
SESSION_MARKER_KEY = "@sakura-lane-group"
AGENT_PANE = "agent"
STACK_PANE = "stack"
_STACK_SHELL = 'exec "${SHELL:-sh}"'
LAUNCH_LOCK_NAME = ".{group}.launch.lock"

__all__ = ["LaneBusy", "LaneConflict", "LaneError", "agent_panes", "done", "marked_windows", "open", "preflight"]


def open(identity: LaneIdentity, group: LaneGroup, from_ref: str | None, client: TmuxClient) -> None:  # noqa: A001
    """Make sure the lane's worktree and marked window exist and select the window; a second call changes nothing.

    Raises LaneConflict when a window with the lane's name was not created by sakura for this lane.
    """
    with _launch_lock(group):
        existing = _find_window(client, group, identity)
        if existing:
            client.select_window(existing.id)
            return
        _ensure_worktree(identity, group, from_ref)
        window = _build_window(client, identity, group)
        client.select_window(window.id)


def preflight(identity: LaneIdentity, group: LaneGroup, remove: bool) -> list[str]:
    """Return the reasons closing the lane would lose work; empty means it is safe. Changes nothing."""
    return worktrees.pending_work(identity, group, check_merged=remove)


def done(identity: LaneIdentity, group: LaneGroup, remove: bool, client: TmuxClient,
         before_kill: Callable[[], None] | None = None) -> None:
    """Close the lane after every check: stop its stack, optionally remove the worktree, then kill its window.

    The window goes last, and `before_kill` runs just before, because the command may be running inside
    that very window. Raises LaneBusy (nothing changed) when preflight finds work that would be lost.
    """
    reasons = preflight(identity, group, remove)
    if reasons:
        raise LaneBusy(reasons)
    window = _marked_window(client, group, identity)
    if identity.worktree.exists():
        worktrees.run_command(group.down, identity.worktree, "down")
        if remove:
            worktrees.remove(identity, group)
    if before_kill:
        before_kill()
    if window:
        client.kill_window(window.id)


def marked_windows(client: TmuxClient, group: LaneGroup) -> list[Window]:
    """Return the windows of the group's session that sakura marked as lanes of this group; no others."""
    prefix = f"{group.name}/"
    return [window for window in _windows(client, group) if window.options.get(LANE_MARKER_KEY, "").startswith(prefix)]


def agent_panes(window: Window) -> list[Pane]:
    """Return the panes of the window that carry the agent marker (a lane has exactly one)."""
    return [pane for pane in window.panes if pane.marker == AGENT_PANE]


@contextlib.contextmanager
def _launch_lock(group: LaneGroup) -> Iterator[None]:
    """Serialize `open` of one group across processes, from the window lookup through the finished window.

    The lock file sits in the worktree root under a dot name, which no lane name can take.
    """
    path = group.worktree_root / LAUNCH_LOCK_NAME.format(group=group.name)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def _ensure_worktree(identity: LaneIdentity, group: LaneGroup, from_ref: str | None) -> None:
    branch_was_new = not worktrees.branch_exists(group, identity)
    created = worktrees.create(identity, group, from_ref)
    if not created and worktrees.setup_done(identity):
        return
    try:
        for command in group.setup:
            worktrees.run_command(command, identity.worktree, "setup")
        worktrees.mark_setup_done(identity)
    except BaseException:
        if created:
            worktrees.rollback(identity, group, branch_was_new)
        raise


def _marked_window(client: TmuxClient, group: LaneGroup, identity: LaneIdentity) -> Window | None:
    return next((window for window in _windows(client, group)
                 if window.options.get(LANE_MARKER_KEY) == identity.window_marker), None)


def _find_window(client: TmuxClient, group: LaneGroup, identity: LaneIdentity) -> Window | None:
    marked = _marked_window(client, group, identity)
    if marked:
        return marked
    if any(window.name == identity.name for window in _windows(client, group)):
        raise LaneConflict(
            f"window {identity.name!r} in session {group.session!r} was not created for lane "
            f"{identity.window_marker}; rename or close it first")
    return None


def _windows(client: TmuxClient, group: LaneGroup) -> list[Window]:
    return client.windows(group.session) if client.has_session(group.session) else []


def _build_window(client: TmuxClient, identity: LaneIdentity, group: LaneGroup) -> Window:
    window = _new_window(client, identity, group)
    try:
        client.set_option(window.id, LANE_MARKER_KEY, identity.window_marker)
        client.set_option(window.panes[0].id, PANE_MARKER_KEY, AGENT_PANE)
        client.split_window(window.id, ["sh", "-c", f"{group.up}\n{_STACK_SHELL}"], identity.worktree, {}, STACK_PANE)
    except Exception:
        client.kill_window(window.id)
        raise
    return window


def _new_window(client: TmuxClient, identity: LaneIdentity, group: LaneGroup) -> Window:
    agent = shlex.split(group.agent)
    if client.has_session(group.session):
        window_id = client.new_window(group.session, identity.name, agent, identity.worktree, {})
        return next(window for window in client.windows(group.session) if window.id == window_id)
    client.ensure_session(group.session, identity.worktree, identity.name, agent, {}, (SESSION_MARKER_KEY, group.name))
    # A session made just now holds only this window; the launch lock keeps any other lane from adding one.
    return next(window for window in client.windows(group.session) if window.name == identity.name)
