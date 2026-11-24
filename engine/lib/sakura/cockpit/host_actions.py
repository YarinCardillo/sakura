"""The host side of the cockpit's actions: controls that belong to this machine, not to a task, lane or account.

"Digest now" (`G`) has the fixed target `["host", "digest"]` and "Add project" (`P`) the fixed target
`["host", "projects"]`, "Always-on service" (`B`) the fixed target `["host", "services"]`, the connectors (`O`, `o`) `["host", "connectors"]` and the
digest's settings (`I`, `J`) `["host", "digest-settings"]` the automations (`A`) `["host", "automations"]` and the engine (`K` set a rule, `U` use a preset, `V` save the rules as a preset, `Q` set the pace, `Y` yield to the person, `M` refresh the models, `F` `Z` `E` the phone page, `g` `j` add an account) `["host", "engine"]`, so a request can only name
them by those keys and never chooses what runs.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from sakura.connectors import contract

KIND_HOST = "host"


@dataclass(frozen=True)
class HostTarget:
    """The target of a host action. `key` is `("host", name)`."""

    name: str

    kind = KIND_HOST

    @property
    def key(self) -> tuple:
        return (KIND_HOST, self.name)

    @property
    def repo(self) -> Path:
        """Where the command runs: the digest reads no repository, so the root, which names no one."""
        return Path(os.sep)


DIGEST = HostTarget("digest")
PROJECTS = HostTarget("projects")
SERVICES = HostTarget("services")
CONNECTORS = HostTarget("connectors")
DIGEST_SETTINGS = HostTarget("digest-settings")
AUTOMATIONS = HostTarget("automations")
ENGINE = HostTarget("engine")
SCHEDULED = HostTarget("scheduled")
TARGETS = (DIGEST, PROJECTS, SERVICES, CONNECTORS, DIGEST_SETTINGS, AUTOMATIONS, ENGINE, SCHEDULED)
_ACTIONS_OF = {SCHEDULED.name: frozenset({"scheduledcreate"}), DIGEST.name: frozenset("G"), PROJECTS.name: frozenset("P"), SERVICES.name: frozenset("B"),
               CONNECTORS.name: frozenset("Oo") | contract.ACTIONS, DIGEST_SETTINGS.name: frozenset("IJ"), AUTOMATIONS.name: frozenset("A"), ENGINE.name: frozenset({*"KUVQYMFZEgj", "setupapply"})}


def allows(target: HostTarget, key: str) -> bool:
    """Whether the host control `key` belongs to `target`: each fixed target takes only its own actions."""
    return key in _ACTIONS_OF.get(target.name, frozenset())
