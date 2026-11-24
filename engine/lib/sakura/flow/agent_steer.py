"""Writing to one agent: what happens to a message the person sends to a run of the lane.

The message goes to the newest run of the agent's chain (`flow/agent_feed.py`): live into a running Claude run, held for
a run that is running but takes no live input (a worker continues it with the message when it ends), a new run that
continues the conversation of one that ended (`flow/agent_resume.py`), refused in words when the agent cannot be
written to. The outcome says which, in a sentence for the person.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from sakura.flow import agent_feed, agent_messages, agent_resume, inbox, run_view
from sakura.flow.render import strip_controls
from sakura.flow.config import FlowConfig
from sakura.flow.store import TaskStore

DELIVERED, QUEUED, RESUMED, REFUSED = "delivered", "queued", "resumed", "refused"


@dataclass(frozen=True)
class Outcome:
    """What came of a message to an agent: `outcome`, the `run` that got it (or was asked) and one sentence of `words`."""

    outcome: str
    run: str
    words: str

    def to_json(self) -> str:
        """The outcome as one line of JSON."""
        return json.dumps({"outcome": self.outcome, "run": self.run, "words": self.words})


def send(store: TaskStore, cfg: FlowConfig, common_dir: Path, run: str, text: str, actor: str) -> Outcome:
    """Send `text`, written by `actor`, to the agent of `run`; what became of it is ledgered with the actor (a message
    delivered live is ledgered by the inbox, with its actor, as any steer)."""
    outcome = _dispatch(store, cfg, common_dir, run, text, actor)
    if outcome.outcome != DELIVERED:
        store.append_ledger("operator", f"steer to {outcome.run} {outcome.outcome} actor={actor}")
    return outcome


def _dispatch(store: TaskStore, cfg: FlowConfig, common_dir: Path, run: str, text: str, actor: str) -> Outcome:
    try:
        with agent_resume.chain_lock(store, run):
            target = agent_feed.chain_of(store, run)[-1]
            how, problem = run_view.listening(store, store.read_state(), target, common_dir)
            if how == run_view.RESUME:
                return _resume(store, cfg, common_dir, target, text)
    except agent_feed.FeedRefused as refusal:
        return Outcome(REFUSED, strip_controls(run), str(refusal).capitalize() + ".")
    name, who = target["name"], _who(target)
    if how == run_view.LIVE:
        return _live(store, target, text, actor, who)
    if how == run_view.QUEUE:
        return _hold(store, name, text, actor, who)
    return Outcome(REFUSED, name, problem or f"The {who} cannot be written to now.")


def _resume(store: TaskStore, cfg: FlowConfig, common_dir: Path, target: dict, text: str) -> Outcome:
    """Continue a finished agent's conversation with `text` (the caller holds the chain's lock)."""
    try:
        name = agent_resume.start(store, cfg, common_dir, target["name"], text)
    except agent_resume.ResumeRefused as refusal:
        return Outcome(REFUSED, target["name"], str(refusal))
    try:
        agent_resume.spawn_worker(store, name, begun=True)
    except OSError:
        agent_resume.abandon(store, name)
        return Outcome(REFUSED, target["name"], f"The {_who(target)} could not be started now. Your message is kept for it.")
    return Outcome(RESUMED, name, f"The {_who(target)} is back at it.")


def _live(store: TaskStore, target: dict, text: str, actor: str, who: str) -> Outcome:
    """Offer the message to the run's live channel; one it does not take in time is taken back and held for when the run ends."""
    name = target["name"]
    if len(agent_messages.held_for(store.read_state(), {name})) >= agent_messages.QUEUE_LIMIT:
        return Outcome(REFUSED, name, agent_messages.FULL_WORDS)
    queued = inbox.enqueue(store, "steer", text, "", actor, run=name)
    if inbox.wait_live(store, inbox.command_id(queued), run=name):
        return Outcome(DELIVERED, name, f"The {who} has it now.")
    try:
        if inbox.withdraw(store, queued):
            agent_resume.hold_for(store, name, text, actor)
        elif inbox.was_rejected(store, queued):
            return Outcome(REFUSED, name, agent_messages.FULL_WORDS)
        else:
            agent_resume.wake(store, name)
    except agent_messages.HoldRefused as full:
        return Outcome(REFUSED, name, str(full))
    return Outcome(QUEUED, name, f"The {who} reads this when it finishes.")


def _hold(store: TaskStore, name: str, text: str, actor: str, who: str) -> Outcome:
    try:
        agent_resume.hold_for(store, name, text, actor)
    except agent_messages.HoldRefused as full:
        return Outcome(REFUSED, name, str(full))
    return Outcome(QUEUED, name, f"The {who} reads this when it finishes.")


def _who(record: dict) -> str:
    return strip_controls(str(record["role"])).replace("_", " ")
