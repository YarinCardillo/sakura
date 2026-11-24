"""Port block allocation for workspaces."""

import json
from pathlib import Path

import pytest

from sakura.flow import ports


def _registry() -> Path:
    return Path.home() / ".local/state/sakura/ports.json"


@pytest.fixture(autouse=True)
def free_ports(monkeypatch):
    monkeypatch.setattr(ports, "is_free", lambda _port: True)


def test_allocator_skips_bound_port(monkeypatch):
    occupied = ports.PORT_BASE + 3
    monkeypatch.setattr(ports, "is_free", lambda port: port != occupied)
    block = ports.allocate("repo/t/0")
    assert block == range(ports.PORT_BASE + ports.PORT_BLOCK, ports.PORT_BASE + 2 * ports.PORT_BLOCK)


def test_allocator_never_reuses_owned_block():
    first = ports.allocate("repo/t/0")
    second = ports.allocate("repo/t/1")
    assert not set(first) & set(second)
    assert ports.allocate("repo/t/0") == first


def test_release_frees_the_block_and_owners_lists_ports():
    block = ports.allocate("repo/t/0")
    assert ports.owners() == {"repo/t/0": list(block)}
    ports.release("repo/t/0")
    assert ports.owners() == {}
    assert ports.allocate("repo/t/2") == block
    assert "repo/t/2" in json.loads(_registry().read_text())


def test_release_of_unknown_owner_is_a_no_op():
    ports.release("repo/none/0")
    assert ports.owners() == {}


def test_allocator_reserves_requested_port_and_moves_idle_owner(tmp_path, monkeypatch):
    monkeypatch.setattr(ports, "_registry_path", lambda: tmp_path / "ports.json")
    original = ports.allocate("repo/t/0")
    requested = original.start + 100
    moved = ports.allocate("repo/t/0", preferred_start=requested)
    assert moved.start == requested
    assert ports.allocate("repo/t/0", preferred_start=requested) == moved


def test_allocator_conflict_keeps_previous_reservation(tmp_path, monkeypatch):
    monkeypatch.setattr(ports, "_registry_path", lambda: tmp_path / "ports.json")
    original = ports.allocate("repo/t/0")
    ports.allocate("repo/t/1", preferred_start=original.start + ports.PORT_BLOCK)
    before = ports.owners()
    with pytest.raises(ports.PortsExhausted):
        ports.allocate("repo/t/0", preferred_start=original.start + ports.PORT_BLOCK)
    assert ports.owners() == before
