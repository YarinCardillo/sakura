"""Plain words for the log lines of an outside lane, and the `kind` every event of the snapshot is rendered by.

The app shows `words` when it is there and falls back to its own reading of `text` when it is not. A person's own
message to a lane is never an engine event: its words say what the person did. Titles come from the agent: they are data,
shown as text, never run.
"""

from __future__ import annotations

import re

LEDGER_TAG = re.compile(r"\[([^\]]{1,40})\] ")
TITLE_CHARACTERS = 120
MARKED = {"ok": "You marked it done", "ko": "You marked it not done"}


def ledger_kind(text: str) -> str:
    """The tag of a ledger line (`[pace] ...` is `pace`); `ledger` for a line without one."""
    found = LEDGER_TAG.match(text)
    return found[1] if found else "ledger"


def outside_words(line: dict) -> str | None:
    """What an outside lane's log line says in plain words, or None for a kind with no words of its own."""
    kind = line.get("kind")
    title = _title(line)
    if kind == "opened":
        return _with("Opened", title)
    if kind == "row":
        return _with("Added", title)
    if kind == "asked":
        return "Asked a question"
    if kind == "changed":
        return "Updated the lane"
    if kind == "closed":
        return _with("Closed", str(line.get("outcome") or ""))
    if kind == "steer":
        return "You wrote to the lane"
    if kind == "mark":
        return _with(MARKED.get(str(line.get("verdict")), "You answered"), title)
    if kind == "reply":
        return _with("You replied", title)
    return None


def _title(line: dict) -> str:
    return " ".join(str(line.get("title") or "").split())[:TITLE_CHARACTERS]


def _with(lead: str, detail: str) -> str:
    return f"{lead}: {detail}" if detail else lead
