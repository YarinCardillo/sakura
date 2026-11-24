"""What an interactive agent leaves behind: its pane record and, for Claude, the conversation transcript."""

from __future__ import annotations

import json
import re
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from sakura.flow.tmux import TmuxClient, TmuxError

DEFAULT_RECORDS_DIR = "~/.local/state/tmux-agents"
CLAUDE_TOOL = "claude"
TAIL_BYTES = 1 << 20
SYNTHETIC_MODEL = "<synthetic>"
MODEL_ACK = "Set model to"

SESSION_ID = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
_SLUG_CHARS = re.compile(r"[^A-Za-z0-9]")
_NON_ALNUM = re.compile(r"[^a-z0-9]+")
_MODEL_ACK_NAME = re.compile(re.escape(MODEL_ACK) + r"\s+([^<\n]+)")


@dataclass(frozen=True)
class Sources:
    """Where hooks and Claude keep their files; None means the person's real location."""

    records_dir: Path | None = None
    home: Path | None = None

    def records(self) -> Path:
        """The directory of `tmux-agent-record` files."""
        return Path(self.records_dir or DEFAULT_RECORDS_DIR).expanduser()

    def claude_home(self) -> Path:
        """The directory holding `.claude` when the record names no config directory."""
        return Path(self.home) if self.home else Path.home()


def read_record(client: TmuxClient, pane: str, sources: Sources) -> dict | None:
    """Return the pane's `tmux-agent-record` JSON plus `recorded_at` and `server_started_at`, or None.

    None when the file is missing or malformed, or when tmux cannot say when its server started.
    """
    path = sources.records() / f"{pane[1:]}.json"
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
        recorded_at, started_at = path.stat().st_mtime, client.server_start_time(pane)
    except (OSError, ValueError, TmuxError):
        return None
    if not isinstance(record, dict):
        return None
    return {**record, "recorded_at": recorded_at, "server_started_at": float(started_at)}


def is_current(record: dict | None, worktree: Path) -> bool:
    """Return whether the record was written in the lane worktree since the tmux server started.

    Pane ids restart at %0 with a new server, so an older file under the same id belongs to another pane.
    """
    if not record or not isinstance(record.get("cwd"), str) or not record["cwd"]:
        return False
    if record.get("recorded_at", 0) < record.get("server_started_at", float("inf")):
        return False
    return _same_directory(Path(record["cwd"]), worktree)


def locate(record: dict | None, worktree: Path, sources: Sources) -> Path | None:
    """Return the Claude transcript of the record's conversation, or None when it cannot be vouched for."""
    if not is_current(record, worktree) or record.get("tool") != CLAUDE_TOOL:
        return None
    session = record.get("session_id")
    if not isinstance(session, str) or not SESSION_ID.fullmatch(session):
        return None
    slug = _SLUG_CHARS.sub("-", str(worktree.resolve()))
    path = _config_dir(record, sources) / "projects" / slug / f"{session}.jsonl"
    return path if path.is_file() else None


def model(path: Path) -> str:
    """Return the model of the latest assistant message in the transcript, or "" when none is readable."""
    for entry in _recent_entries(path):
        found = _assistant_model(entry)
        if found:
            return found
    return ""


def model_change_seen(path: Path, requested: str, since: float) -> bool:
    """Return whether the transcript shows, after epoch time `since`, the model change `requested`.

    Shown by an assistant message whose model name contains `requested`, or by Claude's own
    "Set model to <display name>" reply to the /model command when that name contains `requested`.
    """
    wanted = requested.lower()
    for entry in _recent_entries(path):
        if _entry_time(entry) < since:
            continue
        found, acknowledged = _assistant_model(entry).lower(), _acknowledged_model(entry)
        if (found and wanted in found) or (acknowledged and _slug(requested) in _slug(acknowledged)):
            return True
    return False


def _config_dir(record: dict, sources: Sources) -> Path:
    named = record.get("claude_config_dir")
    if isinstance(named, str) and named and Path(named).is_absolute():
        return Path(named)
    return sources.claude_home() / ".claude"


def _same_directory(left: Path, right: Path) -> bool:
    try:
        return left.resolve() == right.resolve()
    except OSError:
        return False


def _recent_entries(path: Path) -> Iterator[dict]:
    """Yield the transcript's JSON objects, newest first, from its last TAIL_BYTES only."""
    try:
        with path.open("rb") as handle:
            size = handle.seek(0, 2)
            handle.seek(max(0, size - TAIL_BYTES))
            chunk = handle.read()
        lines = chunk.splitlines()
        if size > TAIL_BYTES:
            lines = lines[1:]
    except OSError:
        return
    for line in reversed(lines):
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if isinstance(entry, dict):
            yield entry


def _assistant_model(entry: dict) -> str:
    message = entry.get("message")
    if entry.get("type") != "assistant" or not isinstance(message, dict):
        return ""
    found = message.get("model")
    return found if isinstance(found, str) and found != SYNTHETIC_MODEL else ""


def _user_text(entry: dict) -> str:
    message = entry.get("message")
    content = message.get("content") if entry.get("type") == "user" and isinstance(message, dict) else None
    return content if isinstance(content, str) else ""


def _acknowledged_model(entry: dict) -> str:
    """The model named by a "Set model to ..." reply, or "" when the entry is not one."""
    found = _MODEL_ACK_NAME.search(_user_text(entry))
    return found.group(1) if found else ""


def _slug(name: str) -> str:
    """Lowercase `name` with runs of other characters as one hyphen, so "Sonnet 5.5" and "sonnet-5-5" agree."""
    return _NON_ALNUM.sub("-", name.lower()).strip("-")


def _entry_time(entry: dict) -> float:
    try:
        return datetime.fromisoformat(entry["timestamp"]).timestamp()
    except (KeyError, TypeError, ValueError):
        return 0.0
