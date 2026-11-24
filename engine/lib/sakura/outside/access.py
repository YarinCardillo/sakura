"""Host-issued outside agent grants."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import re
import secrets
import stat
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home
from sakura.flow import gates
from sakura.outside import records, verbs

GRANT_ENV = "SAKURA_OUTSIDE_GRANT"
GRANT_DIRECTORY = "outside-grants"
TOKEN = re.compile(r"[A-Za-z0-9_-]{43}")
REFUSAL = "a host-issued outside grant is required for this agent"
RESERVED_NAMES = frozenset({"digest"})
MAX_GRANT_BYTES = 4096


@dataclass(frozen=True)
class Grant:
    agent: records.Agent
    verbs: frozenset[str]


def issue(name: str, allowed: list[str], cwd: Path) -> str:
    """Issue or rotate the selected agent's grant and return its token once."""
    path = _path(name)
    if not allowed or set(allowed) - verbs.VERBS.keys():
        raise records.Refused("choose one or more outside verbs: " + ", ".join(verbs.VERBS))
    agent = records.agent_here(name, cwd.resolve())
    token = secrets.token_urlsafe(32)
    try:
        path.parent.mkdir(mode=0o700, exist_ok=True)
        info = path.parent.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise OSError
        gates.write_atomically(path, json.dumps({"token_hash": _digest(token), "where": str(agent.where),
                                                "verbs": sorted(set(allowed))}).encode())
    except OSError:
        raise records.Refused("the outside grant could not be saved") from None
    return token


def resolve(name: str, token: str) -> Grant:
    """Bind the presented token to one named principal and its issued verbs."""
    if not TOKEN.fullmatch(token):
        raise records.Refused(REFUSAL)
    try:
        entry = _read(_path(name))
        if not hmac.compare_digest(_digest(token), entry["token_hash"]):
            raise ValueError
        where = Path(entry["where"])
        allowed = frozenset(entry["verbs"])
        if not where.is_absolute() or not allowed or allowed - verbs.VERBS.keys():
            raise ValueError
    except (OSError, ValueError, TypeError, KeyError, engine_home.EngineHomeError):
        raise records.Refused(REFUSAL) from None
    return Grant(records.Agent(name, where), allowed)


def _path(name: str) -> Path:
    records.check_id("agent name", name)
    if name in RESERVED_NAMES:
        raise records.Refused("this outside agent name is reserved for the host")
    return engine_home.root() / GRANT_DIRECTORY / f"{name}.json"


def _read(path: Path) -> dict:
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    with os.fdopen(os.open(path, flags), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise OSError
        data = stream.read(MAX_GRANT_BYTES + 1)
        if len(data) > MAX_GRANT_BYTES:
            raise ValueError
        return json.loads(data)


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode("ascii")).hexdigest()
