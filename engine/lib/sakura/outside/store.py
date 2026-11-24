"""The store of outside lanes: `<engine home>/outside/<lane>/lane.json` and `log.jsonl`.

The record is written only by the lane's agent verbs (`open_lane`, `put_row`, `ask`, `close`); the person's
answers live only in the log (see `inbox.py`). Folders are 0700 and files 0600. Every write takes the lane's lock,
a new lane also the store's lock; a record is replaced whole by a rename and a log line is appended whole, so a
crash leaves the old or the new state, never half. Reads never create anything.
"""

from __future__ import annotations

import copy
import fcntl
import json
import os
import shutil
import uuid
from contextlib import contextmanager
from dataclasses import dataclass, replace
from datetime import datetime, timedelta
from pathlib import Path
from typing import BinaryIO, Iterator, Sequence

from sakura import actors, appendlog, engine_home, file_cache, text_limits
from sakura.outside import records
from sakura.outside.records import Agent, Lane, Refused, StoreError
from sakura import questions
from sakura.questions import Question

STORE_FOLDER, RECORD_FILE, LOG_FILE, LANE_LOCK, STORE_LOCK = "outside", "lane.json", "log.jsonl", ".lane.lock", ".lock"
MAX_OPEN_LANES, MAX_ROWS, MAX_QUESTIONS = 50, 50, 20
# What one lane's log may hold. An agent can change one row or field any number of times, and every change is a line,
# so the agent's history is capped: past it a change is refused and the agent opens a new lane. The person's answers
# (a reply, a mark, a message) and the closing line have an allowance of their own, so an agent can never use up the room
# an answer needs: a reply is at most `questions.MAX_REPLY` characters of which each may serialize to twelve bytes (two
# `\u` escapes), and the lane can hold `MAX_QUESTIONS` questions and `MAX_ROWS` rows to answer.
MAX_LOG_ENTRIES, MAX_LOG_BYTES = 2000, 1_000_000
ESCAPED_CHARACTER_BYTES, LINE_OVERHEAD_BYTES = 12, 1000
WORST_ANSWER_BYTES = (questions.MAX_REPLY + records.MAX_TITLE) * ESCAPED_CHARACTER_BYTES + LINE_OVERHEAD_BYTES
MAX_ANSWER_ENTRIES = MAX_LOG_ENTRIES
MAX_ANSWER_BYTES = (MAX_QUESTIONS + MAX_ROWS) * WORST_ANSWER_BYTES
ANSWER_KINDS = ("mark", "reply", "steer", "closed")
# How much of a log a read parses: the two allowances together. A larger file is a broken history.
LOG_READ_BYTES = MAX_LOG_BYTES + MAX_ANSWER_BYTES
CLOSE_MARKER = ".closed"
RETENTION = timedelta(days=90)
DIRECTORY_MODE, FILE_MODE = 0o700, 0o600
OPENED, UPDATED, UNCHANGED, CLOSED = "opened", "updated", "unchanged", "closed"


@dataclass(frozen=True)
class Change:
    """What a write did: `result` is `opened`, `updated`, `unchanged` or `closed`; `lane` is the lane after it."""

    result: str
    lane: Lane
    is_new: bool


def path() -> Path:
    """The store folder under the engine home; not created."""
    return engine_home.root() / STORE_FOLDER


def read(root: Path, lane_id: str) -> Lane:
    """The lane `lane_id`. Raises Refused for a bad or unknown id and StoreError for an unreadable record."""
    records.check_id("lane id", lane_id)
    record = root / lane_id / RECORD_FILE
    if not record.exists():
        raise Refused(f"no outside lane {lane_id!r}")
    try:
        return records.lane_from_json(json.loads(record.read_text()))
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise StoreError(f"outside lane {lane_id!r} cannot be read: {error}") from error


def scan(root: Path, *, closed: bool = False) -> tuple[list[Lane], list[str]]:
    """The open lanes (the closed ones with `closed`), oldest first, and one line per record that cannot be read."""
    found, problems = [], []
    for folder in sorted(root.iterdir()) if root.is_dir() else []:
        if not (folder / RECORD_FILE).exists() or not records.ID_PATTERN.fullmatch(folder.name):
            continue  # not a lane, or one still being created
        if not closed and (folder / CLOSE_MARKER).exists():
            continue  # closed: the open scan never rereads it
        try:
            lane = read(root, folder.name)
            if not closed and lane.closed_at is None:
                require_whole_history(root, lane.id)
        except (StoreError, Refused) as error:
            problems.append(str(error))
            continue
        if (lane.closed_at is not None) == closed:
            found.append(lane)
    return sorted(found, key=lambda lane: (lane.opened_at, lane.id)), problems


History = tuple[Lane, list[dict]]


def open_histories(root: Path) -> tuple[list[History], list[str]]:
    """Read each open lane and its log once for display callers."""
    lanes, problems = scan(root)
    return [(lane, log(root, lane.id, for_display=True)) for lane in lanes], problems


def lanes(root: Path, *, closed: bool = False) -> list[Lane]:
    """The open lanes (the closed ones with `closed`); raises StoreError for an unreadable record."""
    found, problems = scan(root, closed=closed)
    if problems:
        raise StoreError(problems[0])
    return found


def log(root: Path, lane_id: str, *, for_display: bool = False) -> list[dict]:
    """The numbered log of a lane, oldest first. Display readers may reuse an unchanged file."""
    records.check_id("lane id", lane_id)
    require_whole_history(root, lane_id)
    path = root / lane_id / LOG_FILE
    if for_display:
        try:
            return copy.deepcopy(file_cache.load(path, _read_display_log))
        except FileNotFoundError:
            return []
    return appendlog.read(path, max_bytes=LOG_READ_BYTES)


def _read_display_log(stream: BinaryIO) -> list[dict]:
    return [entry for entry in appendlog.read_stream(stream, LOG_READ_BYTES) if entry["seq"] > 0]


def require_whole_history(root: Path, lane_id: str) -> None:
    """Raise StoreError unless the lane's log is small enough to be read whole. A read never shortens a history: one
    over `LOG_READ_BYTES` (grown before the caps) is a broken lane, shown as such and refused to the agent, because
    a tail left out would lose the person's answers and reopen the rows they answered."""
    try:
        size = (root / lane_id / LOG_FILE).stat().st_size
    except FileNotFoundError:
        return
    except OSError as error:
        raise StoreError(f"outside lane {lane_id!r} log cannot be read: {error}") from error
    if size > LOG_READ_BYTES:
        raise StoreError(f"outside lane {lane_id!r} is broken: its history is {size} bytes, more than the {LOG_READ_BYTES} "
                         "this app reads whole, so its rows and questions are not shown and nothing is answered "
                         "on it; close it with `sakura outside close` and open a new lane")


@contextmanager
def locked(root: Path, lane_id: str) -> Iterator[None]:
    """Hold the lane's lock; raises Refused when the lane does not exist."""
    records.check_id("lane id", lane_id)
    if not (root / lane_id).is_dir():
        raise Refused(f"no outside lane {lane_id!r}")
    with _flock(root / lane_id / LANE_LOCK):
        yield


def open_lane(root: Path, agent: Agent, lane_id: str, *, title: str, about: str = "",
              links: Sequence[tuple[str, str]] = (), phase: str = "draft", now: datetime) -> Change:
    """Open the lane, or update it when `agent` opened it already; the same fields answer `unchanged`."""
    records.check_id("lane id", lane_id)
    fields = _checked_lane_fields(title, about, links, phase)
    _make_private_folder(root)
    with _flock(root / STORE_LOCK):
        if not (root / lane_id / RECORD_FILE).exists():
            return _create(root, agent, lane_id, fields, now)
        with locked(root, lane_id):
            return _update(root, agent, lane_id, fields, now)


def put_row(root: Path, agent: Agent, lane_id: str, row_id: str, *, title: str, steps: Sequence[str], expected: str = "",
            note: str = "", now: datetime) -> Change:
    """Add the owner row, or replace its content (the row waits again, under a new revision); the same content
    answers `unchanged` and keeps its revision. The content is checked before anything is written."""
    content = {"title": title, "steps": steps, "expected": expected, "note": note}
    stamp = text_limits.utc_text(now)
    with locked(root, lane_id):
        lane = _writable(root, agent, lane_id)
        known = next((item for item in lane.rows if item.id == row_id), None)
        row = records.make_row(row_id, **content, now_text=stamp, revision=known.revision if known else 1)
        if known and known.digest == row.digest:
            return Change(UNCHANGED, lane, False)
        if known:
            row = records.make_row(row_id, **content, now_text=stamp, revision=known.revision + 1)
        if known is None and len(lane.rows) >= MAX_ROWS:
            raise Refused(f"lane {lane_id!r} has {MAX_ROWS} rows; the limit is {MAX_ROWS}")
        rows = tuple(row if item.id == row_id else item for item in lane.rows) if known else (*lane.rows, row)
        changed = _save(root, agent, replace(lane, rows=rows), now, {"kind": "row", "row": row_id,
                                                                     "digest": row.digest, "title": row.title})
        return Change(UPDATED if known else OPENED, changed, known is None)


def ask(root: Path, agent: Agent, lane_id: str, question: Question, now: datetime) -> Change:
    """Add the question, or replace its content; the same content answers `unchanged`."""
    with locked(root, lane_id):
        lane = _writable(root, agent, lane_id)
        known = next((item for item in lane.questions if item.id == question.id), None)
        if known and known.digest == question.digest:
            return Change(UNCHANGED, lane, False)
        if known and _was_answered(root, lane, known):
            raise Refused(f"question {known.id!r} was answered; ask it again under a new id, "
                          f"for example {_free_id(lane, known.id)!r}")
        if known is None and len(lane.questions) >= MAX_QUESTIONS:
            raise Refused(f"lane {lane_id!r} has {MAX_QUESTIONS} questions; the limit is {MAX_QUESTIONS}")
        kept = tuple(question if item.id == question.id else item for item in lane.questions)
        asked = kept if known else (*lane.questions, question)
        changed = _save(root, agent, replace(lane, questions=asked), now,
                        {"kind": "asked", "question": question.id, "digest": question.digest})
        return Change(UPDATED if known else OPENED, changed, known is None)


def close(root: Path, by: actors.Actor, lane_id: str, *, outcome: str, now: datetime) -> Change:
    """Close the lane: its agent with `done` or `dropped`, the person with `dropped`; a closed lane is `unchanged`."""
    _check_closer(by, outcome)
    with locked(root, lane_id):
        lane = read(root, lane_id)
        if by.source == actors.OUTSIDE_SOURCE and by.login != lane.agent.name:
            raise Refused(f"lane {lane_id!r} belongs to {lane.agent.name!r}, not {by.login!r}")
        if lane.closed_at is not None:
            return Change(UNCHANGED, lane, False)
        stamp = text_limits.utc_text(now)
        done = replace(lane, closed_at=stamp, outcome=outcome, updated_at=stamp)
        write_record(root, done)
        if _has_whole_history(root, lane_id):  # a broken history is closed without a line: it cannot be read to add one
            append_log(root, lane_id, by, lane.agent, now, {"kind": "closed", "outcome": outcome})
        _mark_closed(root, lane_id)
        return Change(CLOSED, done, False)


def note_pushed(root: Path, lane_id: str, now: datetime) -> None:
    """Record that a push about this lane was sent at `now`."""
    with locked(root, lane_id):
        write_record(root, replace(read(root, lane_id), pushed_at=text_limits.utc_text(now)))


def append_log(root: Path, lane_id: str, by: actors.Actor, agent: Agent, now: datetime, fields: dict) -> dict:
    """Append a line to the lane's log, written by `by` (the caller holds the lane's lock); return the line."""
    line = {**fields, "at": text_limits.utc_text(now), "by": actors.format(by)}
    if by.source == actors.OUTSIDE_SOURCE:
        line["where"] = str(agent.where)
    _require_room(root, lane_id, line)
    try:
        number = appendlog.append(root / lane_id / LOG_FILE, line, max_entries=MAX_LOG_ENTRIES + MAX_ANSWER_ENTRIES,
                                  max_bytes=LOG_READ_BYTES)
    except appendlog.LogFull as error:
        raise Refused(f"lane {lane_id!r} history is full ({error}): close the lane and open a new one") from error
    except OSError as error:
        raise StoreError(f"outside lane {lane_id!r} log cannot be written: {error}") from error
    return {"seq": number, **line}


def _require_room(root: Path, lane_id: str, line: dict) -> None:
    """Refuse a line that would pass the allowance of its own kind: the agent's history or the person's answers, each
    counted apart (the caller holds the lane's lock)."""
    is_answer = line.get("kind") in ANSWER_KINDS
    entries, size = 0, 0
    for entry in log(root, lane_id):
        if (entry.get("kind") in ANSWER_KINDS) == is_answer:
            entries, size = entries + 1, size + _serialized_bytes(entry)
    cap_entries, cap_bytes = (MAX_ANSWER_ENTRIES, MAX_ANSWER_BYTES) if is_answer else (MAX_LOG_ENTRIES, MAX_LOG_BYTES)
    if entries + 1 > cap_entries or size + _serialized_bytes({"seq": 0, **line}) > cap_bytes:
        if is_answer:
            raise Refused(f"lane {lane_id!r} has no room left for answers ({entries} entries, {size} bytes): close the "
                          "lane and open a new one")
        raise Refused(f"lane {lane_id!r} history is full ({entries} entries, {size} bytes; the limits are "
                      f"{cap_entries} and {cap_bytes}): close the lane and open a new one")


def _serialized_bytes(entry: dict) -> int:
    """What a line of the log takes on disk (`appendlog.append` writes it compact, with a newline)."""
    return len(json.dumps(entry, separators=(",", ":")).encode()) + 1


def write_record(root: Path, lane: Lane) -> None:
    """Replace the lane's record whole by a rename (the caller holds the lane's lock)."""
    target = root / lane.id / RECORD_FILE
    temporary = target.with_name(f".{RECORD_FILE}.{uuid.uuid4().hex}.tmp")
    try:
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, FILE_MODE)
        with os.fdopen(descriptor, "w") as stream:
            json.dump(records.lane_to_json(lane), stream, indent=1)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    except OSError as error:
        raise StoreError(f"outside lane {lane.id!r} record cannot be written: {error}") from error


def _checked_lane_fields(title: str, about: str, links: Sequence[tuple[str, str]], phase: str) -> dict:
    records.check_line("title", title, records.MAX_TITLE)
    records.check_text("about", about, records.MAX_ABOUT, required=False)
    if phase not in records.PHASES:
        raise Refused(f"phase must be one of {', '.join(records.PHASES)}")
    return {"title": title, "about": about, "links": records.make_links(links), "phase": phase}


def _create(root: Path, agent: Agent, lane_id: str, fields: dict, now: datetime) -> Change:
    _remove_expired(root, now)
    if len(scan(root)[0]) >= MAX_OPEN_LANES:
        raise Refused(f"{MAX_OPEN_LANES} outside lanes are open; the limit is {MAX_OPEN_LANES}. "
                      f"Close one with `sakura outside close`")
    _make_private_folder(root / lane_id)
    stamp = text_limits.utc_text(now)
    lane = Lane(lane_id, agent, fields["title"], fields["about"], fields["links"], fields["phase"], (), (), stamp,
                stamp, None, "", 0, None)
    with locked(root, lane_id):
        write_record(root, lane)
        append_log(root, lane_id, _author(agent), agent, now, {"kind": "opened", "title": lane.title})
    return Change(OPENED, lane, True)


def _update(root: Path, agent: Agent, lane_id: str, fields: dict, now: datetime) -> Change:
    lane = _writable(root, agent, lane_id)
    if all(getattr(lane, name) == value for name, value in fields.items()):
        return Change(UNCHANGED, lane, False)
    names = [name for name, value in fields.items() if getattr(lane, name) != value]
    changed = _save(root, agent, replace(lane, **fields), now, {"kind": "changed", "fields": names})
    return Change(UPDATED, changed, False)


def _writable(root: Path, agent: Agent, lane_id: str) -> Lane:
    lane = read(root, lane_id)
    try:
        require_whole_history(root, lane_id)
    except StoreError as broken:
        raise Refused(str(broken)) from broken
    if lane.agent.name != agent.name:
        raise Refused(f"lane {lane_id!r} belongs to {lane.agent.name!r}, not {agent.name!r}")
    if lane.closed_at is not None:
        raise Refused(f"lane {lane_id!r} is closed; open a new lane with another id")
    return lane


def _save(root: Path, agent: Agent, lane: Lane, now: datetime, fields: dict) -> Lane:
    saved = replace(lane, updated_at=text_limits.utc_text(now))
    # The log line first: a history that is full refuses the change before the record holds it.
    append_log(root, lane.id, _author(agent), agent, now, fields)
    write_record(root, saved)
    return saved


def _check_closer(by: actors.Actor, outcome: str) -> None:
    if by.source == actors.OUTSIDE_SOURCE:
        if outcome not in records.OUTCOMES:
            raise Refused(f"outcome must be one of {', '.join(records.OUTCOMES)}")
    elif by.source in actors.PERSON_SOURCES:
        if outcome != "dropped":
            raise Refused("the person closes a lane as dropped")
    else:
        raise Refused(f"an actor of source {by.source!r} cannot close an outside lane")


def _has_whole_history(root: Path, lane_id: str) -> bool:
    try:
        require_whole_history(root, lane_id)
    except StoreError:
        return False
    return True


def _mark_closed(root: Path, lane_id: str) -> None:
    """Leave a marker beside the record of a closed lane, so a scan for open lanes skips it without reading it."""
    try:
        os.close(os.open(root / lane_id / CLOSE_MARKER, os.O_WRONLY | os.O_CREAT, FILE_MODE))
    except OSError as error:
        raise StoreError(f"outside lane {lane_id!r} cannot be marked closed: {error}") from error


def _was_answered(root: Path, lane: Lane, question: Question) -> bool:
    return any(line.get("kind") == "reply" and line.get("question") == question.id
               and line.get("digest") == question.digest for line in log(root, lane.id))


def _free_id(lane: Lane, question_id: str) -> str:
    taken = {item.id for item in lane.questions}
    number = 2
    while f"{question_id[:36]}-{number}" in taken:
        number += 1
    return f"{question_id[:36]}-{number}"


def _author(agent: Agent) -> actors.Actor:
    return actors.Actor(actors.OUTSIDE_SOURCE, agent.name)


def _remove_expired(root: Path, now: datetime) -> None:
    for lane in scan(root, closed=True)[0]:
        if datetime.fromisoformat(lane.closed_at) + RETENTION < now:
            with locked(root, lane.id):
                shutil.rmtree(root / lane.id)


def _make_private_folder(folder: Path) -> None:
    try:
        folder.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
        os.chmod(folder, DIRECTORY_MODE)
    except OSError as error:
        raise StoreError(f"{folder} cannot be created: {error}") from error


@contextmanager
def _flock(lock: Path) -> Iterator[None]:
    try:
        descriptor = os.open(lock, os.O_RDWR | os.O_CREAT, FILE_MODE)
    except OSError as error:
        raise StoreError(f"{lock} cannot be locked: {error}") from error
    with os.fdopen(descriptor, "r+") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        yield
