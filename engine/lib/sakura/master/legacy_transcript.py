"""Recover human-visible messages from the account's old Codex rollouts."""

import json
import os
import re
from pathlib import Path

from sakura import beneath
from sakura.master.history import Binding

MAX_FILE = 4 * 1024 * 1024
MAX_ENTRIES = 20000
MAX_MESSAGES = 1000
THREAD = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{0,127}")
DATE_DIR = re.compile(r"(?:[0-9]{2}|[0-9]{4})")


def read(binding: Binding, session_id: str) -> tuple[list[dict], str | None]:
    """Return only validated user and assistant transcript text."""
    if binding.harness != "codex" or not THREAD.fullmatch(session_id):
        return [], "master_transcript_unavailable"
    root = Path(binding.folder)
    try:
        if not _same_root(root, binding):
            return [], "master_storage_changed"
        candidates = _candidates(root, session_id)
        if len(candidates) == 1:
            data = beneath.read_capped(root, candidates[0], MAX_FILE)
            messages = _messages(data, session_id)
            if messages and _same_root(root, binding):
                return messages, None
    except (OSError, ValueError, RuntimeError):
        pass
    return [], "master_transcript_unavailable"


def _same_root(root: Path, binding: Binding) -> bool:
    info = root.stat()
    return root.resolve(strict=True) == root and (info.st_dev, info.st_ino) == tuple(binding.folder_identity)


def _candidates(root: Path, session_id: str) -> list[str]:
    suffix = f"-{session_id}.jsonl"
    budget = [MAX_ENTRIES]
    return [*_scan(root, ("sessions",), 3, suffix, budget), *_scan(root, ("archived_sessions",), 0, suffix, budget)]


def _scan(root: Path, components: tuple[str, ...], depth: int, suffix: str, budget: list[int]):
    try:
        with beneath._folder(root, components, create=False) as fd, os.scandir(fd) as entries:
            for entry in entries:
                budget[0] -= 1
                if budget[0] < 0:
                    raise ValueError("transcript search limit")
                if depth and DATE_DIR.fullmatch(entry.name) and entry.is_dir(follow_symlinks=False):
                    yield from _scan(root, (*components, entry.name), depth - 1, suffix, budget)
                elif (not depth and entry.name.startswith("rollout-") and entry.name.endswith(suffix)
                      and entry.is_file(follow_symlinks=False)):
                    yield "/".join((*components, entry.name))
    except FileNotFoundError:
        return


def _messages(data: bytes, session_id: str) -> list[dict]:
    first, _, rest = data.partition(b"\n")
    meta = json.loads(first)
    if not isinstance(meta, dict) or meta.get("type") != "session_meta" or _payload(meta).get("id") != session_id:
        return []
    events, response = [], []
    for line in rest.splitlines():
        row = json.loads(line)
        if not isinstance(row, dict):
            continue
        payload = _payload(row)
        if row.get("type") == "event_msg":
            message = _event(payload)
            if message:
                events.append(message)
        elif row.get("type") == "response_item":
            message = _response(payload)
            if message:
                response.append(message)
        if max(len(events), len(response)) > MAX_MESSAGES:
            raise ValueError("transcript message limit")
    return events or response


def _payload(row: dict) -> dict:
    data = row.get("payload")
    return data if isinstance(data, dict) else {}


def _event(payload: dict) -> dict | None:
    kind, text = payload.get("type"), payload.get("message")
    role = {"user_message": "user", "agent_message": "assistant"}.get(kind) if isinstance(kind, str) else None
    return {"role": role, "text": text} if role and isinstance(text, str) and text else None


def _response(payload: dict) -> dict | None:
    role, content = payload.get("role"), payload.get("content")
    if payload.get("type") != "message" or role not in ("user", "assistant") or not isinstance(content, list):
        return None
    text = "\n".join(item["text"] for item in content if isinstance(item, dict)
                     and item.get("type") in ("input_text", "output_text") and isinstance(item.get("text"), str))
    return {"role": role, "text": text} if text else None
