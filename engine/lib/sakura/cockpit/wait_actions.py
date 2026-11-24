"""The wait side of the cockpit's actions: the target and which actions each kind of wait takes.

A wait is addressed by its id, `["wait", repo, task, kind, key]` as compact JSON (`flow/waits.py`): the key names the
gate of an approval or the row of an owner row, so an action can only reach the very point the person was shown. A
wait the engine has since resolved is no longer in the snapshot, and the controller refuses it.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from sakura.cockpit import attention, digest_reading, proposal_actions, track_actions
from sakura.flow import waits
from sakura.scheduled import inbox as scheduled_inbox

if TYPE_CHECKING:
    from sakura.cockpit.items import Item

KIND_WAIT = "wait"
# Offered on an owner row only while workspace 0 is away from the engine's head (`Wait.head_moved`).
RESTORE_KEY = "H"
BLOCKED_ACTIONS = ("R", "s", "C")
OPEN = "open"
NEEDS_YOU, TRIAGE = "needs_you", "triage"
# The action keys each kind of wait offers; `blocked` offers none, there is nothing the cockpit may answer.
ACTION_KEYS = {scheduled_inbox.APPROVAL: ("y", "N"), waits.APPROVAL: ("a",), waits.OWNER_ROW: ("k", RESTORE_KEY), waits.QUESTION: ("r",),
               waits.TRACK_DECISION: (track_actions.SETTLE,),
               waits.PROPOSAL: ("y", "N"), waits.AUTOMATION: ("y", "N"), waits.BLOCKED: (), waits.REVIEW: ("markreviewed",)}
KEY_LENGTH = 5
NAME_POSITION = 4


@dataclass(frozen=True)
class WaitTarget:
    """The target of a wait action. `key` is `("wait", repo, task, kind, key)`, the wait's id."""

    wait: waits.Wait
    projects: tuple[Path, ...] = ()
    lane: Item | None = None
    missing_projects: frozenset[Path] = frozenset()

    is_replay_only: bool = False

    kind = KIND_WAIT

    @property
    def key(self) -> tuple:
        return tuple(json.loads(self.wait.id))

    @property
    def repo(self) -> Path:
        return Path(self.wait.repo)

    @property
    def folder_missing(self) -> bool:
        """Whether the project folder associated with the wait is missing."""
        return self.wait.folder_missing

    @property
    def is_outside(self) -> bool:
        """Whether an outside agent raised this wait (its answer goes to the lane's log, not to an engine task)."""
        return self.wait.source == "outside"

    @property
    def wait_kind(self) -> str:
        """`approval`, `owner_row`, `question`, `proposal` or `blocked`."""
        return self.wait.kind

    @property
    def name(self) -> str:
        """The key of the wait as it was drawn: the gate of an approval, the row id of an owner row, the id of a
        question or a proposal, with the revision the person was shown (`name@revision`)."""
        return self.key[NAME_POSITION]

    @property
    def subject(self) -> str:
        """The gate of an approval, the row id of an owner row, the id of a question or a proposal, without the
        revision."""
        return waits.split_key(self.name)[0]

    @property
    def revision(self) -> str:
        """The revision of the wait the person was shown; empty for a wait drawn without one."""
        return waits.split_key(self.name)[1]


def target_of(wait: waits.Wait) -> WaitTarget:
    """The target a wait of the snapshot stands for."""
    return WaitTarget(wait)


def inbox_kind(wait: waits.Wait) -> str:
    """The public Inbox kind. The target key keeps the kind that dispatches existing answers."""
    if wait.finding is not None or (wait.source == "scheduled" and wait.kind == waits.PROPOSAL) or digest_reading.date_of_wait(wait):
        return waits.PROPOSAL
    if wait.kind in (waits.OWNER_ROW, waits.AUTOMATION, waits.PROPOSAL, scheduled_inbox.APPROVAL):
        return waits.APPROVAL
    return waits.QUESTION if wait.kind == waits.TRACK_DECISION else wait.kind


def inbox_section(wait: waits.Wait) -> str:
    """The Inbox section derived from the public kind."""
    return TRIAGE if inbox_kind(wait) == waits.PROPOSAL else NEEDS_YOU


def inbox_group(wait: waits.Wait) -> dict[str, str]:
    """The stable lane or source key and its display label."""
    if digest_reading.date_of_wait(wait):
        return {"id": "digest", "label": digest_reading.DIGEST_TITLE}
    if wait.finding:
        origin = wait.finding["origin"]
        return {"id": json.dumps(["scheduled", origin["task_id"]], separators=(",", ":")), "label": origin["title"]}
    if wait.kind == scheduled_inbox.APPROVAL:
        return {"id": "scheduled", "label": scheduled_inbox.LABEL}
    if wait.kind == waits.AUTOMATION:
        return {"id": "automations", "label": waits.AUTOMATIONS_PROJECT}
    key = json.loads(wait.id)
    return {"id": json.dumps(key[1:3], separators=(",", ":")),
            "label": wait.project if wait.track else wait.lane_title or wait.task or wait.project or wait.source}


def allows(target: WaitTarget, action_key: str) -> bool:
    """Whether the action `action_key` applies to this kind of wait."""
    if action_key in attention.ACTIONS:
        return action_key != "unsnooze" or target.wait.is_snoozed
    if inbox_kind(target.wait) == waits.PROPOSAL:
        return action_key in (proposal_actions.START, proposal_actions.DISMISS)
    if action_key == RESTORE_KEY and not target.wait.head_moved:
        return False
    return action_key in ACTION_KEYS.get(target.wait_kind, ())
