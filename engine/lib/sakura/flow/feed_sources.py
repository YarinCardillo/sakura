"""The two sources of a lane's feed that are not a role's run log: the lane's ledger lines and its project's board.

Both are read whole and returned in order; `flow/feed.py` pages them, cleans every text and marks them as data. A
ledger line is `- [tag] text` (lines of another shape, such as a ruling or the title, are not notices). A board post
shown is one posted on the lane's project at or after the lane started, by anyone (the lane itself, another lane, the
master, the person or the engine).
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home, text_limits
from sakura.flow import board, ledger_times, render, store

MAX_LEDGER_BYTES = 8 * 1024 * 1024
MAX_TITLE_CHARS = 120
NOTICE_LINE = re.compile(r"- \[([^\]]*)\] (.*)")


@dataclass(frozen=True)
class Notice:
    """One ledger line: its tag, its text and the time it was written ("" when none was recorded)."""

    tag: str
    text: str
    at: str


@dataclass(frozen=True)
class Post:
    """One board post with the task and title of the lane it came from ("" for the master, the person and the engine)."""

    seq: int
    at: str
    from_task: str
    from_title: str
    role: str
    text: str


def notices(task_store: store.TaskStore) -> list[Notice]:
    """Every notice of the task's ledger, in the order written; old lines without a time come first because they were
    written first."""
    try:
        with task_store.ledger.open("rb") as stream:
            data = stream.read(MAX_LEDGER_BYTES)
    except OSError:
        return []
    times, found, offset = ledger_times.read(task_store.ledger), [], 0
    for raw in data.split(b"\n"):
        match = NOTICE_LINE.fullmatch(raw.decode(errors="replace"))
        if match:
            found.append(Notice(match[1], match[2], times.get(offset, "")))
        offset += len(raw) + 1
    return found


def posts(task_store: store.TaskStore, since: int, started_at: str) -> list[Post]:
    """The project's board posts numbered above `since` and posted at or after `started_at`, oldest first; none when the
    board cannot be read."""
    try:
        found = board.read_from(engine_home.project_dir(task_store.person_repository), started_at, since)
    except (board.BoardRefused, OSError):
        return []
    titles: dict[str, str] = {}
    return [_post(task_store, titles, entry) for entry in found]


def _post(task_store: store.TaskStore, titles: dict[str, str], entry: board.Post) -> Post:
    author = entry.author
    if author.kind == "role" and author.task and author.task not in titles:
        titles[author.task] = _title(task_store.repo, author.task)
    role = author.role if author.kind == "role" else author.kind
    return Post(entry.seq, entry.at, author.task, titles.get(author.task, ""), role, entry.text)


def _title(repo: Path, task: str) -> str:
    """The first non-blank line of a task's input, cleaned and cut; "" when the task or its input is not there."""
    if not store.TASK_ID.fullmatch(task):
        return ""
    try:
        text = store.TaskStore(repo, task).input.read_text(errors="replace")[:4096]
    except OSError:
        return ""
    first = next((line.strip().lstrip("#").strip() for line in text.splitlines() if line.strip()), "")
    cleaned = text_limits.without_hidden_characters(render.strip_controls(first))
    return " ".join(cleaned.split())[:MAX_TITLE_CHARS]
