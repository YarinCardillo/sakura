"""Port blocks for workspaces, recorded per owner in a per-user registry."""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
import socket
from pathlib import Path
from typing import Iterator

PORT_BASE = 41000
PORT_BLOCK = 10
PORT_LIMIT = 65535
REGISTRY = Path(".local/state/sakura/ports.json")
# A port is free only when it binds on both loopback addresses: a cage rule names the port on both.
LOOPBACK_ADDRESSES = ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1"))


class PortsExhausted(RuntimeError):
    """Raised when no free port block is left above PORT_BASE."""


def allocate(owner: str, size: int = PORT_BLOCK, preferred_start: int | None = None) -> range:
    """Return the person's port block, allocating a free one on first use.

    The owner is `<repo name>/<task>/<workspace>`. A block is free when no owner holds any of its ports and every
    port can be bound on both 127.0.0.1 and ::1, whatever host the app's origin names: the cage opens a port on
    both, so no other service may already listen on either.
    """
    with _locked() as registry:
        if owner in registry:
            held = registry[owner]
            if preferred_start is None or held[0] == preferred_start:
                return range(held[0], held[-1] + 1)
            if not all(is_free(port) for port in held):
                raise PortsExhausted(f"cannot move active port block for {owner}")
        taken = {port for key, held in registry.items() if key != owner for port in held}
        starts = [preferred_start] if preferred_start is not None else range(PORT_BASE, PORT_LIMIT - size + 2, size)
        for start in starts:
            block = range(start, start + size)
            if (start >= 1 and start + size <= PORT_LIMIT + 1 and not taken.intersection(block)
                    and all(is_free(port) for port in block)):
                registry[owner] = list(block)
                return block
    raise PortsExhausted(f"no free block of {size} ports" if preferred_start else f"no free block of {size} ports above {PORT_BASE}")


def release(owner: str) -> None:
    """Forget the person's block; unknown owners are ignored."""
    with _locked() as registry:
        registry.pop(owner, None)


def release_prefix(prefix: str) -> None:
    """Forget every block whose owner starts with `prefix`."""
    with _locked() as registry:
        for owner in [owner for owner in registry if owner.startswith(prefix)]:
            registry.pop(owner)


def owners() -> dict[str, list[int]]:
    """Return every owner with its ports."""
    with _locked() as registry:
        return {owner: list(held) for owner, held in registry.items()}


def _registry_path() -> Path:
    return Path.home() / REGISTRY


@contextlib.contextmanager
def _locked() -> Iterator[dict[str, list[int]]]:
    """Yield the registry under an exclusive lock and write it back on exit."""
    path = _registry_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(f"{path}.lock", "a") as guard:
        fcntl.flock(guard, fcntl.LOCK_EX)
        registry = json.loads(path.read_text()) if path.exists() else {}
        yield registry
        _write(path, registry)


def _write(path: Path, registry: dict) -> None:
    temp = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    with open(temp, "w") as stream:
        json.dump(registry, stream, indent=2, sort_keys=True)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temp, path)


def binds_reusing(port: int) -> bool:
    """Whether `port` binds on both loopback addresses with SO_REUSEADDR: refused by a listener on that address,
    not by connections of an earlier one still closing (TIME_WAIT). A wildcard listener is not refused: read the
    kernel's listener table for that."""
    for family, address in LOOPBACK_ADDRESSES:
        with socket.socket(family, socket.SOCK_STREAM) as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                probe.bind((address, port))
            except OSError:
                return False
    return True


def is_free(port: int) -> bool:
    """Whether nothing listens on `port`: it binds on both 127.0.0.1 and ::1."""
    for family, address in LOOPBACK_ADDRESSES:
        with socket.socket(family, socket.SOCK_STREAM) as probe:
            try:
                probe.bind((address, port))
            except OSError:
                return False
    return True
