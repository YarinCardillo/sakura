"""The asks the person has decided, kept with the answer they gave (`<state dir>/cockpit/decided.jsonl`).

Inbox > Done reads this log and the terminal proposal records. Logged answer paths include the cockpit's own actions (`actions.run`) and the operator commands a phone page or a script
reaches (`sakura flow approve|mark|reply|stop|resume`, `sakura proposal accept|reject`). A path that records an answer
another path already recorded is de-duplicated, so one answer is one line however many paths saw it.

The answer text is what the person typed and the title is what an agent wrote: both are stored with terminal controls
removed and shown as data, never run and never given to the master. The log is append-only and numbered
(`appendlog.py`); a line that does not parse is skipped. Recording is best effort: the answer has already happened, so a
log that cannot be written is reported on stderr and never undoes or blocks it.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Iterable
from datetime import datetime, timedelta
from pathlib import Path

from sakura import actors, appendlog, engine_home, text_limits
from sakura.flow import ask_words, store, waits
from sakura.flow.render import strip_controls
from sakura.scheduled.inbox import APPROVAL as SCHEDULED_APPROVAL

FILE = Path("cockpit") / "decided.jsonl"
ANSWERS = ("approve", "decline", "reply", "choice", "resume", "stop", "accept", "reject", "mark_ok", "mark_ko", "reviewed", "merged", "closed", "started", "dismissed", "close")
MAX_TEXT = 4000
RETENTION_DAYS = 7
MAX_LISTED = 200
# The same answer to the same ask seen by two paths (a detached approval records a moment after the cockpit's own
# action returns) is one line when both fall within this many seconds.
DUPLICATE_SECONDS = 60
# How much of the end of the log a read or a duplicate check parses: a week of answers is far below it.
READ_TAIL_BYTES = 1024 * 1024
DUPLICATE_TAIL_BYTES = 64 * 1024
DIRECTORY_MODE = 0o700
PUBLIC_FIELDS = ("id", "key", "kind", "project", "task", "lane_title", "title", "answer", "choice", "text", "actor", "place", "at", "lane")
WAIT_KEY_LENGTH, NAME_POSITION = 5, 4
ANY_SUBJECT = None
PLACES = {actors.APP_SOURCE: "desktop", "overlay": "phone", actors.CLI_SOURCE: "cli", "github": "github"}


class _Duplicate(Exception):
    """An append found the same answer to the same ask already kept."""


def default_state_dir() -> Path:
    """The host's state folder (`engine_home.state_dir`), which holds the cockpit's records (`cockpit/actions.jsonl` too)."""
    return engine_home.state_dir()


def path(state_dir: Path) -> Path:
    """The decided-asks log under `state_dir`."""
    return Path(state_dir) / FILE


def wait_fields(wait: waits.Wait) -> dict:
    """The part of a wait a decided ask keeps: its id and key, kind, project, task (the lane's name), the lane's title and
    the ask's title."""
    kind = {SCHEDULED_APPROVAL: waits.APPROVAL, waits.TRACK_DECISION: waits.QUESTION}.get(wait.kind, wait.kind)
    return {"id": wait.id, "key": json.loads(wait.id), "kind": kind, "project": wait.project, "task": wait.task,
            "lane_title": wait.lane_title, "title": wait.title}


def find_wait(repo: Path, stores: list[store.TaskStore], kind: str, subject: str | None) -> dict | None:
    """The fields of the open wait of `kind` whose name (without its revision) is `subject`, any name for `ANY_SUBJECT`,
    among the waits of `stores` (a repository's proposals when `stores` is empty); None when there is none or the
    engine's records cannot be read now."""
    try:
        found = waits.collect_stores({Path(repo): stores})
    except (OSError, ValueError):
        return None
    wait = next((wait for wait in found if wait.kind == kind and _has_subject(wait.id, subject)), None)
    return None if wait is None else wait_fields(wait)


def fallback_wait(repo: Path, task: str, kind: str, subject: str) -> dict:
    """The fields of an ask whose engine record is no longer readable: what the command itself knows, with the ask's title
    in words from its gate and the lane's title when the lane still has its state."""
    key = ["wait", str(repo), task, kind, subject]
    title = (waits.gate_title(subject) if kind == waits.APPROVAL else ask_words.BLOCKED_TITLE if kind == waits.BLOCKED else "")
    return {"id": json.dumps(key, separators=(",", ":")), "key": key, "kind": kind, "project": Path(repo).name,
            "task": task, "lane_title": waits.lane_title_of(Path(repo), task) if kind in (waits.APPROVAL, waits.BLOCKED) else "",
            "title": title}


def record(state_dir: Path, *, wait: dict, answer: str, choice: str | None, text: str | None, actor: str,
           at: datetime) -> None:
    """Keep that `actor` answered `wait` (see `wait_fields`) with `answer` at `at`, with the `choice` pressed and the
    `text` typed, when there are any. Raises ValueError for an answer that is not one of `ANSWERS`. An answer already
    kept for the same ask within `DUPLICATE_SECONDS` is not kept twice."""
    if answer not in ANSWERS:
        raise ValueError(f"{answer!r} is not an answer; use one of {', '.join(ANSWERS)}")
    entry = decision_entry(wait, answer, choice, text, actor, at)
    target = path(state_dir)
    try:
        target.parent.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
        appendlog.append(target, entry, room=lambda: _require_new(target, entry))
    except _Duplicate:
        return
    except OSError as error:
        print(f"cockpit: cannot write {target}: {strip_controls(str(error))}: decided {answer!r} was not kept",
              file=sys.stderr)


def record_now(wait: dict, answer: str, actor: str, *, choice: str | None = None, text: str | None = None) -> None:
    """`record` in the default state folder, at this moment: what an operator command calls once its answer is applied."""
    record(default_state_dir(), wait=wait, answer=answer, choice=choice, text=text, actor=actor,
           at=datetime.now().astimezone())


def recent(state_dir: Path, *, now: datetime, days: int = RETENTION_DAYS, limit: int = MAX_LISTED,
           track_entries: Iterable[dict] = ()) -> list[dict]:
    """The answers of the last `days` days, newest first, at most `limit`; nothing is created and a line that is not
    a whole entry is skipped."""
    from sakura.cockpit import proposal_actions

    cutoff = now - timedelta(days=days)
    proposals = list(proposal_actions.decisions(state_dir, cutoff))
    projected = {entry["id"] for entry in proposals}
    logged = (entry for entry in _tail(path(state_dir), READ_TAIL_BYTES)
              if entry.get("answer") not in (proposal_actions.STARTED, proposal_actions.DISMISSED)
              or entry.get("id") not in projected)
    dated = [(when, entry) for entry in (*logged, *proposals, *track_entries) if (when := _when(entry)) is not None]
    dated.reverse()
    dated.sort(key=lambda pair: pair[0], reverse=True)
    return [_public_entry(entry) for when, entry in dated if when >= cutoff][:limit]


def _public_entry(entry: dict) -> dict:
    fields = {name: entry.get(name) for name in PUBLIC_FIELDS}
    if fields["place"] not in PLACES.values():
        fields["place"] = _place(fields["actor"] or "")
    return fields


def _place(actor: str) -> str:
    return PLACES.get(str(actor).partition(":")[0], "cli")


# Entries

def decision_entry(wait: dict, answer: str, choice: str | None, text: str | None, actor: str, at: datetime) -> dict:
    """Build the public decision fields from the stored answer and wait."""
    cleaned = {name: strip_controls(str(wait.get(name, ""))) for name in ("id", "kind", "project", "task", "lane_title", "title")}
    return {**cleaned, "key": [strip_controls(str(part)) for part in wait["key"]], "answer": answer,
            "choice": _plain(choice), "text": _plain(text, MAX_TEXT), "actor": strip_controls(actor),
            "place": _place(actor), "at": text_limits.utc_text(at), "lane": _plain(wait.get("lane"))}


def _plain(value: str | None, limit: int | None = None) -> str | None:
    return strip_controls(value)[:limit] if value else None


def _when(entry: dict) -> datetime | None:
    try:
        return datetime.fromisoformat(entry["at"]) if isinstance(entry.get("answer"), str) else None
    except (KeyError, TypeError, ValueError):
        return None


def _has_subject(wait_id: str, subject: str | None) -> bool:
    return subject is ANY_SUBJECT or waits.split_key(json.loads(wait_id)[NAME_POSITION])[0] == subject


# Duplicates

def _require_new(target: Path, entry: dict) -> None:
    """Raise `_Duplicate` when the same answer to the same ask is among the last lines of the log."""
    if any(_same_answer(kept, entry) for kept in _tail(target, DUPLICATE_TAIL_BYTES)):
        raise _Duplicate


def _same_answer(kept: dict, entry: dict) -> bool:
    when, other = _when(kept), _when(entry)
    if when is None or other is None or kept.get("answer") != entry["answer"]:
        return False
    ask, kept_ask = _ask_of(entry), _ask_of(kept)
    if ask is None or kept_ask is None or ask[0] != kept_ask[0]:
        return False
    revisions = (ask[1], kept_ask[1])
    return (revisions[0] == revisions[1] or None in revisions) and abs(other - when) <= timedelta(seconds=DUPLICATE_SECONDS)


def _ask_of(entry: dict) -> tuple[tuple, str | None] | None:
    """What an answer answers: the repository, task, kind and name of the ask, and the revision named, if one was. None
    for an entry whose key is not a wait's."""
    key = entry.get("key")
    if not isinstance(key, list) or len(key) != WAIT_KEY_LENGTH:
        return None
    name, revision = waits.split_key(str(key[NAME_POSITION]))
    return (*key[1:NAME_POSITION], name), revision or None


# Reading

def _tail(target: Path, size: int) -> Iterable[dict]:
    """The whole entries in the last `size` bytes of the log, oldest first; a missing log has none."""
    try:
        with target.open("rb") as stream:
            end = stream.seek(0, 2)
            stream.seek(max(0, end - size))
            raw = stream.read()
            cut = end > size
    except FileNotFoundError:
        return []
    lines = raw.splitlines()[1 if cut else 0:]
    return [entry for line in lines if (entry := _parse(line)) is not None]


def _parse(line: bytes) -> dict | None:
    try:
        entry = json.loads(line)
    except ValueError:
        return None
    return entry if isinstance(entry, dict) else None
