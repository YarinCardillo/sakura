"""Reads a role run's log into entries: what the agent said and which tools it called.

A Claude run logs `stream-json` events and a Codex run `--json` events, one JSON object per line; other lines
(the command line the engine echoes first, anything a tool printed) are skipped. An entry is either `text` (what the
agent wrote) or `tool` (a tool call, with the path and first line it names when the tool takes one, and the
command of a shell call). The agent's
private reasoning and the results tools returned are not entries. Everything here is the agent's own words and
choices, unchecked: `flow/feed.py` bounds and cleans it before it reaches a person.

Append-only: new log lines only add entries after the earlier ones, so an entry's position in the list never moves.
A block or item the reader does not know counts as a tool, so a new provider kind fails closed in `roles.ran_tools`.
"""

from __future__ import annotations

import json
import re
from collections.abc import Iterator
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from typing import TYPE_CHECKING

from sakura import text_limits
from sakura.flow import acp, live_input

if TYPE_CHECKING:
    from sakura.flow.roles import RoleSpec

TEXT = "text"
TOOL = "tool"
YOU = "you"
MESSAGE_EVENT = "sakura_message"  # a line the engine logs for a message it gives a Codex run, which echoes none
UNKNOWN_TOOL = "unknown"
CODEX_TEXT_FIELDS = {"agent_message": "text", "error": "message"}
CODEX_HIDDEN_ITEMS = frozenset({"reasoning", "todo_list"})
CODEX_PROGRESS_EVENTS = frozenset({"item.started", "item.updated"})
CLAUDE_HIDDEN_BLOCKS = frozenset({"thinking", "redacted_thinking"})
PATH_KEYS = ("file_path", "notebook_path", "path")
TARGET_WORDS = 6  # the words of a command that say what it runs
TIME_KEY = "timestamp"
MILLISECOND_EPOCH_FROM = 1e11  # an epoch number this large is milliseconds, not seconds
MILLISECONDS_PER_SECOND = 1000


@dataclass(frozen=True)
class LogEntry:
    """One thing a run did: `text` it wrote, or a `tool` call with the `path` and `line` it names ("" and None if none);
    `at` is the UTC time its event carried ("" when the log stamps none).

    Where it sits in the log, which only `scan` fills: `offset` and `end` are the bytes its event line starts and ends
    at, `block` its place among the entries of that line, and `tool_id` the id a tool's result names (these never
    take part in a comparison, so two entries are equal when they say the same thing)."""

    kind: str
    text: str
    tool: str = ""
    path: str = ""
    line: int | None = None
    at: str = ""
    command: str = ""
    target: str = field(default="", compare=False)
    offset: int = field(default=-1, compare=False)
    end: int = field(default=-1, compare=False)
    block: int = field(default=0, compare=False)
    tool_id: str = field(default="", compare=False)
    can_report_verdict: bool = True


@dataclass(frozen=True)
class Scan:
    """What `scan` found: the `entries` of the whole lines it read, `closed` (tool id -> the byte its result event ends
    at) for every tool whose result it saw, `end`, the byte where the last whole line ends, `live`, the text of the
    message being written (its text deltas since the last message completed), and `settled`, the byte from which only
    the deltas of that message follow, and `reset_seen`, whether this read met the start or the end of a message (so the live
    text it holds begins there, not before)."""

    entries: list[LogEntry]
    closed: dict[str, int]
    end: int
    live: str = ""
    settled: int = 0
    reset_seen: bool = False


def entries(spec: RoleSpec, log_text: str) -> list[LogEntry]:
    """The entries of `log_text`, a log of the run `spec` describes, oldest first."""
    return scan(spec, log_text.encode(), is_complete=True).entries


def scan(spec: RoleSpec, data: bytes, base: int = 0, is_complete: bool = False, keep_messages: bool = False,
         known_tools: frozenset[str] | set[str] = frozenset()) -> Scan:
    """Read the log bytes `data`, which start at byte `base` of the log, as entries with their places in it.

    Only whole lines are read: a last line without its newline is left for the next read (`Scan.end` says where this
    one stopped) unless `is_complete` says the log is over. With `keep_messages` the messages sent to the agent
    (a Claude run's replayed user lines, the lines the engine logs for a Codex resume) are `you` entries; the first of a
    run's replays is the engine's own prompt, which the caller knows to skip. `known_tools` are the ids of tools an earlier
    read already made entries of: a Codex tool item that names one only closes it."""
    found: list[LogEntry] = []
    closed: dict[str, int] = {}
    read_codex = spec.provider == "codex"
    seen_tools: set[str] = set(known_tools)
    live = LiveTail(base)
    stopped = base
    for offset, end, event in lines(data, base, is_complete):
        live.take(event, offset, end)
        if spec.provider == "goose":
            made = _acp_event(event, seen_tools, closed, end)
        else:
            made = _codex_event(event, seen_tools, closed, end) if read_codex else _claude_event(event, closed, end)
        made += _message_entries(event, keep_messages)
        found.extend(replace(entry, at=_event_time(event), offset=offset, end=end, block=index)
                     for index, entry in enumerate(made))
        stopped = end
    return Scan(found, closed, stopped, "".join(live.parts), live.settled, live.reset_seen)


class LiveTail:
    """The text deltas of the message being written, and where the settled part of the log ends; fed event by event, so a
    reader can follow a message of any length in as many reads as it takes."""

    def __init__(self, start: int) -> None:
        self.parts: list[str] = []
        self.settled = start
        self.reset_seen = False

    def take(self, event: dict, offset: int, end: int) -> None:
        """Take the event of the line from byte `offset` to `end`."""
        kind = event.get("type")
        if kind == acp.DELTA_EVENT and isinstance(event.get("text"), str):
            if not self.parts:
                self.settled = offset
            self.parts.append(event["text"])
        elif kind in (acp.FINAL_EVENT, acp.MESSAGE_EVENT, acp.ERROR_EVENT):
            self.parts, self.settled, self.reset_seen = [], end, True
        elif kind == "stream_event":
            self._stream(event, offset)
        elif kind in ("assistant", "user"):
            self.parts, self.settled, self.reset_seen = [], end, True
        elif not self.parts:
            self.settled = end

    def _stream(self, event: dict, offset: int) -> None:
        inner = event.get("event")
        if event.get("parent_tool_use_id") is not None or not isinstance(inner, dict):
            return
        if inner.get("type") == "message_start":
            self.parts, self.settled, self.reset_seen = [], offset, True
        delta = inner.get("delta")
        if inner.get("type") == "content_block_delta" and isinstance(delta, dict) and delta.get("type") == "text_delta" \
                and isinstance(delta.get("text"), str):
            self.parts.append(delta["text"])


def _message_entries(event: dict, keep_messages: bool) -> list[LogEntry]:
    if not keep_messages:
        return []
    if event.get("type") == MESSAGE_EVENT:
        words = event.get("text")
    else:
        is_replay = event.get("type") == "user" and event.get("parent_tool_use_id") is None  # a subagent's prompt is agent text
        words = live_input.replayed_text(event) if is_replay else None
    return [LogEntry(YOU, words)] if isinstance(words, str) and words else []


def lines(data: bytes, base: int, is_complete: bool = False) -> Iterator[tuple[int, int, dict]]:
    """The (start, end, event) of every whole line of `data`; `end` is past the newline and `event` is empty for a line
    that is not a JSON object."""
    offset, finish = base, base + len(data)
    for raw in data.split(b"\n"):
        if offset + len(raw) >= finish and not (is_complete and raw):
            return
        end = min(offset + len(raw) + 1, finish)
        try:
            event = json.loads(raw)
        except (ValueError, RecursionError):
            event = None
        yield offset, end, event if isinstance(event, dict) else {}
        offset = end


def _claude_event(event: dict, closed: dict[str, int], end: int) -> list[LogEntry]:
    message = event.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    if not isinstance(content, list):
        return []
    if event.get("type") != "assistant":
        closed.update({block["tool_use_id"]: end for block in content if _is_result(block)})
        return []
    return [entry for entry in map(_claude_block_entry, content) if entry]


def _is_result(block: object) -> bool:
    return isinstance(block, dict) and block.get("type") == "tool_result" and isinstance(block.get("tool_use_id"), str)


def _claude_block_entry(block: object) -> LogEntry | None:
    if not isinstance(block, dict):
        return LogEntry(TOOL, "", UNKNOWN_TOOL)
    kind = block.get("type")
    if kind == "text":
        return LogEntry(TEXT, block["text"]) if isinstance(block.get("text"), str) and block["text"] else None
    if kind in CLAUDE_HIDDEN_BLOCKS:
        return None
    if kind == "tool_use":
        arguments = block.get("input") if isinstance(block.get("input"), dict) else {}
        identity = block.get("id")
        return LogEntry(TOOL, "", _name(block.get("name")), _first_path(arguments), _line(arguments.get("offset")),
                        command=_command(arguments.get("command")), target=_target(arguments),
                        tool_id=identity if isinstance(identity, str) else "")
    return LogEntry(TOOL, "", _name(kind))



def _acp_event(event: dict, seen_tools: set[str], closed: dict[str, int], end: int) -> list[LogEntry]:
    kind = event.get("type")
    text = event.get("text")
    if kind in (acp.FINAL_EVENT, acp.MESSAGE_EVENT) and isinstance(text, str) and text:
        return [LogEntry(TEXT, text, can_report_verdict=kind == acp.FINAL_EVENT)]
    if kind != acp.TOOL_EVENT or not isinstance(identity := event.get("tool_id"), str):
        return []
    if event.get("closed") is True:
        closed[identity] = end
    if event.get("starts") is True and identity not in seen_tools:
        seen_tools.add(identity)
        return [LogEntry(TOOL, "", _name(event.get("title")), tool_id=identity)]
    return []

def _codex_event(event: dict, seen_tools: set[str], closed: dict[str, int], end: int) -> list[LogEntry]:
    """A tool item is one entry, made at the first event that names it (`item.started` or `item.completed`); its
    `item.completed` closes it."""
    item = event.get("item")
    if not isinstance(item, dict):
        return []
    is_progress = event.get("type") in CODEX_PROGRESS_EVENTS
    was_seen = item.get("id") in seen_tools
    entry = _codex_item_entry(item, is_progress, seen_tools)
    if not is_progress and isinstance(item.get("id"), str) and (was_seen or (entry and entry.kind == TOOL)):
        closed[item["id"]] = end
    return [entry] if entry else []


def _codex_item_entry(item: dict, is_progress: bool, seen_tools: set[str]) -> LogEntry | None:
    kind = item.get("type")
    if kind in CODEX_TEXT_FIELDS:
        text = item.get(CODEX_TEXT_FIELDS[kind])
        return LogEntry(TEXT, text) if not is_progress and isinstance(text, str) and text else None
    if kind in CODEX_HIDDEN_ITEMS:
        return None
    identity = item.get("id")
    if isinstance(identity, str):
        if identity in seen_tools:
            return None
        seen_tools.add(identity)
    target = _codex_target(item)
    return LogEntry(TOOL, "", _name(kind), path=target if _is_changed_file(item) else "",
                    command=_command(item.get("command")), target=target, tool_id=identity if isinstance(identity, str) else "")


def _target(arguments: dict) -> str:
    """What a tool works on, from its input: its file, else its pattern or query, else the first words of its command; "" when
    its input names none."""
    for key in ("file_path", "notebook_path", "pattern", "query", "url", "path"):
        if isinstance(arguments.get(key), str):
            return arguments[key]
    return _first_words(arguments.get("command"))


def _is_changed_file(item: dict) -> bool:
    changes = item.get("changes")
    return isinstance(changes, list) and bool(changes) and isinstance(changes[0], dict) and isinstance(changes[0].get("path"), str)


def _codex_target(item: dict) -> str:
    return item["changes"][0]["path"] if _is_changed_file(item) else _first_words(item.get("command"))


def _first_words(command: object) -> str:
    first_line = next((line for line in _command(command).splitlines() if line.strip()), "")
    return " ".join(first_line.split()[:TARGET_WORDS])


def _command(value: object) -> str:
    return value if isinstance(value, str) else ""


def _event_time(event: dict) -> str:
    """The UTC time (`text_limits.utc_text`) of an event's `timestamp`, an ISO text or epoch seconds or milliseconds;
    "" when it has none or one that is not a time."""
    stamp = event.get(TIME_KEY)
    try:
        if isinstance(stamp, str):
            parsed = datetime.fromisoformat(stamp)
            return text_limits.utc_text(parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc))
        if isinstance(stamp, (int, float)) and not isinstance(stamp, bool):
            seconds = stamp / MILLISECONDS_PER_SECOND if stamp >= MILLISECOND_EPOCH_FROM else stamp
            return text_limits.utc_text(datetime.fromtimestamp(seconds, timezone.utc))
    except (ValueError, OverflowError, OSError):
        return ""
    return ""


def _name(value: object) -> str:
    return value if isinstance(value, str) and value else UNKNOWN_TOOL


def _first_path(arguments: dict) -> str:
    return next((arguments[key] for key in PATH_KEYS if isinstance(arguments.get(key), str)), "")


def _line(value: object) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else None


def took_message(spec: RoleSpec, data: bytes) -> bool:
    """Whether the run whose log starts with `data` has read the message it was started with: a Claude run replays it as its
    first user line, a Codex run (which echoes nothing) has begun a turn or an item."""
    for _, _, event in lines(data, 0, False):
        if spec.provider == "goose":
            if event.get("type") == acp.ACCEPTED_EVENT:
                return True
        elif spec.provider == "codex":
            if event.get("type") == "turn.started" or isinstance(event.get("item"), dict):
                return True
        elif event.get("type") == "user" and event.get("parent_tool_use_id") is None and \
                live_input.replayed_text(event) is not None:
            return True
    return False


def login_failed(spec: RoleSpec, data: bytes) -> bool:
    """Whether a Claude run's log says the provider refused it for a missing login: its answer is the CLI's "Not logged in"
    notice, not the agent's (an assistant event marked `authentication_failed`, or an error result of that text)."""
    if spec.provider != "claude":
        return False
    for _, _, event in lines(data, 0, False):
        if event.get("type") == "assistant" and event.get("error") == "authentication_failed":
            return True
        if event.get("type") == "result" and event.get("is_error") is True and \
                str(event.get("result", "")).startswith("Not logged in"):
            return True
    return False


COMMIT_COMMAND = re.compile(r"\bgit\s+(?:-\S+\s+)*commit\b")
HOOK_ERROR = re.compile(r"^PreToolUse:\w+ hook error: (.*)$")
CHECK_PREFIX = re.compile(r"^sakura (?:pre-commit|[\w-]+): ")
MAX_REASON_CHARS = 200


def refused_commits(spec: RoleSpec, data: bytes) -> list[str]:
    """The reason of every `git commit` of a Claude run that was refused, oldest first: a commit command whose result is an
    error, or that the engine's own brake refused (`PreToolUse:Bash hook error: ...`), as the one line of the result that says
    why (the last that is not one of Git's `hint:` lines, without the engine's own `sakura pre-commit:` prefix). It is what
    the agent's tool printed: data for a person to read, bounded and cleaned, never an instruction."""
    if spec.provider != "claude":
        return []
    commits: set[str] = set()
    found: list[str] = []
    for _, _, event in lines(data, 0, False):
        for block in _content_blocks(event):
            if event.get("type") == "assistant" and block.get("type") == "tool_use" and block.get("name") == "Bash" \
                    and isinstance(block.get("id"), str) and COMMIT_COMMAND.search(str((block.get("input") or {}).get("command", ""))):
                commits.add(block["id"])
            elif event.get("type") == "user" and block.get("tool_use_id") in commits:
                reason = _refusal(_result_text(block))
                if reason:
                    found.append(reason)
    return found


def _content_blocks(event: dict) -> list[dict]:
    message = event.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    return [block for block in content if isinstance(block, dict)] if isinstance(content, list) else []


def _result_text(block: dict) -> str:
    content = block.get("content")
    if isinstance(content, list):
        return "\n".join(str(part.get("text", "")) for part in content if isinstance(part, dict))
    return content if isinstance(content, str) else ""


def _refusal(text: str) -> str:
    """The reason line of a failed commit's result, or "" for one that did not fail."""
    rows = [row.strip() for row in text.splitlines() if row.strip()]
    if rows and (hooked := HOOK_ERROR.match(rows[0])):
        return hooked[1][:MAX_REASON_CHARS]
    if not rows or not rows[0].startswith("Exit code"):
        return ""
    reasons = [row for row in rows[1:] if not row.startswith("hint:")]
    return CHECK_PREFIX.sub("", reasons[-1])[:MAX_REASON_CHARS] if reasons else ""
