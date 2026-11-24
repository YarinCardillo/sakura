"""Make agent output safe and readable on an operator's terminal."""

from __future__ import annotations

import json
import re
import time
import unicodedata
from pathlib import Path
from typing import Callable, Iterable, TextIO

from sakura.flow import run_records
from sakura.flow.store import TaskStore

# String sequences (OSC, DCS, SOS, PM, APC) in 7-bit and 8-bit form, ended by
# BEL or ST, or by the end of the text when the terminator never arrives.
_STRING = re.compile(r"(?:\x1b[\]PX^_]|[\x90\x98\x9d\x9e\x9f]).*?(?:\x07|\x1b\\|\x9c|\Z)", re.DOTALL)
_CSI = re.compile(r"(?:\x1b\[|\x9b)[0-?]*[ -/]*[@-~]?")
_ESCAPE = re.compile(r"\x1b[ -/]*[0-~]?")
_CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")
# Format characters (bidi overrides and isolates, zero-width marks), line and paragraph separators: they reorder
# or hide what an operator reads without being control characters. Lone surrogates (Cs, how Python carries
# bytes that are not valid UTF-8) cannot be printed or encoded at all.
_INVISIBLE_CATEGORIES = frozenset({"Cf", "Zl", "Zp", "Cs"})


def strip_controls(text: str) -> str:
    """Remove every terminal control sequence, control character, invisible Unicode format or separator
    character and lone surrogate, except newline and tab."""
    for pattern in (_STRING, _CSI, _ESCAPE, _CONTROL):
        text = pattern.sub("", text)
    if text.isascii():
        return text
    return "".join(c for c in text if unicodedata.category(c) not in _INVISIBLE_CATEGORIES)


# -- readable lines from agent logs --------------------------------------------

TOOL_INPUT_CHARS = 200
# Bound for numbers taken from untrusted logs, so arithmetic and formatting cannot overflow.
MAX_COUNT = 10**15
TOOL_RESULT_LINES = 3
TERMINAL_RUN_STATES = ("finished", "aborted", "orphaned")
DEFAULT_POLL_SECONDS = 0.5
CLAUDE_TOKEN_KEYS = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")
CODEX_TOKEN_KEYS = ("input_tokens", "output_tokens")


def render_line(provider: str, line: str) -> str | None:
    """Return the readable form of one log line, or None when the line is noise.

    Provider JSON events are summarized; a line that is not a JSON object is shown as is. The result never
    carries terminal control sequences.
    """
    event = _json_object(line)
    renderer = _EVENT_RENDERERS.get(provider)
    if not event or renderer is None:
        return strip_controls(line)
    text = renderer(event)
    if text is None:
        return None
    return strip_controls(text) or None


def _json_object(line: str) -> dict:
    if not line.startswith("{"):
        return {}
    try:
        event = json.loads(line)
    except (ValueError, RecursionError):
        return {}
    return event if isinstance(event, dict) else {}


def _text(value: object) -> str:
    return value if isinstance(value, str) else ""


def _count(value: object) -> int:
    if not isinstance(value, (int, float)) or isinstance(value, bool) or value != value:
        return 0
    return max(-MAX_COUNT, min(MAX_COUNT, value))


def _claude_event(event: dict) -> str | None:
    handler = _CLAUDE_EVENTS.get(event.get("type"))
    return handler(event) if handler else None


def _claude_system(event: dict) -> str | None:
    if event.get("subtype") != "init":
        return None
    return f"session {_text(event.get('session_id'))} model {_text(event.get('model'))}"


def _claude_blocks(event: dict) -> list[dict]:
    message = event.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    return [block for block in content if isinstance(block, dict)] if isinstance(content, list) else []


def _claude_assistant(event: dict) -> str | None:
    shown = [_claude_assistant_block(block) for block in _claude_blocks(event)]
    return "\n".join(part for part in shown if part) or None


def _claude_assistant_block(block: dict) -> str:
    if block.get("type") == "text":
        return _text(block.get("text"))
    if block.get("type") == "tool_use":
        arguments = json.dumps(block.get("input"), ensure_ascii=False)[:TOOL_INPUT_CHARS]
        return f"> {_text(block.get('name'))} {arguments}"
    return ""


def _claude_user(event: dict) -> str | None:
    shown = [_tool_result_text(block.get("content")) for block in _claude_blocks(event) if block.get("type") == "tool_result"]
    return "\n".join(part for part in shown if part) or None


def _tool_result_text(content: object) -> str:
    if isinstance(content, list):
        content = "\n".join(_text(part.get("text")) for part in content if isinstance(part, dict))
    return "\n".join(_text(content).splitlines()[:TOOL_RESULT_LINES])


def _claude_result(event: dict) -> str:
    usage = event.get("usage") if isinstance(event.get("usage"), dict) else {}
    tokens = sum(_count(usage.get(key)) for key in CLAUDE_TOKEN_KEYS)
    seconds = _count(event.get("duration_ms")) / 1000
    return f"result: {_text(event.get('subtype'))} in {seconds:.1f} s, {tokens} tokens"


def _codex_event(event: dict) -> str | None:
    kind = event.get("type")
    if kind == "thread.started":
        return f"session {_text(event.get('thread_id'))}"
    if kind == "item.completed" and isinstance(event.get("item"), dict):
        return _codex_item(event["item"])
    if kind == "turn.completed":
        usage = event.get("usage") if isinstance(event.get("usage"), dict) else {}
        return f"turn: {sum(_count(usage.get(key)) for key in CODEX_TOKEN_KEYS)} tokens"
    return None


def _codex_item(item: dict) -> str | None:
    if item.get("type") == "agent_message":
        return _text(item.get("text"))
    if item.get("type") == "command_execution":
        return f"> {_text(item.get('command'))} (exit {item.get('exit_code')})"
    return None


_CLAUDE_EVENTS = {"system": _claude_system, "assistant": _claude_assistant, "user": _claude_user,
                  "result": _claude_result}
_EVENT_RENDERERS = {"claude": _claude_event, "codex": _codex_event}


# -- following logs ------------------------------------------------------------


class _LineTail:
    """Reads a growing file line by line; a final line without a newline is held back until flushed."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._offset = 0
        self._partial = b""

    def read_lines(self) -> list[str]:
        """Return the complete lines appended since the last call."""
        try:
            with self._path.open("rb") as stream:
                stream.seek(self._offset)
                data = stream.read()
        except FileNotFoundError:
            return []
        self._offset += len(data)
        *complete, self._partial = (self._partial + data).split(b"\n")
        return [line.decode(errors="replace") for line in complete]

    def flush(self) -> list[str]:
        """Return the held-back partial line, if any."""
        rest, self._partial = self._partial, b""
        return [rest.decode(errors="replace")] if rest else []


def follow(task_store: TaskStore, run: str, out: TextIO, poll_seconds: float = DEFAULT_POLL_SECONDS) -> None:
    """Render run `run` as its log grows, until its record has ended and the log is drained.

    Raises RunRecordError when the run is not recorded.
    """
    record = _run_record(task_store, run)
    tail = _LineTail(run_records.log_path(task_store, run))
    while True:
        has_ended = record["state"] in TERMINAL_RUN_STATES
        lines = tail.read_lines() + (tail.flush() if has_ended else [])
        _print_rendered(out, (render_line(record["provider"], line) for line in lines))
        if has_ended:
            break
        time.sleep(poll_seconds)
        record = _run_record(task_store, run)
    _print_run_end(task_store, record, out)


def follow_ledger(task_store: TaskStore, out: TextIO, poll_seconds: float = DEFAULT_POLL_SECONDS,
                  should_stop: Callable[[], bool] = lambda: False) -> None:
    """Print the task ledger from its start and keep printing new lines until `should_stop` or the caller's end."""
    tail = _LineTail(task_store.ledger)
    while True:
        is_last = should_stop()
        _print_rendered(out, (strip_controls(line) for line in tail.read_lines() + (tail.flush() if is_last else [])))
        if is_last:
            return
        time.sleep(poll_seconds)


def _run_record(task_store: TaskStore, run: str) -> dict:
    record = task_store.read_state().get("runs", {}).get(run)
    if record is None:
        raise run_records.RunRecordError(f"run {strip_controls(run)} is not recorded")
    return record


def _print_rendered(out: TextIO, lines: Iterable[str | None]) -> None:
    shown = [line for line in lines if line is not None]
    if shown:
        out.write("\n".join(shown) + "\n")
        out.flush()


def _print_run_end(task_store: TaskStore, record: dict, out: TextIO) -> None:
    code = record.get("exit_code")
    summary = f"run {record['name']} {record['state']}, exit {'unknown' if code is None else code}"
    hint = (f"reopen: sakura flow open {task_store.task_id} {record['name']}" if record.get("session_id") else "")
    _print_rendered(out, (strip_controls(part) for part in (summary, hint) if part))
