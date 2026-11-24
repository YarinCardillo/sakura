"""Questions a role of an engine lane asks the person, kept in `<task folder>/questions.jsonl`.

A role asks through its `ask_person` tool and is not blocked: the question is recorded with an id the engine makes
(`q` and eight hex digits) and the lane goes on. It appears as a question wait (`flow/waits.py`); the person's reply,
once, is recorded here and handed to the lane as a steer that starts with "The person answered question". The steer
names the question only by its id and the asking role, both the engine's: the question's text was written by a role
and never goes into a message that carries the person's authority. A reply names the digest of the question it answers, so it never applies to a question that changed or was answered
meanwhile. Question text and choices have the limits of `sakura/questions.py`.

The log is a numbered append-only file (`sakura/appendlog.py`), not the task state: a role's tool runs in another
process than the engine, and the engine refuses a state file another process wrote. It is bounded as an outside lane's
log is (`outside/store.py`): the roles' questions are capped and a question past the cap is refused; the person's
replies have an allowance of their own, room for a worst-case reply (`questions.MAX_REPLY` characters at twelve bytes
each, two `\\u` escapes) to every question that can wait at once, so the roles never use up the room a reply needs. A
read parses the log whole or not at all: a log over both allowances (grown before them) is refused, never read
shortened, as a tail left out would reopen the questions the person answered.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import uuid
from collections.abc import Callable, Iterator
from datetime import datetime
from pathlib import Path

from sakura import appendlog, questions, text_limits
from sakura.flow.store import TaskStore

FILE = "questions.jsonl"
MAX_OPEN = 20
ANSWER_LINE = "The person answered question "
ID_HEX_DIGITS = 8
REPLY_LOCK = "questions.reply.lock"
LOCK_MODE = 0o600
ASKED, REPLIED = "asked", "replied"
ESCAPED_CHARACTER_BYTES, LINE_OVERHEAD_BYTES = 12, 1000
ASKED_ALLOWANCE = appendlog.Allowance(2000, 1_000_000)
WORST_REPLY_BYTES = questions.MAX_REPLY * ESCAPED_CHARACTER_BYTES + LINE_OVERHEAD_BYTES
REPLY_ALLOWANCE = appendlog.Allowance(ASKED_ALLOWANCE.entries, MAX_OPEN * WORST_REPLY_BYTES)
READ_BYTES = ASKED_ALLOWANCE.bytes + REPLY_ALLOWANCE.bytes


class ReplyRefused(ValueError):
    """Raised, with a one-line reason, when a reply does not apply: not waiting any more, or not an acceptable answer."""


class QuestionsBroken(ValueError):
    """Raised when the lane's question log is larger than a read parses whole; it is never read shortened."""


def ask(task_store: TaskStore, role: str, run: str, text: str, choices: list[str], now: datetime) -> questions.Question:
    """Record a question asked by `role` in `run` and return it. Raises questions.QuestionRefused for a text or
    choices that break a limit, when `MAX_OPEN` questions are already open, when the roles' questions fill their
    allowance and when the log cannot be read whole."""
    try:
        waiting = open_questions(task_store)
    except QuestionsBroken as error:
        raise questions.QuestionRefused(str(error)) from None
    if len(waiting) >= MAX_OPEN:
        raise questions.QuestionRefused(f"{MAX_OPEN} questions are already waiting for the person")
    question = questions.make(f"q{uuid.uuid4().hex[:ID_HEX_DIGITS]}", text, choices, now)
    record = {"kind": ASKED, "id": question.id, "text": question.text, "choices": list(question.choices),
              "digest": question.digest, "asked_at": question.asked_at, "role": role, "run": run}
    try:
        _append(task_store, record, room=lambda: _require_room(task_store, record))
    except (appendlog.LogFull, appendlog.LogBroken) as error:
        raise questions.QuestionRefused(f"this lane's questions are full ({error}): no more questions in it") from None
    task_store.append_ledger("questions", f"question {question.id} asked by {role}")
    return question


def open_questions(task_store: TaskStore) -> list[dict]:
    """The recorded questions the person has not answered, oldest first (`kind` `asked` entries). Raises
    QuestionsBroken for a log that cannot be read whole."""
    try:
        entries = appendlog.read_whole(task_store.root / FILE, READ_BYTES)
    except appendlog.LogBroken as error:
        raise QuestionsBroken(f"this lane's questions cannot be read whole ({error}): none is shown or answered") from None
    answered = {entry.get("id") for entry in entries if entry.get("kind") == REPLIED}
    return [entry for entry in entries if entry.get("kind") == ASKED and entry.get("id") not in answered]


def answer(task_store: TaskStore, question_id: str, digest: str, choice: str, text: str, by: str, now: datetime,
           deliver: Callable[[str], None]) -> None:
    """Record the person's reply to `question_id` as it was when they saw it (`digest`) and hand the lane its steer
    line through `deliver`, which runs before the reply is recorded: a delivery that fails leaves the question
    waiting. Raises ReplyRefused: no longer waiting (unknown, changed or already answered), not an acceptable answer
    (one of the choices, or text, not both), no room left for replies, or a log that cannot be read whole; the lane
    is told nothing then."""
    with _replying(task_store):
        try:
            waiting = open_questions(task_store)
        except QuestionsBroken as error:
            raise ReplyRefused(str(error)) from None
        entry = next((found for found in waiting if found["id"] == question_id), None)
        if entry is None or entry["digest"] != digest:
            raise ReplyRefused(f"no longer waiting: question {question_id[:20]!r} was answered, changed or never asked")
        asked = questions.Question(entry["id"], entry["text"], tuple(entry["choices"]), entry["digest"], entry["asked_at"])
        problem = questions.reply_problem(asked, choice, text)
        if problem:
            raise ReplyRefused(problem)
        record = {"kind": REPLIED, "id": question_id, "digest": digest, "choice": choice, "text": text, "by": by,
                  "at": text_limits.utc_text(now)}
        try:
            _require_room(task_store, record)
        except (appendlog.LogFull, appendlog.LogBroken) as error:
            raise ReplyRefused(f"no room left for replies in this lane ({error})") from None
        deliver(_steer_line(entry, choice or text))
        _append(task_store, record)


def _require_room(task_store: TaskStore, record: dict) -> None:
    """Raise appendlog.LogFull when `record` would pass the allowance of its kind: the roles' questions or the
    person's replies, each counted apart."""
    is_reply = record["kind"] == REPLIED
    appendlog.require_room(task_store.root / FILE, record, lambda entry: (entry.get("kind") == REPLIED) == is_reply,
                           REPLY_ALLOWANCE if is_reply else ASKED_ALLOWANCE, READ_BYTES)


def _append(task_store: TaskStore, record: dict, room: Callable[[], None] | None = None) -> None:
    appendlog.append(task_store.root / FILE, record, max_entries=ASKED_ALLOWANCE.entries + REPLY_ALLOWANCE.entries,
                     max_bytes=READ_BYTES, room=room)


@contextlib.contextmanager
def _replying(task_store: TaskStore) -> Iterator[None]:
    """One reply at a time per lane, so the check that a question waits and the recording of its reply are one step."""
    descriptor = os.open(Path(task_store.root) / REPLY_LOCK, os.O_RDWR | os.O_CREAT, LOCK_MODE)
    with os.fdopen(descriptor, "r+") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        yield


def _steer_line(entry: dict, given: str) -> str:
    """The person's answer, named by the question's id and the asking role (the engine's own words); the answer is
    the only free text."""
    return f"{ANSWER_LINE}{entry['id']} (asked by {entry['role']}): {given}"
