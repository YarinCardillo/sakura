"""Translate command-line flags into the `flags` configuration layer."""

from __future__ import annotations

import tomllib

SCALAR_FLAGS = ("preset", "dial", "familiarity")
RELAX_KEY = "relax"


class FlagError(ValueError):
    """Raised when a flag value is malformed."""


def build_overrides(
    preset: str | None, dial: str | None, familiarity: str | None,
    roles: list[str], settings: list[str], relax: list[str],
) -> dict:
    """Return the nested overrides dict for the given flag values."""
    overrides: dict = {}
    for name, value in zip(SCALAR_FLAGS, (preset, dial, familiarity)):
        if value is not None:
            overrides[name] = value
    for item in roles:
        key, value = _split(item, "--role", "name=value")
        _put(overrides, f"roles.{key}", value)
    for item in settings:
        key, value = _split(item, "--set", "dotted.key=value")
        _put(overrides, key, _parse_value(value))
    if relax:
        overrides[RELAX_KEY] = list(relax)
    return overrides


def _split(item: str, flag: str, shape: str) -> tuple[str, str]:
    key, separator, value = item.partition("=")
    if not separator or not key:
        raise FlagError(f"{flag} expects {shape}; got {item!r}")
    return key, value


def _parse_value(text: str) -> object:
    try:
        return tomllib.loads(f"v = {text}")["v"]
    except tomllib.TOMLDecodeError:
        return text


def _put(target: dict, dotted: str, value: object) -> None:
    *parents, leaf = dotted.split(".")
    for part in parents:
        target = target.setdefault(part, {})
    target[leaf] = value
