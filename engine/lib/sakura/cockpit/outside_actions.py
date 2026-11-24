"""The person's answers to outside lanes: mark, reply, write to the agent and close.

These run in the cockpit's own process, not as a command of `sakura outside`: no command line of an outside agent
carries a verb of the person, so an answer exists only through the cockpit's action table, for an actor of source
`overlay` (the gate) or `cli` (`sakura cockpit act`). Each call appends to the lane's log under the lane's lock and
refuses content that changed since the person saw it ("no longer waiting"). The `argv` is for the confirmation page
and the action log only; nothing spawns it.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from sakura import actors, engine_home
from sakura.cockpit import digest_reading, proposal_actions
from sakura.flow import exits as exits_codes
from sakura.flow import waits
from sakura.outside import inbox, store
from sakura.outside.records import Refused, StoreError

KIND_OUTSIDE = "outside"
SEPARATOR = "--"
DIGEST_SEPARATOR = "@"
Handler = Callable[[actors.Actor], tuple[int, str]]


@dataclass(frozen=True)
class Call:
    """What an outside action does: the words that describe it, the store folder, and the in-process handler."""

    argv: list[str]
    cwd: Path
    handler: Handler


class NotAnswerable(ValueError):
    """The target or inputs do not name something the person can answer; the reason is one line."""


def mark(wait, inputs: dict) -> Call:
    """Mark the row the wait stands for Ok or Ko (optional note) as the row was when the person saw it."""
    lane, row, digest = _parts(wait)
    options = [f"--verdict={inputs['verdict']}", *([f"--note={inputs['note']}"] if "note" in inputs else [])]
    return _call("mark", options, [lane, row, digest], lambda root, by, now: inbox.mark(
        root, by, lane, row, digest, inputs["verdict"], inputs.get("note", ""), now,
        require_answerable=lambda: _require_answerable(wait.wait)))


def reply(wait, inputs: dict) -> Call:
    """Answer the question the wait stands for with one of its choices or with text, never both."""
    lane, question, digest = _parts(wait)
    choice, text = inputs.get("choice", ""), inputs.get("text", "")
    if bool(choice) == bool(text):
        raise NotAnswerable("give a choice or text, one of them")
    if choice and choice not in wait.wait.choices:
        raise NotAnswerable(f"{choice!r} is not one of the choices")
    options = [f"--choice={choice}"] if choice else [f"--text={text}"]
    return _call("reply", options, [lane, question, digest], lambda root, by, now: inbox.reply(
        root, by, lane, question, digest, choice, text, now,
        require_answerable=lambda: _require_answerable(wait.wait)))


def steer(item, inputs: dict) -> Call:
    """Write a message to the lane's agent."""
    lane = item.key[1]
    return _call("steer", [], [lane, inputs["text"]],
                 lambda root, by, now: inbox.steer(root, by, lane, inputs["text"], now))


def close(item, inputs: dict) -> Call:
    """Close the lane as dropped."""
    lane = item.key[1]

    def closing(root: Path, by: actors.Actor, now: datetime) -> dict:
        inbox.require_person(by, "close")
        return {"lane": lane, "outcome": store.close(root, by, lane, outcome="dropped", now=now).lane.outcome}

    return _call("close", [], [lane], closing)


def _require_answerable(wait: waits.Wait) -> None:
    if digest_reading.date_of_wait(wait) or proposal_actions.outcome(wait.id):
        raise Refused("Use Start or Dismiss to resolve this proposal.")


def _parts(wait) -> tuple[str, str, str]:
    name, separator, digest = wait.name.rpartition(DIGEST_SEPARATOR)
    if not separator or not name or not digest:
        raise NotAnswerable("this wait does not name a row or question")
    return wait.key[2], name, digest


def _call(verb: str, options: list[str], positionals: list[str],
          act: Callable[[Path, actors.Actor, datetime], dict]) -> Call:
    try:
        root = store.path()
    except engine_home.EngineHomeError as error:
        raise NotAnswerable(str(error)) from error
    return Call(["cockpit-outside", verb, *options, SEPARATOR, *positionals], root, _handler(root, act))


def _handler(root: Path, act: Callable[[Path, actors.Actor, datetime], dict]) -> Handler:
    def run(by: actors.Actor) -> tuple[int, str]:
        try:
            line = act(root, by, datetime.now(timezone.utc))
        except Refused as error:
            return exits_codes.USAGE, json.dumps({"result": "refused", "reason": str(error)})
        except (StoreError, OSError) as error:
            return exits_codes.FAILED, json.dumps({"result": "error", "reason": str(error)})
        return exits_codes.OK, json.dumps({"result": "recorded", "entry": line})

    return run
