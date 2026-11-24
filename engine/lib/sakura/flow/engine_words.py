"""The engine's ledger lines about an agent, said for the person: what happened to the agent or to a message to it, with no
run name, actor or path in it. The raw line stays apart as technical detail (`flow/agent_feed.py`)."""

from __future__ import annotations

import re
from collections.abc import Callable

from sakura.flow import ask_words
from sakura.flow.render import strip_controls

RUN = r"[a-z]+-\d+(?:\.\d+)*"
STEERED = re.compile(rf"steer to {RUN} (?P<outcome>delivered|queued|resumed|refused)(?: actor=\S+)?")
COULD_NOT_CONTINUE = re.compile(rf"{RUN} could not continue: (?P<why>.+)", re.DOTALL)
HELD = re.compile(rf"{RUN}: (?:a message for it is held and cannot be delivered|a worker for its held messages could not "
                  r"be started): .+", re.DOTALL)
OUTPUT_CAP = re.compile(rf"output cap exceeded: {RUN} was terminated")
LOG_REPLACED = re.compile(rf"log replaced during the run: {RUN} was terminated")
LEFT_RUNNING = re.compile(rf"{RUN} left processes running after it exited; they were terminated")
STEER_WORDS = {
    "delivered": "Your message reached the {who} while it was working.",
    "queued": "Your message is held: the {who} reads it as soon as it can.",
    "resumed": "Your message reached the {who}: it picked the conversation back up.",
    "refused": "Your message was not sent: the {who} cannot be written to now.",
}


def who(role: str) -> str:
    """The agent as a person calls it: its role, in words."""
    return strip_controls(role).replace("_", " ")


def notice(line: str, role: str) -> tuple[str, str]:
    """A ledger line that names a run of an agent whose role is `role`, as plain words, and the raw line as technical
    detail ("" when the words are the line itself)."""
    agent = who(role)
    words = _known(line, agent) or _generic(line, agent)
    return words, "" if words == line else line


def _known(line: str, agent: str) -> str | None:
    if steered := STEERED.fullmatch(line):
        return STEER_WORDS[steered["outcome"]].format(who=agent)
    if refused := COULD_NOT_CONTINUE.fullmatch(line):
        return f"The {agent} could not pick the conversation back up: {ask_words.plain_folders(refused['why'])}"
    if HELD.fullmatch(line):
        return f"Your message is held: it could not be passed on to the {agent} yet."
    for pattern, words in _FIXED:
        if pattern.fullmatch(line):
            return words(agent)
    return None


_FIXED: tuple[tuple[re.Pattern, Callable[[str], str]], ...] = (
    (OUTPUT_CAP, lambda agent: f"The {agent} wrote more than the engine allows, so it was stopped."),
    (LOG_REPLACED, lambda agent: f"The {agent}'s log was changed while it ran, so it was stopped."),
    (LEFT_RUNNING, lambda agent: f"The {agent} left programs running after it ended: the engine stopped them."),
)


def _generic(line: str, agent: str) -> str:
    """A line the engine has no words for: its run names become the agent's role and its paths lose their folders."""
    plain = re.sub(RUN, f"the {agent}", ask_words.plain_folders(line))
    sentence = plain[:1].upper() + plain[1:]
    return sentence if sentence.endswith((".", "!", "?")) else sentence + "."
