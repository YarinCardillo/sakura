"""Live input: a message to a Claude role that is running now, in the same session, through the engine's hold on its stdin.

A Claude role starts with `--input-format stream-json --replay-user-messages` (plan task S4.3, measured: a message sent
during a turn is taken at the next tool boundary, in the same session, and closing stdin after the last `result` ends
the run with nothing left). The engine writes the prompt as the first user message and keeps stdin open. While the run
lives, a steer (`flow/inbox.py`, `take_live`) is written there as another user message and the run is `delivered`; a
steer that finds the channel closed stays in the inbox and reaches the next role (`queued`).

The channel closes stdin once every message it wrote was replayed back (the CLI re-emits each user message it takes up)
and a `result` followed the last of them, so a message written a moment too late is never half taken: it was not
written. The supervising thread never writes the pipe: one writer thread per run does, fed by a queue, so a role that stops
reading cannot hold up the run's time limits or `finish`. `send` waits at most `WRITE_BOUND_SECONDS` for its message to
be written; past that the message is refused (the caller queues it for the next role) and the channel is closed.

A message is registered for its replay before it reaches the pipe, so a replay that comes at once finds it. A write that
times out may still complete when the role reads again: that message is `UNCERTAIN`, the channel closes, and when the run
ends the replay decides (`uncertain_taken`: it was delivered; `unconsumed`: it was not, queue it), never both.

A message written and never replayed (the run ended or was stopped first, or the replay never came within
`ACK_GRACE_SECONDS` of a result) is handed back with `unconsumed`, so the engine queues it for the next role: never
lost, and written to at most one live run.
"""

from __future__ import annotations

import json
import queue
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import BinaryIO

ACK_GRACE_SECONDS = 20
# How long `send` waits for the writer thread to put a message into the pipe; past it the message is `queued`.
WRITE_BOUND_SECONDS = 2.0
READ_CHUNK = 1 << 20
RESULT, USER = "result", "user"
# What `send` answers: the message is in the pipe; it was never written (queue it); or its write timed out and may still
# complete, so the channel decides when the run ends (`unconsumed`, `uncertain_taken`) and the caller queues nothing.
SENT, REFUSED, UNCERTAIN = "sent", "refused", "uncertain"


@dataclass
class _Written:
    """One message for the role: the command it came from (empty for the prompt), its text and how far it got
    (`pending`, `writing`, `written`, `refused`: never written, or abandoned)."""

    command: str
    text: str
    entry: dict | None = None
    state: str = "pending"
    done: threading.Event = field(default_factory=threading.Event)


def message_line(text: str) -> bytes:
    """One stream-json user message as a line of stdin."""
    return (json.dumps({"type": USER, "message": {"role": USER, "content": text}}) + "\n").encode()


class LiveChannel:
    """The engine's hold on one role's stdin and its view of what the role echoed back; thread safe."""

    def __init__(self, clock: Callable[[], float] = time.monotonic, *,
                 wait_for_write: Callable[[threading.Event, float], object] = threading.Event.wait) -> None:
        self._lock = threading.Lock()
        self._clock = clock
        self._wait_for_write = wait_for_write
        self._stdin: BinaryIO | None = None
        self._waiting: list[_Written] = []
        self._result_since_take_up = False
        self._closed = False
        self._offset = 0
        self._pending = b""
        self._result_at: float | None = None
        self._outbox: queue.Queue[_Written | None] = queue.Queue()
        self._writer: threading.Thread | None = None
        self._sentinel_sent = False
        self._uncertain_taken: list[str] = []
        self._failed: list[_Written] = []

    @property
    def is_open(self) -> bool:
        """Whether a message can still be written."""
        with self._lock:
            return self._stdin is not None and not self._closed

    def attach(self, stdin: BinaryIO, prompt: str) -> None:
        """Take over the role's stdin and queue the prompt as its first message; returns at once."""
        with self._lock:
            self._stdin = stdin
            self._writer = threading.Thread(target=self._write_loop, daemon=True)
            self._writer.start()
            self._enqueue(_Written("", prompt))

    def send(self, command: str, text: str, entry: dict | None = None) -> str:
        """Have `text` written as a user message. `SENT`: it is in the pipe. `REFUSED`: it was never written (the
        channel is closed, the role gone, or the writer had not begun within `WRITE_BOUND_SECONDS`): queue it. `UNCERTAIN`:
        its write began and did not finish in time; it may still complete, the channel closes and settles it later."""
        with self._lock:
            if self._stdin is None or self._closed:
                return REFUSED
            item = _Written(command, text, entry)
            self._enqueue(item)
        self._wait_for_write(item.done, WRITE_BOUND_SECONDS)
        with self._lock:
            if item.state == "written":
                return SENT
            if item.state == "writing":
                item.state = "uncertain"
                self._close()
                return UNCERTAIN
            if item.state == "pending":
                item.state = "refused"
                self._close()
            self._forget(item)
            return REFUSED

    def observe(self, log_path: Path) -> None:
        """Read what the role logged since the last call: replayed messages and results; close stdin when all is taken."""
        with self._lock:
            if self._closed and not self._waiting:
                return
            for event in self._new_events(log_path):
                self._take(event)
            self._close_if_done()

    def finish(self) -> None:
        """Close stdin for good (the run ends or is stopped): no later message is written. Never waits for the writer."""
        with self._lock:
            self._close()

    def uncertain_taken(self) -> list[str]:
        """The commands whose write timed out (`UNCERTAIN`) and that the role replayed after all: they were delivered."""
        with self._lock:
            return list(self._uncertain_taken)

    def unconsumed(self) -> list[tuple[str, dict]]:
        """The (command id, entry) of every message from a command that was written and never replayed; the entry is
        what `send` was given (the command's actor, kind and own text), or just its id and written text."""
        with self._lock:
            return [(item.command, item.entry or {"id": item.command, "text": item.text})
                    for item in [*self._waiting, *self._failed] if item.command]

    # -- private

    def _write_loop(self) -> None:
        """The one thread that writes the role's stdin; the pipe may block here and nowhere else."""
        while (item := self._outbox.get()) is not None:
            with self._lock:
                if item.state != "pending":
                    continue
                item.state = "writing"
            ok = self._write_line(item.text)
            with self._lock:
                if ok and item.state == "writing":
                    item.state = "written"
                elif not ok:
                    self._forget(item)
                    self._close()
            item.done.set()
        self._close_stdin()

    def _enqueue(self, item: _Written) -> None:
        """Register the message for its replay, then hand it to the writer (the caller holds the lock): the pipe never
        sees a message the channel does not already know."""
        self._waiting.append(item)
        self._outbox.put(item)

    def _forget(self, item: _Written) -> None:
        """The write failed (the role is gone): the message never arrived. One whose delivery had been left uncertain was
        applied by its caller, so it is kept for `unconsumed` to hand back."""
        if item in self._waiting:
            self._waiting.remove(item)
        if item.state == "uncertain":
            self._failed.append(item)
        item.state = "failed"

    def _write_line(self, text: str) -> bool:
        try:
            self._stdin.write(message_line(text))
            self._stdin.flush()
        except (BrokenPipeError, ValueError, OSError):
            return False
        return True

    def _close_stdin(self) -> None:
        try:
            self._stdin.close()
        except (BrokenPipeError, ValueError, OSError):
            pass

    def _new_events(self, log_path: Path) -> list[dict]:
        try:
            with log_path.open("rb") as log:
                log.seek(self._offset)
                data = self._pending + log.read(READ_CHUNK)
                self._offset = log.tell()
        except OSError:
            return []
        *lines, self._pending = data.split(b"\n")
        return [event for event in map(_parsed, lines) if event]

    def _take(self, event: dict) -> None:
        if event.get("type") == RESULT:
            self._result_since_take_up = True
            self._result_at = self._clock()
        elif event.get("type") == USER and (text := replayed_text(event)) is not None:
            for index, item in enumerate(self._waiting):
                if item.text == text:
                    del self._waiting[index]
                    if item.state == "uncertain":
                        self._uncertain_taken.append(item.command)
                    self._result_since_take_up = False
                    break

    def _close_if_done(self) -> None:
        if self._closed or not self._result_since_take_up:
            return
        graced = self._result_at is not None and self._clock() - self._result_at >= ACK_GRACE_SECONDS
        if not self._waiting or graced:
            self._close()

    def _close(self) -> None:
        """Mark the channel closed (the caller holds the lock) and tell the writer to close stdin after its queue."""
        if not self._sentinel_sent and self._stdin is not None:
            self._sentinel_sent = True
            self._outbox.put(None)
        self._closed = True


def replayed_texts(log_path: Path) -> list[str]:
    """The text of every user message the role replayed in its log (tool results are not messages); an unreadable or
    missing log has none."""
    try:
        lines = log_path.read_bytes().split(b"\n")
    except OSError:
        return []
    return [text for event in map(_parsed, lines) if event.get("type") == USER and (text := replayed_text(event)) is not None]


def _parsed(line: bytes) -> dict:
    try:
        event = json.loads(line)
    except ValueError:
        return {}
    return event if isinstance(event, dict) else {}


def replayed_text(event: dict) -> str | None:
    """The text of a user event that carries plain text (a replayed message), None for one that carries a tool result."""
    content = (event.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list) and content and all(isinstance(block, dict) and block.get("type") == "text" for block in content):
        return "".join(str(block.get("text", "")) for block in content)
    return None
