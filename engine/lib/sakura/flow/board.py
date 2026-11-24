"""The board: attributed posts of one project's roles, the master, the person and the engine.

One numbered append-only log per project, `<project folder>/board.jsonl` (`appendlog.py`: two writers at once lose
no post). The author of a post is never what the writer says: a role's tools take it from the policy the engine
wrote for that run, the engine posts as itself. Text is bounded in characters and in UTF-8 bytes, and refused, not cleaned, when it holds a control,
bidi or zero-width character.

The board is bounded as an outside lane's log is (`outside/store.py`): what agents (roles and the master) post is capped
and a post past the cap is refused; the person's and the engine's posts have an allowance of their own, so agents never
use up their room. A read parses the board whole or not at all: a board over both allowances (grown before them) is
broken, shown as such and refused, never read shortened.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from sakura import appendlog, text_limits

BOARD_FILE = "board.jsonl"
MAX_TEXT = 2000
KINDS = ("role", "master", "person", "engine")
AGENT_KINDS = ("role", "master")
AGENT_ALLOWANCE = appendlog.Allowance(2000, 1_000_000)
OWN_ALLOWANCE = appendlog.Allowance(2000, 1_000_000)  # the person's and the engine's posts
READ_BYTES = AGENT_ALLOWANCE.bytes + OWN_ALLOWANCE.bytes
IDENTIFIER = re.compile(r"[A-Za-z0-9._-]{0,64}")
FOLDER_MODE = 0o700


class BoardRefused(ValueError):
    """Raised when a post is refused (its text or its author is not acceptable, or its kind's room is used up) and
    when the board cannot be read whole."""


@dataclass(frozen=True)
class Author:
    """Who wrote a post: a `kind` and, for a role, the task, role and run the engine knows it by."""

    kind: str
    task: str = ""
    role: str = ""
    run: str = ""


@dataclass(frozen=True)
class Post:
    """One post: its number on the board, when it was posted, its author and its text."""

    seq: int
    at: str
    author: Author
    text: str


def post(project_root: Path, author: Author, text: str, now: datetime | None = None) -> Post:
    """Append a post to the board of the project whose folder is `project_root`.

    Raises BoardRefused for a blank text, one over `MAX_TEXT` characters or with a control, bidi or zero-width
    character, for an author that is not a known kind with plain identifiers, when the posts of the author's kind fill
    their allowance and when the board is broken.
    """
    _require_author(author)
    problem = text_limits.text_problem(text, MAX_TEXT) or text_limits.hidden_character_problem(text)
    if not problem and len(text.encode()) > MAX_TEXT:
        problem = f"{len(text.encode())} bytes; the limit is {MAX_TEXT}"
    if problem:
        raise BoardRefused(problem)
    project_root.mkdir(parents=True, exist_ok=True, mode=FOLDER_MODE)
    at = text_limits.utc_text(now or datetime.now(timezone.utc))
    record = {"at": at, "kind": author.kind, "task": author.task, "role": author.role, "run": author.run, "text": text}
    path = project_root / BOARD_FILE
    is_agent = author.kind in AGENT_KINDS
    allowance = AGENT_ALLOWANCE if is_agent else OWN_ALLOWANCE

    def room() -> None:
        appendlog.require_room(path, record, lambda entry: (entry.get("kind") in AGENT_KINDS) == is_agent, allowance,
                               READ_BYTES)

    try:
        seq = appendlog.append(path, record, max_entries=AGENT_ALLOWANCE.entries + OWN_ALLOWANCE.entries,
                               max_bytes=READ_BYTES, room=room)
    except appendlog.LogBroken as error:
        raise BoardRefused(_broken(error)) from None
    except appendlog.LogFull as error:
        raise BoardRefused(f"the board of this project is full for posts of this kind ({error})") from None
    return Post(seq, at, author, text)


def read(project_root: Path, since: int = 0) -> list[Post]:
    """The posts numbered above `since`, oldest first; an entry that is not a whole post is skipped. Raises
    BoardRefused for a board that cannot be read whole."""
    try:
        entries = appendlog.read_whole(project_root / BOARD_FILE, READ_BYTES, after=since)
    except appendlog.LogBroken as error:
        raise BoardRefused(_broken(error)) from None
    posts = []
    for entry in entries:
        try:
            author = Author(entry["kind"], entry["task"], entry["role"], entry["run"])
            posts.append(Post(entry["seq"], entry["at"], author, entry["text"]))
        except KeyError:
            continue
    return posts


def read_from(project_root: Path, started_at: str, since: int = 0) -> list[Post]:
    """The posts numbered above `since` that were posted at or after `started_at` (a UTC ISO time; "" or one that
    cannot be read means every post), oldest first. Raises BoardRefused for a board that cannot be read whole."""
    start = _instant(started_at)
    return [found for found in read(project_root, since) if start is None or (_instant(found.at) or start) >= start]


def _instant(text: str) -> datetime | None:
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def _broken(error: appendlog.LogBroken) -> str:
    return f"the board of this project is broken ({error}): no post of it is shown and none is taken"


def _require_author(author: Author) -> None:
    fields = (author.task, author.role, author.run)
    if author.kind not in KINDS or not all(IDENTIFIER.fullmatch(field) for field in fields):
        raise BoardRefused(f"refused author {author.kind!r}: a kind of {', '.join(KINDS)} and plain identifiers")
