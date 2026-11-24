"""The records of an outside lane and the checks on what an outside agent writes into them.

An outside agent is untrusted like every agent output: every field is bounded and checked before it is stored.
A refusal is `Refused` with a one-line reason; a record that cannot be read back is `StoreError`.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from sakura import repository, text_limits
from sakura.questions import Question

ID_PATTERN = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")
PHASES, OUTCOMES = ("draft", "ready"), ("done", "dropped")
MAX_TITLE, MAX_ABOUT, MAX_STEP, MAX_EXPECTED, MAX_NOTE = 200, 2000, 300, 1000, 500
MAX_LABEL, MAX_URL, MAX_STEPS, MAX_LINKS = 40, 500, 20, 4
HTTPS = "https://"


class Refused(ValueError):
    """Raised, before anything is written, with a one-line reason that names the rule broken."""


class StoreError(OSError):
    """Raised when a record or log file of the store cannot be read or written."""


@dataclass(frozen=True)
class Agent:
    """An outside agent: the name it gives and the folder it runs in (the main worktree of its repository)."""

    name: str
    where: Path


@dataclass(frozen=True)
class Link:
    """A reference the person can open: a short label and an https URL."""

    label: str
    url: str


@dataclass(frozen=True)
class OwnerRow:
    """A check only the person makes. `digest` names this content at this `revision`, which grows on every content
    change (a row restored to an earlier content has a new digest, so no earlier answer counts); `since` is when this
    content was added."""

    id: str
    title: str
    steps: tuple[str, ...]
    expected: str
    note: str
    digest: str
    since: str
    revision: int = 1


@dataclass(frozen=True)
class Lane:
    """One piece of work an outside agent follows. `closed_at` is None while open; `outcome` is empty until closed."""

    id: str
    agent: Agent
    title: str
    about: str
    links: tuple[Link, ...]
    phase: str
    rows: tuple[OwnerRow, ...]
    questions: tuple[Question, ...]
    opened_at: str
    updated_at: str
    closed_at: str | None
    outcome: str
    read_up_to: int
    pushed_at: str | None
    delivered_up_to: int = 0


def agent_here(name: str, cwd: Path) -> Agent:
    """The agent called `name` working in `cwd`: `where` is the main worktree of its repository, else `cwd`."""
    check_id("agent name", name)
    try:
        common = repository.common_dir(cwd)
    except (repository.RepositoryError, OSError):
        return Agent(name, cwd)
    return Agent(name, common.parent if common.name == ".git" else cwd)


def check_id(what: str, value: str) -> None:
    """Raise Refused unless `value` is a valid lane, row, question or agent id."""
    if not isinstance(value, str) or not ID_PATTERN.fullmatch(value):
        raise Refused(f"{what} {str(value)[:40]!r} must match {ID_PATTERN.pattern}")


def check_line(what: str, value: str, limit: int) -> None:
    """Raise Refused unless `value` is one printable line of 1 to `limit` characters."""
    problem = text_limits.one_line_problem(value, limit) if isinstance(value, str) else "must be text"
    if problem:
        raise Refused(f"{what} {problem}")


def check_text(what: str, value: str, limit: int, *, required: bool = True) -> None:
    """Raise Refused unless `value` is pasteable text of at most `limit` characters (blank passes if not required)."""
    if not isinstance(value, str):
        raise Refused(f"{what} must be text")
    if not required and not value:
        return
    problem = text_limits.text_problem(value, limit)
    if problem:
        raise Refused(f"{what}: {problem}")


def make_links(pairs: Sequence[tuple[str, str]]) -> tuple[Link, ...]:
    """The links of `pairs` (label, URL), checked: up to four, https only."""
    if len(pairs) > MAX_LINKS:
        raise Refused(f"{len(pairs)} links; the limit is {MAX_LINKS}")
    for label, url in pairs:
        check_line("link label", label, MAX_LABEL)
        _check_url(url)
    return tuple(Link(label, url) for label, url in pairs)


def make_row(row_id: str, *, title: str, steps: Sequence[str], expected: str, note: str, now_text: str,
             revision: int = 1) -> OwnerRow:
    """The owner row with these fields, checked, its digest computed from its content and its `revision`."""
    check_id("row id", row_id)
    check_line("row title", title, MAX_TITLE)
    if not 1 <= len(steps) <= MAX_STEPS:
        raise Refused(f"{len(steps)} steps; give 1 to {MAX_STEPS}")
    for step in steps:
        check_line("step", step, MAX_STEP)
    check_text("expected result", expected, MAX_EXPECTED, required=False)
    check_text("note", note, MAX_NOTE, required=False)
    fields = {"id": row_id, "title": title, "steps": list(steps), "expected": expected, "note": note,
              "revision": revision}
    return OwnerRow(row_id, title, tuple(steps), expected, note, text_limits.content_digest(fields), now_text, revision)


def lane_to_json(lane: Lane) -> dict:
    """The JSON document of a lane record."""
    return {"id": lane.id, "agent": {"name": lane.agent.name, "where": str(lane.agent.where)}, "title": lane.title,
            "about": lane.about, "links": [{"label": link.label, "url": link.url} for link in lane.links],
            "phase": lane.phase, "rows": [_row_to_json(row) for row in lane.rows],
            "questions": [_question_to_json(question) for question in lane.questions], "opened_at": lane.opened_at,
            "updated_at": lane.updated_at, "closed_at": lane.closed_at, "outcome": lane.outcome,
            "read_up_to": lane.read_up_to, "delivered_up_to": lane.delivered_up_to, "pushed_at": lane.pushed_at}


def lane_from_json(data: dict) -> Lane:
    """The lane of a record document; raises KeyError, TypeError or ValueError for a damaged one."""
    agent = Agent(str(data["agent"]["name"]), Path(data["agent"]["where"]))
    return Lane(str(data["id"]), agent, str(data["title"]), str(data["about"]),
                tuple(Link(item["label"], item["url"]) for item in data["links"]), str(data["phase"]),
                tuple(OwnerRow(item["id"], item["title"], tuple(item["steps"]), item["expected"], item["note"],
                               item["digest"], item["since"], int(item.get("revision", 1))) for item in data["rows"]),
                tuple(Question(item["id"], item["text"], tuple(item["choices"]), item["digest"], item["asked_at"])
                      for item in data["questions"]),
                str(data["opened_at"]), str(data["updated_at"]), data["closed_at"], str(data["outcome"]),
                int(data["read_up_to"]), data["pushed_at"], int(data.get("delivered_up_to", data["read_up_to"])))


def _check_url(url: str) -> None:
    if not isinstance(url, str) or not url.startswith(HTTPS) or len(url) == len(HTTPS):
        raise Refused(f"link must be an {HTTPS} URL")
    problem = text_limits.one_line_problem(url, MAX_URL)
    if problem or re.search(r"\s", url):
        raise Refused(f"link must be one {HTTPS} URL of at most {MAX_URL} characters without whitespace")


def _row_to_json(row: OwnerRow) -> dict:
    return {"id": row.id, "title": row.title, "steps": list(row.steps), "expected": row.expected, "note": row.note,
            "digest": row.digest, "since": row.since, "revision": row.revision}


def _question_to_json(question: Question) -> dict:
    return {"id": question.id, "text": question.text, "choices": list(question.choices), "digest": question.digest,
            "asked_at": question.asked_at}
