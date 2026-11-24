"""The phone transport's target references and host-detail filtering."""

from __future__ import annotations

import hmac
import json
import re
import threading
from collections import OrderedDict
from contextvars import ContextVar
from dataclasses import replace
from pathlib import Path

from sakura import public_links
from sakura.cockpit import actions
from sakura.flow.store import TASK_ID

MAX_IDS = 32768
ID_PREFIX = "t1."
ABSOLUTE_PATH = re.compile(r"(?<![\w:/])/(?!/)[^\s\"'<>\])]+")
TARGET_KINDS = frozenset({"flow", "lane", "group", "outside", "digest", "account", "host", "project", "wait", "scheduled"})
PRIVATE_FIELDS = frozenset({"technical", "next_command", "worktree", "folder", "pending", "command", "argv", "cwd", "prompt"})
SAFE_REFUSALS = frozenset({
    "this control is only available on the machine the cockpit runs on",
    "no longer waiting: the engine has moved on since the page was drawn; nothing was run",
    "no such target; it may have changed since the page was drawn",
})
REFUSED = "the cockpit could not accept the action"
ACTION_FAILED = "the engine could not complete the action"
AUDIT_FAILED = "the action audit could not be saved"


class Ids:
    """Bounded, process-local registry of references emitted by the phone transport."""

    def __init__(self, secret: bytes) -> None:
        self._secret = secret
        self._keys: OrderedDict[str, tuple] = OrderedDict()
        self._lock = threading.Lock()

    def register(self, key: tuple) -> str:
        payload = json.dumps(key, separators=(",", ":")).encode()
        reference = ID_PREFIX + hmac.digest(self._secret, b"phone-target\0" + payload, "sha256").hex()
        with self._lock:
            self._keys[reference] = key
            self._keys.move_to_end(reference)
            if len(self._keys) > MAX_IDS:
                self._keys.popitem(last=False)
        return reference

    def lookup(self, reference: str) -> tuple | None:
        if not isinstance(reference, str):
            return None
        with self._lock:
            return self._keys.get(reference)


VIEW_IDS: ContextVar[Ids | None] = ContextVar("phone_view_ids", default=None)


def text(value: str) -> str:
    """Remove embedded filesystem paths from phone display text."""
    return ABSOLUTE_PATH.sub("[host path]", public_links.text(value))


def result(key: str, done: actions.ActionResult) -> actions.ActionResult:
    """Keep only the exit code, audit status and a validated created lane id."""
    output = ""
    if done.exit_code != 0:
        output = ACTION_FAILED
    elif key == "start" and TASK_ID.fullmatch(done.output):
        output = done.output
    return replace(done, output=output, audit=AUDIT_FAILED if done.audit else "")


def document(value, ids: Ids):
    """Project a local snapshot document into the phone transport."""
    if isinstance(value, list):
        return [document(element, ids) for element in value]
    if not isinstance(value, dict):
        return text(value) if isinstance(value, str) else value
    key = _wire_key(value.get("target")) or _wire_key(value.get("id"))
    result = {}
    for name, element in value.items():
        if name in ("tracks", "track_actions", "track_choices", "track_problem", "track"):
            continue
        if name == "key" and isinstance(element, list):
            continue
        if name in ("id", "target") and (reference := _wire_key(element)):
            result[name] = ids.register(reference)
        elif name in PRIVATE_FIELDS:
            result[name] = [] if isinstance(element, list) else None if element is None else ""
        elif name == "repo" and isinstance(element, str) and element.startswith("/"):
            result[name] = Path(element).name
        elif name == "project" and isinstance(element, str) and element.startswith("/"):
            result[name] = ids.register(("project", element))
        elif name == "detail" and value.get("subkind") == "scheduled_approval":
            result[name] = ""
        elif name == "events":
            result[name] = []
        elif name == "actions" and key:
            result[name] = [_action(action) for action in element if not actions.is_local_only(action["key"], key)]
        else:
            result[name] = document(element, ids)
    return result


def _wire_key(value: object) -> tuple | None:
    if not isinstance(value, str) or not value.startswith("["):
        return None
    try:
        key = json.loads(value)
    except (ValueError, RecursionError):
        return None
    if (isinstance(key, list) and len(key) in (2, 3, 5) and all(isinstance(word, str) for word in key)
            and key[0] in TARGET_KINDS):
        return tuple(key)
    return None


def _action(payload: dict) -> dict:
    action = actions.ACTIONS.get(payload["key"])
    if action is None or not action.local_inputs:
        return payload
    return {**payload, "optional": [name for name in payload["optional"] if name not in action.local_inputs]}
