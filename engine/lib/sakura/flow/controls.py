"""Commands shown to the person exactly as they will run: no character the terminal would act on or hide."""

from __future__ import annotations

import unicodedata

# Format characters (bidi overrides, zero-width marks), line and paragraph separators and lone surrogates reorder or
# hide what the person reads without being control characters.
INVISIBLE_CATEGORIES = frozenset({"Cf", "Zl", "Zp", "Cs"})


def is_control(character: str) -> bool:
    """Below 0x20 (tab and newline included), DEL, or a C1 control."""
    code = ord(character)
    return code < 0x20 or 0x7F <= code <= 0x9F


def escaped(text: str) -> str:
    """`text` with every control character written as `\\xNN` and every invisible one as `\\uNNNN`."""
    return "".join(_escape(character) for character in text)


def _escape(character: str) -> str:
    if is_control(character):
        return f"\\x{ord(character):02x}"
    if not character.isascii() and unicodedata.category(character) in INVISIBLE_CATEGORIES:
        return f"\\u{ord(character):04x}"
    return character
