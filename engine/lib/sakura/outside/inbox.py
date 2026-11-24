"""The person's answers to an outside lane, and their delivery to the agent that asked.

The answers (`mark`, `reply`, `steer`) are lines of the lane's log, appended under the lane's lock, so an answer
always names the exact content the person saw and a stale or repeated one is refused. A row is open while the log
holds no mark for its current digest, a question while it holds no reply for its current digest.

The agent reads its inbox, the person's lines numbered above its cursor. A read with `after` acknowledges up to that
number first; a read without it acknowledges nothing, so entries come again until a later read passes them. What a
read returned is recorded apart as delivered: the person is told what the agent has not been shown (`unread`), while
redelivery still starts after the acknowledged cursor, so an entry whose delivery was cut off is never lost.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, replace
from datetime import datetime
from pathlib import Path
from typing import Callable

from sakura import actors, questions
from sakura.outside import records, store
from sakura.outside.records import Lane, OwnerRow, Refused
from sakura.questions import Question

PERSON_KINDS = ("mark", "reply", "steer")
VERDICTS = ("ok", "ko")
PAGE_SIZE, POLL_SECONDS = 50, 1
MAX_TITLE = 200


@dataclass(frozen=True)
class Page:
    """One read of an inbox: the entries, the cursor to acknowledge them with, and what the lane still waits on."""

    entries: list[dict]
    cursor: int
    open_rows: int
    open_questions: int
    is_closed: bool


def mark(root: Path, by: actors.Actor, lane_id: str, row_id: str, digest: str, verdict: str, note: str,
         now: datetime, *, require_answerable: Callable[[], None] | None = None) -> dict:
    """Record the person's verdict on the row as it was when they saw it; return the log line."""
    require_person(by, "mark")
    if verdict not in VERDICTS:
        raise Refused(f"verdict must be one of {', '.join(VERDICTS)}")
    records.check_text("note", note, records.MAX_NOTE, required=False)
    with store.locked(root, lane_id):
        lane, log = _open_lane(root, lane_id)
        row = next((item for item in open_rows(lane, log) if item.id == row_id and item.digest == digest), None)
        if row is None:
            raise Refused(f"no longer waiting: row {row_id!r} was changed or already marked")
        if require_answerable is not None:
            require_answerable()
        fields = {"kind": "mark", "row": row_id, "digest": digest, "title": row.title, "verdict": verdict,
                  "note": note}
        return store.append_log(root, lane_id, by, lane.agent, now, fields)


def reply(root: Path, by: actors.Actor, lane_id: str, question_id: str, digest: str, choice: str, text: str,
          now: datetime, *, require_answerable: Callable[[], None] | None = None) -> dict:
    """Record the person's reply to the question as it was when they saw it; return the log line."""
    require_person(by, "reply")
    with store.locked(root, lane_id):
        lane, log = _open_lane(root, lane_id)
        asked = next((item for item in open_questions(lane, log) if item.id == question_id and item.digest == digest),
                     None)
        if asked is None:
            raise Refused(f"no longer waiting: question {question_id!r} was changed or already answered")
        problem = questions.reply_problem(asked, choice, text)
        if problem:
            raise Refused(f"reply: {problem}")
        if require_answerable is not None:
            require_answerable()
        fields = {"kind": "reply", "question": question_id, "digest": digest, "title": _title(asked.text),
                  "choice": choice, "text": text}
        return store.append_log(root, lane_id, by, lane.agent, now, fields)


def steer(root: Path, by: actors.Actor, lane_id: str, text: str, now: datetime) -> dict:
    """Record a message from the person to the lane's agent; return the log line."""
    require_person(by, "steer")
    records.check_text("message", text, questions.MAX_REPLY)
    with store.locked(root, lane_id):
        lane, _ = _open_lane(root, lane_id)
        return store.append_log(root, lane_id, by, lane.agent, now, {"kind": "steer", "text": text})


def read(root: Path, agent_name: str, lane_id: str, after: int | None) -> Page:
    """The inbox of `agent_name`'s lane; `after` first acknowledges up to that number (never past the log).

    What the page holds is recorded as delivered, which acknowledges nothing."""
    with store.locked(root, lane_id):
        lane = _owned(root, agent_name, lane_id)
        log = store.log(root, lane_id)
        if after is not None:
            lane = _acknowledged(root, lane, log, after)
        page = _page(lane, log)
        _record_delivery(root, lane, page)
        return page


def wait(root: Path, agent_name: str, lane_id: str, after: int | None, seconds: int, *,
         sleep: Callable[[float], None] = time.sleep, clock: Callable[[], float] = time.monotonic) -> Page:
    """Like `read`, but with no entry yet wait up to `seconds`, polling the log without holding the lane's lock."""
    deadline = clock() + seconds
    page = read(root, agent_name, lane_id, after)
    while not page.entries and not page.is_closed and clock() < deadline:
        sleep(POLL_SECONDS)
        page = read(root, agent_name, lane_id, after)
    return page


def open_rows(lane: Lane, log: list[dict]) -> list[OwnerRow]:
    """The lane's rows that wait for the person: none marked at their current content (none for a closed lane)."""
    answered = _answered(log, "mark", "row")
    return [] if lane.closed_at else [row for row in lane.rows if (row.id, row.digest) not in answered]


def open_questions(lane: Lane, log: list[dict]) -> list[Question]:
    """The lane's questions that wait for the person: none replied to at their current content."""
    answered = _answered(log, "reply", "question")
    return [] if lane.closed_at else [item for item in lane.questions if (item.id, item.digest) not in answered]


def done_rows(lane: Lane, log: list[dict]) -> int:
    """How many rows are marked Ok at their current content."""
    oks = {(line["row"], line["digest"]) for line in log if line.get("kind") == "mark" and line.get("verdict") == "ok"}
    return sum(1 for row in lane.rows if (row.id, row.digest) in oks)


def unread(lane: Lane, log: list[dict]) -> int:
    """How many of the person's lines no read has returned to the agent yet."""
    seen = max(lane.delivered_up_to, lane.read_up_to)
    return sum(1 for line in log if line["kind"] in PERSON_KINDS and line["seq"] > seen)


def require_person(by: actors.Actor, verb: str) -> None:
    """Raise Refused unless `by` stands for the person (the cockpit gate or the person's own command line)."""
    if by.source not in actors.PERSON_SOURCES:
        raise Refused(f"only the person answers; an actor of source {by.source!r} cannot {verb}")


def _open_lane(root: Path, lane_id: str) -> tuple[Lane, list[dict]]:
    lane = store.read(root, lane_id)
    if lane.closed_at is not None:
        raise Refused(f"lane {lane_id!r} is closed")
    return lane, store.log(root, lane_id)


def _owned(root: Path, agent_name: str, lane_id: str) -> Lane:
    lane = store.read(root, lane_id)
    if lane.agent.name != agent_name:
        raise Refused(f"lane {lane_id!r} belongs to {lane.agent.name!r}, not {agent_name!r}")
    return lane


def _acknowledged(root: Path, lane: Lane, log: list[dict], after: int) -> Lane:
    last = max((line["seq"] for line in log), default=0)
    if after < 0 or after > last:
        raise Refused(f"cursor {after} is past the end of the log ({last})")
    if after <= lane.read_up_to:
        return lane
    moved = replace(lane, read_up_to=after, delivered_up_to=max(lane.delivered_up_to, after))
    store.write_record(root, moved)
    return moved


def _record_delivery(root: Path, lane: Lane, page: Page) -> None:
    """Remember the highest entry a read returned; a read that returned nothing records nothing."""
    if page.entries and page.entries[-1]["seq"] > lane.delivered_up_to:
        store.write_record(root, replace(lane, delivered_up_to=page.entries[-1]["seq"]))


def _page(lane: Lane, log: list[dict]) -> Page:
    entries = [line for line in log if line["kind"] in PERSON_KINDS and line["seq"] > lane.read_up_to]
    shown = entries[:PAGE_SIZE]
    last = max((line["seq"] for line in log), default=0)
    cursor = shown[-1]["seq"] if len(entries) > PAGE_SIZE else max(last, lane.read_up_to)
    return Page(shown, cursor, len(open_rows(lane, log)), len(open_questions(lane, log)), lane.closed_at is not None)


def _answered(log: list[dict], kind: str, key: str) -> set[tuple[str, str]]:
    return {(line[key], line["digest"]) for line in log if line.get("kind") == kind}


def _title(text: str) -> str:
    first = next((line.strip() for line in text.splitlines() if line.strip()), "")
    return "".join(character for character in first if character.isprintable())[:MAX_TITLE]
