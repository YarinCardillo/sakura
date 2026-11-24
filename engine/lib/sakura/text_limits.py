"""Checks for text that comes from outside the engine's own code: a person's input or an agent's words.

Each check returns the reason a value is refused, or None. The reasons are plain sentences a person can read.
"""

from __future__ import annotations

import hashlib
import json
import re
import unicodedata
from datetime import datetime, timezone

from sakura.flow.tmux import paste_refusal

DIGEST_LENGTH = 12
_PRINTABLE = re.compile(r"[^\x00-\x1f\x7f-\x9f]*")
_HIDDEN_CATEGORIES = frozenset({"Cf", "Zl", "Zp"})  # bidi and zero-width marks, line and paragraph separators
# Characters that change what text looks like without being visible: format characters (bidi marks and isolates, zero
# width and joiners), line and paragraph separators, private use and unassigned characters, tags and variation selectors.
_HIDDEN_CATEGORIES = frozenset({"Cf", "Zl", "Zp", "Co", "Cn", "Cs"})
_HIDDEN_RANGES = ((0xE0000, 0xE007F), (0xFE00, 0xFE0F), (0xE0100, 0xE01EF))
# The combining grapheme joiner (category Mn) draws nothing, so two texts that differ only by it look the same.
_HIDDEN_CODES = frozenset({0x034F})


def one_line_problem(value: str, limit: int) -> str | None:
    """Why `value` is not one line of 1 to `limit` printable characters (no control, bidi or zero-width one)."""
    reason = f"must be one line of 1 to {limit} printable characters"
    if not 1 <= len(value) <= limit or not _PRINTABLE.fullmatch(value):
        return reason
    if hidden_character_problem(value):
        return reason
    return None


def text_problem(value: str, limit: int) -> str | None:
    """Why `value` is not pasteable text of at most `limit` characters (newline, carriage return and tab pass)."""
    if not value.strip():
        return "empty"
    if len(value) > limit:
        return f"{len(value)} characters; the limit is {limit}"
    return paste_refusal(value)


def hidden_character_problem(value: str) -> str | None:
    """Why `value` holds a character that is hidden or changes how text reads (a format, separator, private use,
    unassigned, surrogate, tag or variation selector character, or the combining grapheme joiner), or None. Plain space, tab and newline are not hidden."""
    if any(_is_hidden(character) for character in value):
        return "refused text with a hidden character (bidi, zero-width, tag or unassigned)"
    return None


def without_hidden_characters(value: str) -> str:
    """`value` with every hidden character (the ones `hidden_character_problem` refuses) removed."""
    return "".join(character for character in value if not _is_hidden(character))


def _is_hidden(character: str) -> bool:
    code = ord(character)
    return (unicodedata.category(character) in _HIDDEN_CATEGORIES or code in _HIDDEN_CODES
            or any(low <= code <= high for low, high in _HIDDEN_RANGES))


def content_digest(fields: dict) -> str:
    """The first 12 hex characters of the SHA-256 of `fields` as compact JSON with sorted keys."""
    encoded = json.dumps(fields, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()[:DIGEST_LENGTH]


def utc_text(moment: datetime) -> str:
    """`moment` as the UTC ISO text the engine's records use (seconds, `+00:00`)."""
    return moment.astimezone(timezone.utc).isoformat(timespec="seconds")
