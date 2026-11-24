"""Persist launch identities and conversation bindings in the engine home."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import stat
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from pathlib import Path
from uuid import uuid4

from sakura import beneath
from sakura.accounts import write
from sakura.flow import acp

SLOT = re.compile(r"[A-Za-z0-9_-]{1,64}")
LAUNCH = re.compile(r"acp-[0-9a-f]{32}")
MAX_STATE = 2 * 1024 * 1024
MAX_TITLE = 300


class HistoryError(ValueError):
    """A fixed Master history refusal code."""


@dataclass(frozen=True)
class Binding:
    """The identity of one account's session storage."""

    harness: str
    account: str | None
    storage_namespace: str
    folder: str
    folder_identity: tuple[int, int]
    configuration: list | None = None

    @classmethod
    def for_account(cls, harness: str, account: str | None, folder: Path, *, configuration=None) -> Binding:
        """Capture a provider-scoped account folder without opening its contents."""
        if harness not in {"claude", "codex", "goose"} or not folder.is_absolute():
            raise HistoryError("master_storage_unavailable")
        try:
            folder = folder.resolve(strict=True)
            info = folder.stat()
        except (OSError, RuntimeError):
            raise HistoryError("master_storage_unavailable") from None
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
            raise HistoryError("master_storage_unavailable")
        identity = (info.st_dev, info.st_ino)
        key = digest([harness, account, str(folder), identity, configuration])
        return cls(harness, account, f"master:{harness}:{key}", str(folder), identity, configuration)

    def document(self) -> dict:
        """Publish the session binding without disclosing the storage path."""
        return {"harness": self.harness, "account": self.account, "context": "master",
                "storage_namespace": self.storage_namespace}


def digest(data) -> str:
    """Generate a stable identity for engine-owned JSON data."""
    return hashlib.sha256(json.dumps(data, sort_keys=True).encode()).hexdigest()


def launch_id() -> str:
    """Allocate a launch id used only for engine-owned resources."""
    return f"acp-{uuid4().hex}"


def issue(home: Path, slot: str, binding: Binding, policy: str, *, fresh: bool = False,
          launch: str | None = None, parameters: dict | None = None) -> dict:
    """Replace only the active launch, keeping each recorded conversation readable."""
    with _edit(home, slot) as state:
        row = None if fresh else _latest(state, binding.storage_namespace)
        if row and row["policy"] not in ("", policy):
            raise HistoryError("master_policy_changed")
        if row is None:
            if not fresh and any(_same_account(item, binding) for item in state["conversations"]):
                raise HistoryError("master_storage_changed")
            row = _row(binding)
        if parameters is not None:
            choices = [parameters["model"], parameters["effort"]]
            if row["session_id"] and row.get("choices", choices) != choices:
                raise HistoryError("master_choices_changed")
            row["choices"] = choices
        row["policy"] = policy
        _put(state, row)
        state["launch"] = {"launch_id": launch or launch_id(), "actor": row["actor"], "parameters": parameters or {}}
        return {**row, **state["launch"]}


def import_legacy(home: Path, slot: str, binding: Binding, session_id: str, title: str) -> None:
    """Import a legacy title and session against a trusted engine-resolved account."""
    if not acp.valid_session_id(session_id) or not isinstance(title, str) or len(title) > MAX_TITLE:
        raise HistoryError("master_invalid_history")
    with _edit(home, slot) as state:
        if any(row["binding"]["storage_namespace"] == binding.storage_namespace and row["session_id"] == session_id
               for row in state["conversations"]):
            return
        existing = _latest(state, binding.storage_namespace)
        row = {**_row(binding), "title": title, "session_id": session_id, "guidance_pending": False}
        if existing:
            state["conversations"].insert(state["conversations"].index(existing), row)
        else:
            state["conversations"].append(row)


def current(home: Path, slot: str, launch: str) -> dict:
    """Return the active launch or refuse an outcome from a superseded one."""
    return _current(_read(home, slot), launch)


def opened(home: Path, slot: str, launch: str, session_id: str) -> dict:
    """Atomically accept the reported opened session for the current launch."""
    if not acp.valid_session_id(session_id):
        raise HistoryError("master_invalid_history")
    with _edit(home, slot) as state:
        row = _current(state, launch)
        if row["session_id"] and row["session_id"] != session_id:
            raise HistoryError("master_binding_conflict")
        changed = {key: value for key, value in row.items() if key not in {"launch_id", "parameters"}}
        _put(state, {**changed, "session_id": session_id})
        return {**row, "session_id": session_id}


def record_storage(home: Path, slot: str, launch: str, identity: list[int]) -> None:
    """Pin the engine-owned Goose session storage to the active conversation."""
    with _edit(home, slot) as state:
        row = _current(state, launch)
        saved = next(item for item in state["conversations"] if item["actor"] == row["actor"])
        if saved.get("model_storage") not in (None, identity):
            raise HistoryError("master_storage_changed")
        saved["model_storage"] = identity


def prompted(home: Path, slot: str, launch: str) -> None:
    """Record the first completed turn so its guidance is part of replay history."""
    with _edit(home, slot) as state:
        row = _current(state, launch)
        if not row["session_id"]:
            raise HistoryError("master_binding_conflict")
        next(item for item in state["conversations"] if item["actor"] == row["actor"])["guidance_pending"] = False


def listed(home: Path, slot: str) -> list[dict]:
    """List the saved titles and bindings, including previous account selections."""
    state = _read(home, slot)
    return [public(row) for row in state["conversations"] if row["session_id"]]


def public(row: dict) -> dict:
    """Publish a saved conversation's identity without exposing filesystem paths."""
    return {"conversation_id": row["actor"], "session_id": row["session_id"], "title": row["title"],
            "binding": Binding(**row["binding"]).document()}


def transcript(home: Path, slot: str, selector: str) -> dict:
    """Read a saved Codex transcript by opaque conversation id or bound namespace."""
    from sakura.master import legacy_transcript

    rows = _read(home, slot)["conversations"]
    row = next((item for item in reversed(rows) if item["session_id"] and
                selector in (item["actor"], item["binding"]["storage_namespace"])), None)
    if row is None:
        raise HistoryError("master_history_missing")
    messages, reason = legacy_transcript.read(Binding(**row["binding"]), row["session_id"])
    return {**public(row), "outcome": "transcript" if messages else "unavailable",
            "reason": reason, "messages": messages, "read_only": True}


@contextmanager
def turning(home: Path, slot: str):
    """Serialize operations in one slot while account changes can revoke a launch."""
    path = _path(home, slot).with_suffix(".turn")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(fd, "rb") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise HistoryError("master_turn_in_progress") from None
        yield


def _row(binding: Binding) -> dict:
    return {"actor": uuid4().hex, "binding": asdict(binding), "session_id": "", "title": "", "policy": "",
            "guidance_pending": True}


def _same_account(row: dict, binding: Binding) -> bool:
    named = row["binding"]
    return (named["harness"], named["account"]) == (binding.harness, binding.account)


def _latest(state: dict, namespace: str) -> dict | None:
    return next((row for row in reversed(state["conversations"])
                 if row["binding"]["storage_namespace"] == namespace), None)


def _put(state: dict, row: dict) -> None:
    existing = next((item for item in state["conversations"] if item["actor"] == row["actor"]), None)
    if existing is None:
        state["conversations"].append(row)
    else:
        existing.update(row)


def _current(state: dict, launch: str) -> dict:
    active = state["launch"]
    if not isinstance(launch, str) or not LAUNCH.fullmatch(launch) or not active or active["launch_id"] != launch:
        raise HistoryError("master_launch_changed")
    row = next(item for item in state["conversations"] if item["actor"] == active["actor"])
    return {**row, **active}


def _path(home: Path, slot: str) -> Path:
    if not isinstance(slot, str) or not SLOT.fullmatch(slot):
        raise HistoryError("master_invalid_slot")
    return home / "master" / "history" / f"{slot}.json"


def _read(home: Path, slot: str) -> dict:
    path = _path(home, slot)
    try:
        data = json.loads(beneath.read_capped(home, str(path.relative_to(home)), MAX_STATE))
        if not isinstance(data["conversations"], list) or "launch" not in data:
            raise ValueError
    except FileNotFoundError:
        return {"conversations": [], "launch": None}
    except (OSError, ValueError, TypeError, KeyError, RecursionError):
        raise HistoryError("master_history_unavailable") from None
    return data


@contextmanager
def _edit(home: Path, slot: str):
    target = _path(home, slot)
    with write.locked(target):
        state = _read(home, slot)
        yield state
        data = json.dumps(state)
        if len(data.encode()) > MAX_STATE:
            raise HistoryError("master_history_limit")
        write.replace_atomically(target, data)
