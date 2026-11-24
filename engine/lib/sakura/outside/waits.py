"""The waits of outside lanes: each open owner row and each open question is one wait of the engine's one list.

Waits are derived from the lane record and its log (`inbox.open_rows`, `inbox.open_questions`), never stored. The id is
`["wait", "outside", lane, kind, "<row or question>@<digest>"]` as compact JSON (an engine wait carries an absolute
path in the second place, so the two never collide); the digest in it makes an answer name the exact content the
person saw. The text of a wait is the agent's: the cockpit shows it escaped and never acts on it.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from sakura import actors
from sakura.flow import waits
from sakura.outside import inbox, store
from sakura.outside.records import Lane, OwnerRow
from sakura.questions import Question

SOURCE = "outside"
TITLE_LIMIT = 200
MAX_LINKS = 8
# "<label>: <https URL>", the form the digest and most agents write a titled link in; the label is one short phrase.
LINK_PAIR = re.compile(r"(?P<label>[^\s:][^:\n]{0,79}?): (?P<url>https://[^\s]+)")


def collect(root: Path) -> list[waits.Wait]:
    """The open waits of every open outside lane, oldest first; an unreadable lane is skipped (see `store.scan`)."""
    lanes, _ = store.scan(root)
    return waits.ordered([wait for lane in lanes for wait in of_lane(lane, store.log(root, lane.id))])


def of_lane(lane: Lane, log: list[dict]) -> list[waits.Wait]:
    """The waits of one lane: its open rows, then its open questions."""
    return ([_row_wait(lane, row) for row in inbox.open_rows(lane, log)]
            + [_question_wait(lane, question) for question in inbox.open_questions(lane, log)])


def author_of(lane: Lane) -> str:
    """The author text of everything the lane's agent wrote: `outside:<name>`."""
    return author_of_agent(lane.agent.name)


def author_of_agent(name: str) -> str:
    """`outside:<name>` for the outside agent called `name`."""
    return actors.format(actors.Actor(actors.OUTSIDE_SOURCE, name))


def split_links(text: str) -> tuple[str, tuple[tuple[str, str], ...]]:
    """`text` without its `label: https://...` pairs, and the pairs: the person reads the text and opens the titled links
    beside it, never a raw address inside a sentence. A label starts after the last full stop before it."""
    found: list[tuple[str, str]] = []

    def take(match: re.Match) -> str:
        if len(found) >= MAX_LINKS:
            return match[0]
        sentence, stop, label = match["label"].rpartition(". ")
        found.append((label.strip(), match["url"]))
        return sentence + stop

    rest = LINK_PAIR.sub(take, text)
    return "\n".join(" ".join(line.split()) for line in rest.split("\n")).strip(), tuple(found)


def _row_wait(lane: Lane, row: OwnerRow) -> waits.Wait:
    steps = [row.steps[0]] if len(row.steps) == 1 else [f"{number}. {step}" for number, step in enumerate(row.steps, start=1)]
    note, links = split_links(row.note)
    expected = [f"Expected: {row.expected}"] if row.expected else []
    lines = [*steps, *expected, *([f"Note: {note}"] if note else [])]
    return _make(lane, waits.OWNER_ROW, row.id, row.digest, row.title, "\n".join(lines), row.since, (), links)


def _question_wait(lane: Lane, question: Question) -> waits.Wait:
    text, links = split_links(question.text)
    title = " ".join(text.split())[:TITLE_LIMIT]
    return _make(lane, waits.QUESTION, question.id, question.digest, title, text, question.asked_at,
                 question.choices, links)


def _make(lane: Lane, kind: str, name: str, digest: str, title: str, detail: str, since: str,
          choices: tuple[str, ...], links: tuple[tuple[str, str], ...]) -> waits.Wait:
    identity = json.dumps(["wait", SOURCE, lane.id, kind, f"{name}@{digest}"], separators=(",", ":"))
    return waits.Wait(identity, kind, str(lane.agent.where), lane.agent.where.name, lane.id, title, detail, since,
                      SOURCE, author_of(lane), choices, links=links)
