"""Generic layered-settings merge shared by the profile and the flow config."""

from __future__ import annotations

import copy
from typing import Callable, Collection

RELAX_KEY = "relax"

Relaxes = Callable[[str, object, object], bool]
LayerRelaxes = Callable[[str, str, object, object], bool]


def merge(
    defaults: dict,
    layers: list[tuple[str, dict]],
    relaxes: Relaxes,
    error: type[Exception],
    noun: str = "layer",
    relaxes_in_layer: LayerRelaxes | None = None,
    tighten_only: Collection[str] = (),
) -> tuple[dict, dict[str, str], list[str]]:
    """Apply named layers over defaults in order.

    Returns the merged values, the layer name that supplied each flattened key,
    and a list of consented relaxations. A layer that weakens a setting (as
    judged by `relaxes`, or by `relaxes_in_layer(layer name, key, old, new)` for
    rules that depend on which layer sets it) must list the key in its own
    `relax` list, otherwise `error` is raised. A layer named in `tighten_only` (an untrusted one, such as a
    repository's own file) has no `relax` at all: it may carry none and any weakening it makes raises `error`.
    `noun` words the error messages ("<name> <noun> ...").
    """
    known = flatten(defaults)
    values = copy.deepcopy(defaults)
    origins = {key: "defaults" for key in known}
    relaxations: list[str] = []
    for name, layer in layers:
        _apply(values, origins, relaxations, known, name, noun, layer, relaxes, error, relaxes_in_layer,
               name in tighten_only)
    return values, origins, relaxations


def _apply(values, origins, relaxations, known, name, noun, layer, relaxes, error, relaxes_in_layer,
           only_tightens) -> None:
    label = f"{name} {noun}"
    if only_tightens and RELAX_KEY in layer:
        raise error(f"{label} may only tighten settings, so it cannot carry {RELAX_KEY}; remove it "
                    "(a weaker setting comes from a host layer or the person's own choice)")
    allowed = set(layer.get(RELAX_KEY, []))
    if not allowed.issubset(set(known)):
        raise error(f"{label} names an unknown relaxed rule")
    for key, value in flatten(layer).items():
        if key == RELAX_KEY:
            continue
        if key not in known:
            raise error(f"{label} has unknown key {key}")
        old = get_key(values, key)
        weakens = relaxes(key, old, value) or bool(relaxes_in_layer and relaxes_in_layer(name, key, old, value))
        if weakens and only_tightens:
            raise error(f"{label} relaxes {key}, and a layer that only tightens may not; "
                        "a weaker setting comes from a host layer or the person's own choice")
        if weakens and key not in allowed:
            raise error(f"{label} relaxes {key}; add it to relax")
        if weakens:
            relaxations.append(f"{key} ({origins[key]} to {name})")
        set_key(values, key, value)
        origins[key] = name


def flatten(data: dict, prefix: str = "") -> dict[str, object]:
    """Flatten nested tables into dotted keys; lists are leaf values."""
    items: dict[str, object] = {}
    for key, value in data.items():
        name = f"{prefix}.{key}" if prefix else key
        if isinstance(value, dict):
            items.update(flatten(value, name))
        else:
            items[name] = value
    return items


def get_key(data: dict, key: str) -> object:
    """Read a dotted key."""
    current: object = data
    for part in key.split("."):
        current = current[part]  # type: ignore[index]
    return current


def set_key(data: dict, key: str, value: object) -> None:
    """Write a dotted key into existing nested tables."""
    current = data
    parts = key.split(".")
    for part in parts[:-1]:
        current = current[part]
    current[parts[-1]] = value
