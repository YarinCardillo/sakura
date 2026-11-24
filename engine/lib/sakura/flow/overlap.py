"""The overlap notice: after an integration step, the engine tells the board which open lanes of the project change
the same files.

One post per pair of lanes and set of shared files, so a later step that changes nothing about the overlap posts
nothing. Only plain file names are named (`changes.plain`).
"""

from __future__ import annotations

import hashlib
import json

from sakura import engine_home
from sakura.flow import board, changes, delivery, final_head, lane_ceiling, store
from sakura.flow import gitutil
from sakura.flow.gitutil import GitError

ENGINE_AUTHOR = "engine"


def announce(ctx) -> None:
    """Post the overlaps of the lane of `ctx` with the other open lanes of its project. A failure is written to the
    lane's ledger and never stops the integration."""
    try:
        _post_overlaps(ctx)
    except (GitError, OSError, ValueError) as error:
        ctx.store.append_ledger("integration", f"overlap notice not posted: {error}")


def _post_overlaps(ctx) -> None:
    mine = changes.of_lane(ctx.store.read_state(), ctx.repo)
    if not mine:
        return
    root = engine_home.project_dir(ctx.repo)
    known = {post.text for post in board.read(root, 0) if post.author.kind == ENGINE_AUTHOR}
    for other in store.task_stores(ctx.repo):
        if other.task_id == ctx.store.task_id or not lane_ceiling.is_open(other):
            continue
        shared = _shared(mine, changes.of_lane(other.read_state(), ctx.repo))
        text = _notice(ctx, other, shared) if shared else ""
        if text and text not in known:
            board.post(root, board.Author(ENGINE_AUTHOR, ctx.store.task_id), text)
            known.add(text)


def _shared(mine: list[str], theirs: list[str] | None) -> list[str]:
    return sorted(set(changes.plain(mine)) & set(changes.plain(theirs or [])))


def _notice(ctx, other: store.TaskStore, shared: list[str]) -> str:
    pair = sorted(lane_ceiling.lane_name(ctx.repo, task) for task in (ctx.store.task_id, other.task_id))
    return f"lanes {pair[0]} and {pair[1]} both change {', '.join(shared)}"


def delivered_conflicts(ctx, state: dict, store_path, head: str) -> list[dict]:
    """Every lane of the project that delivered a head this lane's base and head do not hold and that does not merge
    cleanly with `head`: for each its name, its delivered head and the plain names of the conflicting files. Empty when
    delivery may go on.

    A delivered head the store does not hold cannot be tested; that is written to the ledger and does not hold up the
    delivery. Raises GitError when the test merge itself fails.
    """
    found_conflicts = []
    own_moment = delivery.delivered_at(state) if delivery.head_of(state) == head else ""  # a push of another head exempts nothing
    late_files = _files_of_a_late_push(store_path, state, head)
    for other in store.task_stores(ctx.repo):
        found, moment = _delivered(other, ctx.store.task_id)
        if not found or (own_moment and moment > own_moment):
            continue  # nothing delivered, or delivered after this lane pushed: it did not come first
        if late_files is not None and _before(moment, delivery.delivered_at(state)):
            continue  # a late push: a lane that delivered before this lane's first push was checked then
        if final_head.descends(store_path, found, state["base_commit"]) or final_head.descends(store_path, found, head):
            continue
        name = lane_ceiling.lane_name(ctx.repo, other.task_id)
        if not final_head.holds(store_path, found):
            ctx.store.append_ledger("pr", f"overlap check skipped for {name}: its delivered head is not in the store")
            continue
        files = final_head.conflicts(ctx, store_path, head, found)
        if late_files is not None:
            files = [name for name in files if name in late_files]
        if files:
            found_conflicts.append({"lane": name, "head": found, "files": changes.plain(files)})
    return found_conflicts


def _files_of_a_late_push(store_path, state: dict, head: str) -> set[str] | None:
    """The files the commits added since this lane's delivery change, when `head` builds on the head it delivered (a late
    commit pushed again); None for a first delivery or a head that does not build on the delivered one."""
    delivered = delivery.head_of(state)
    if not delivered or delivered == head or not final_head.holds(store_path, delivered) \
            or not final_head.descends(store_path, delivered, head):
        return None
    try:
        names = gitutil.git(store_path, "diff", "--name-only", "--no-renames", "-z", delivered, head).split("\0")
        return {name for name in names if name}
    except GitError:
        return None


def _before(moment: str, first_push: str) -> bool:
    """Whether a delivery at `moment` came before this lane's first push; an unknown moment, on either side, is not known to."""
    return bool(moment and first_push and moment <= first_push)


def revision(head: str, conflicts: list[dict]) -> str:
    """What the person approves at the overlap gate: this head and every conflicting delivered head with its files. A
    lane that delivers another conflicting head later makes another revision, which the earlier approval does not cover."""
    ordered = sorted(conflicts, key=lambda item: (item["lane"], item["head"]))
    return hashlib.sha256(json.dumps([head, ordered], sort_keys=True).encode()).hexdigest()


def _delivered(other: store.TaskStore, own_task: str) -> tuple[str, str]:
    """The head another lane delivered, whatever the kind of delivery (`flow/delivery.py`), and the moment it first did ("" when
    the record has none); empty for this lane, for a lane that delivered nothing and for a lane whose state cannot be read."""
    if other.task_id == own_task:
        return "", ""
    try:
        state = other.read_state()
    except (store.StateError, OSError):
        return "", ""
    return delivery.head_of(state), delivery.delivered_at(state)
