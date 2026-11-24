"""Who asked for a change: a structured, validated actor shared by flow and lane commands."""

from __future__ import annotations

import getpass
import os
import re
from dataclasses import dataclass
from typing import Mapping

ACTOR_ENV = "SAKURA_ACTOR"
CLI_SOURCE = "cli"
APP_SOURCE = "app"  # the person at the Sakura window, which runs the cockpit commands for them
OUTSIDE_SOURCE = "outside"
MASTER_SOURCE = "master"
PERSON_SOURCES = ("overlay", CLI_SOURCE, APP_SOURCE)  # the sources that stand for the person
# Sources only the cockpit's own process sets: the verified viewer of the overlay gate, and the master's server. A command
# line (`--actor`, `SAKURA_ACTOR`) names them only inside a child the cockpit or the master's server started, which carries
# `TRUSTED_ENV`; from a terminal the person is `cli:<user>`. This keeps an honest process honest; it is no barrier against
# another process of the same user, which can set the variable too (plan, Confinement).
RESERVED_SOURCES = ("overlay", MASTER_SOURCE)
TRUSTED_ENV = "SAKURA_ACTOR_TRUSTED"
ACTOR_PATTERN = re.compile(r"([a-z]+):([A-Za-z0-9._@+-]{1,64})(?::([A-Za-z0-9._-]{1,64}))?")
_NOT_LOGIN = re.compile(r"[^A-Za-z0-9._@+-]")
MAX_PART = 64
UNKNOWN_USER = "unknown"


class ActorError(ValueError):
    """Raised when an actor string is malformed."""


@dataclass(frozen=True)
class Actor:
    """Source, login and optional node of the party that issued a command; `device` is for the cockpit log only."""

    source: str
    login: str
    node_id: str = ""
    device: str = ""


def parse(value: str) -> Actor:
    """Parse `<source>:<login>[:<node_id>]`; raises ActorError on anything else, control characters included."""
    match = ACTOR_PATTERN.fullmatch(value)
    if not match:
        raise ActorError(
            f"invalid actor {value!r}; use <source>:<login>[:<node_id>] with letters, digits and . _ @ + - only")
    source, login, node_id = match.groups()
    return Actor(source, login, node_id or "")


def format(actor: Actor) -> str:  # noqa: A001 (the module reads as actors.format)
    """Return the canonical `<source>:<login>[:<node_id>]` text of an actor."""
    parts = [actor.source, actor.login] + ([actor.node_id] if actor.node_id else [])
    return ":".join(parts)


def resolve(value: str | None, env: Mapping[str, str] | None = None) -> Actor:
    """Return the actor of a command: the flag, else `SAKURA_ACTOR`, else `cli:<user>`; a overlay or master identity is
    refused unless the process is a child of the cockpit or of the master's server (`TRUSTED_ENV`)."""
    env = os.environ if env is None else env
    chosen = value if value is not None else env.get(ACTOR_ENV) or None
    if chosen is None:
        return Actor(CLI_SOURCE, _local_login())
    actor = parse(chosen)
    if actor.source in RESERVED_SOURCES and env.get(TRUSTED_ENV) != "1":
        raise ActorError(f"a command line cannot claim the {actor.source} identity {chosen!r}; it names the person as "
                         f"{CLI_SOURCE}:<user>")
    return actor


def _local_login() -> str:
    try:
        user = getpass.getuser()
    except (KeyError, OSError):
        user = ""
    return _NOT_LOGIN.sub("-", user)[:MAX_PART] or UNKNOWN_USER
