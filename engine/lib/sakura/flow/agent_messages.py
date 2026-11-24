"""Messages held for one agent: written to a run that cannot take them now, read by it when it can.

A run that takes no live input (a Codex run, or a Claude run whose channel is closed) keeps what the person wrote to it
here, in the task state, until the run has ended and its conversation is continued with them (`flow/agent_resume.py`).
A message is addressed to the run the person wrote to; the conversation it belongs to is that run's chain.
"""

from __future__ import annotations

import os
import uuid

from sakura.flow.store import TaskStore

QUEUE_KEY = "agent_queue"
# How many messages may wait for agents at once. A message that would be the next is refused in words when it is sent;
# nothing waiting is ever dropped to make room (the engine's own requeues are not limited either).
QUEUE_LIMIT = 200
FULL_WORDS = "Too many messages are waiting for this agent. Wait until it has read them."


class HoldRefused(RuntimeError):
    """Raised, with words for the person, when a message cannot be held for an agent."""


class QueueFull(HoldRefused):
    """Raised, with words for the person, when `QUEUE_LIMIT` messages already wait."""


def hold(store: TaskStore, run: str, text: str, actor: str, kind: str = "steer", command: str = "") -> str:
    """Hold `text` for `run`, written by `actor`; returns the id of the message (`command` when one is given, so a command
    that is held twice is held once)."""
    identity = command or str(uuid.uuid4())

    def change(state: dict) -> None:
        held = state.setdefault(QUEUE_KEY, [])
        if all(known["id"] != identity for known in held):
            if len(held) >= QUEUE_LIMIT:
                raise QueueFull(FULL_WORDS)
            held.append({"id": identity, "run": run, "text": text, "actor": actor, "kind": kind})

    store.update_state(change)
    return identity


def hold_back_in(state: dict, run: str, text: str, actor: str) -> None:
    """Hold again, in `state`, a message that was taken for a launch that did not happen; never refused, however many
    wait."""
    state.setdefault(QUEUE_KEY, []).append({"id": str(uuid.uuid4()), "run": run, "text": text, "actor": actor, "kind": "steer"})


def consume_in(state: dict, identities: set[str]) -> None:
    """Remove, in `state`, the messages with these ids (they were handed on)."""
    state[QUEUE_KEY] = [entry for entry in state.get(QUEUE_KEY, []) if entry["id"] not in identities]


def held_for(state: dict, runs: set[str]) -> list[dict]:
    """The messages held for any of `runs`, oldest first."""
    return [entry for entry in state.get(QUEUE_KEY, []) if entry["run"] in runs]


def needs_workspace(state: dict, workspace: str) -> bool:
    """Whether a message held for a run, or a continuation not yet launched or still running, needs the folder
    `workspace`: the copy of the code a reviewer ran in stays until nothing is left to deliver to it."""
    runs = state.get("runs", {})
    here = {name for name, record in runs.items() if os.path.realpath(record["cwd"]) == os.path.realpath(workspace)}
    if any(entry["run"] in here for entry in state.get(QUEUE_KEY, [])):
        return True
    return any(runs[name].get("resumes") and (runs[name]["state"] in ("starting", "running") or runs[name].get("pending_text"))
               for name in here)


def drop(store: TaskStore, identities: set[str]) -> None:
    """Remove the messages with these ids (they were handed on)."""
    def change(state: dict) -> None:
        state[QUEUE_KEY] = [entry for entry in state.get(QUEUE_KEY, []) if entry["id"] not in identities]

    store.update_state(change)


def take(store: TaskStore, runs: set[str]) -> list[dict]:
    """Remove and return the messages held for any of `runs`, oldest first."""
    taken: list[dict] = []

    def change(state: dict) -> None:
        taken.extend(held_for(state, runs))
        state[QUEUE_KEY] = [entry for entry in state.get(QUEUE_KEY, []) if entry["run"] not in runs]

    store.update_state(change)
    return taken
