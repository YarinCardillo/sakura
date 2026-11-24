"""The cursor of one agent's feed (`flow/agent_feed.py`): how far each log was read, which tools were still running,
which runs had their prompt passed, and how many ledger lines were read.

The reader must not read an old log again at every poll, so the cursor holds a byte offset per log. It is opaque to
the app (JSON in URL-safe base64) and checked like any input: a cursor this feed did not give is refused.
"""

from __future__ import annotations

import base64
import binascii
import json
from dataclasses import dataclass, field

from sakura.flow import run_records

MAX_CURSOR_CHARS = 16384
MAX_OPEN_TOOLS = 32
MAX_TOOL_ID_CHARS = 200
MAX_OFFSET = 1 << 40
REQUIRED = {"p", "o", "u", "n"}
OPTIONAL = {"f", "v", "c", "d"}
NOT_A_CURSOR = "not a cursor this feed gave: use the cursor of a previous page, or none"


class CursorRefused(ValueError):
    """Raised, with a one-line reason, when a cursor is not one this feed gave."""


@dataclass
class Cursor:
    """Where an agent's feed stands: `positions` (run -> byte offset the log was read to), `open_tools` (run -> tool id ->
    byte offset of the line that started it, for tools whose result was not read yet), `prompts_passed` (the runs whose
    prompt echo was skipped), `notices` (how many ledger lines were read), `identities` (run -> [inode, bytes of the head
    that were hashed, their hash], to tell a log that was replaced or rewritten from one that grew), `live_from` (run -> where
    the message being written began), `text_chars` (run -> [block, characters] of the long text event at its position that
    were delivered already) and `done` (how many runs from the start of the chain are read to their end and ended: the
    cursor keeps nothing about them, so its size does not grow with the length of the chain).

    An encoded cursor always fits `MAX_CURSOR_CHARS`, the limit `decode` enforces (`encode` raises rather than give one
    that would be refused)."""

    positions: dict[str, int] = field(default_factory=dict)
    open_tools: dict[str, dict[str, int]] = field(default_factory=dict)
    prompts_passed: set[str] = field(default_factory=set)
    notices: int = 0
    identities: dict[str, list] = field(default_factory=dict)
    live_from: dict[str, int] = field(default_factory=dict)
    text_chars: dict[str, list[int]] = field(default_factory=dict)
    done: int = 0

    def encode(self) -> str:
        """The cursor as the text the app hands back; empty for the start of a feed."""
        document = {"p": self.positions, "o": {run: tools for run, tools in self.open_tools.items() if tools},
                    "u": sorted(self.prompts_passed), "n": self.notices}
        for key, value in (("f", self.identities), ("v", self.live_from), ("c", self.text_chars), ("d", self.done)):
            if value:
                document[key] = value
        encoded = base64.urlsafe_b64encode(json.dumps(document, separators=(",", ":"), sort_keys=True).encode()).decode().rstrip("=")
        if len(encoded) > MAX_CURSOR_CHARS:
            raise RuntimeError(f"an agent feed cursor of {len(encoded)} characters would be refused (limit {MAX_CURSOR_CHARS})")
        return encoded


def decode(text: str) -> Cursor:
    """The cursor `text` stands for; the start of a feed for "". Raises CursorRefused for anything this feed did not give."""
    if not text:
        return Cursor()
    if len(text) > MAX_CURSOR_CHARS:
        raise CursorRefused(NOT_A_CURSOR)
    try:
        document = json.loads(base64.urlsafe_b64decode(text + "=" * (-len(text) % 4)))
        return _cursor(document)
    except (ValueError, binascii.Error, KeyError, TypeError, RecursionError):
        raise CursorRefused(NOT_A_CURSOR) from None


def _cursor(document: object) -> Cursor:
    if not isinstance(document, dict) or not REQUIRED <= set(document) <= REQUIRED | OPTIONAL:
        raise ValueError("shape")
    positions = {_run_name(run): _offset(number) for run, number in _mapping(document["p"]).items()}
    tools = {_run_name(run): {_tool_id(tool): _offset(number) for tool, number in _mapping(opened).items()}
             for run, opened in _mapping(document["o"]).items()}
    if sum(len(opened) for opened in tools.values()) > MAX_OPEN_TOOLS:
        raise ValueError("too many open tools")
    return Cursor(positions, tools, {_run_name(run) for run in _list(document["u"])}, _offset(document["n"]),
                  {_run_name(run): _identity(found) for run, found in _mapping(document.get("f", {})).items()},
                  {_run_name(run): _offset(number) for run, number in _mapping(document.get("v", {})).items()},
                  {_run_name(run): _block_chars(found) for run, found in _mapping(document.get("c", {})).items()},
                  _offset(document.get("d", 0)))


def _block_chars(value: object) -> list[int]:
    if not (isinstance(value, list) and len(value) == 2):
        raise ValueError("not a text position")
    return [_offset(value[0]), _offset(value[1])]


def _identity(value: object) -> list:
    if not (isinstance(value, list) and 3 <= len(value) <= 5 and isinstance(value[0], int) and isinstance(value[1], int)
            and all(isinstance(item, (int, str)) for item in value[2:])):
        raise ValueError("not an identity")
    return value


def _mapping(value: object) -> dict:
    if not isinstance(value, dict):
        raise ValueError("not a mapping")
    return value


def _list(value: object) -> list:
    if not isinstance(value, list):
        raise ValueError("not a list")
    return value


def _offset(value: object) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= MAX_OFFSET:
        raise ValueError("not an offset")
    return value


def _run_name(name: object) -> str:
    if not isinstance(name, str) or not run_records.RUN_NAME.fullmatch(name):
        raise ValueError("not a run name")
    return name


def _tool_id(name: object) -> str:
    if not isinstance(name, str) or not 0 < len(name) <= MAX_TOOL_ID_CHARS:
        raise ValueError("not a tool id")
    return name
