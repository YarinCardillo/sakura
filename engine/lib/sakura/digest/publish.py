"""The writers: one JSON file per run, and a lane on the Waiting page.

The files are the mirror other tools read: `<engine home>/digest/<date>.json` and `latest.json`, schema 1, written by
renaming a new file over the target. The lane is `digest-<date>` in the outside store, attributed to the agent
`digest`: the news in its text, each check as a row, each decision as a question, and the previous digests' lanes
closed. A day with nothing new gets a one-line text and no rows. Every field passes the store's own limits; the
digest's schema is already inside them, so a refusal here means the store is full or broken, and it is reported.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, replace
from datetime import datetime

from sakura import beneath, public_links
from sakura.digest import config, schema, state
from sakura.outside import records, store, verbs
from sakura.outside.records import Agent, Refused, StoreError

AGENT_NAME = "digest"
LANE_PHASE = "ready"
MORE_NEWS = "(and {count} more in the digest file)"
FAILED_SOURCES = "Could not read: {names}."
NOTHING_NEW = {"en": "Nothing new since {since}.", "it": "Niente di nuovo dal {since}."}
LINK_LINE = "{label}: {url}"


class PublishError(RuntimeError):
    """Raised, with a one-line reason, when the files or the lane cannot be written."""


@dataclass(frozen=True)
class Published:
    """What a publish wrote: the lane, how many rows and questions it holds, and the earlier lanes it closed."""

    lane: str
    rows: int
    questions: int
    closed: tuple[str, ...]


def publish(digest: schema.Digest, language: str, now: datetime, *, pusher: verbs.Pusher | None = None) -> Published:
    """Write the run's files and its lane; raises PublishError. `language` picks the line of a day with nothing new."""
    digest = _public_digest(digest)
    _write_files(digest)
    try:
        return _write_lane(digest, language, now, pusher)
    except (Refused, StoreError) as error:
        raise PublishError(f"the lane {digest.lane} cannot be written: {error}") from error


def write_files(digest: schema.Digest) -> None:
    """Write `<date>.json` and `latest.json` into the digest folder, each atomically, mode 0600."""
    _write_files(_public_digest(digest))


def _write_files(digest: schema.Digest) -> None:
    data = (json.dumps(schema.digest_to_json(digest), indent=1, ensure_ascii=False) + "\n").encode()
    try:
        folder = state.ensure_folder()
        for name in (f"{digest.date}.json", config.LATEST_FILE):
            beneath.write_bytes(folder, name, data, config.FILE_MODE)
    except (OSError, state.StateError) as error:
        raise PublishError(f"the digest files cannot be written: {error}") from error


def _public_digest(digest: schema.Digest) -> schema.Digest:
    output = digest.output
    reports = tuple(replace(report, error=public_links.text(report.error)) for report in digest.sources)
    news = tuple(replace(item, title=public_links.text(item.title), text=public_links.text(item.text),
                         links=_public_links(item.links)) for item in output.news)
    checks = tuple(replace(item, title=public_links.text(item.title), why=public_links.text(item.why),
                           links=_public_links(item.links)) for item in output.checks)
    decisions = tuple(replace(item, question=public_links.text(item.question),
                              choices=tuple(dict.fromkeys(map(public_links.text, item.choices))),
                              links=_public_links(item.links)) for item in output.decisions)
    return replace(digest, sources=reports, output=schema.ModelOutput(news, checks, decisions))


def _public_links(links: tuple[schema.Link, ...]) -> tuple[schema.Link, ...]:
    return tuple(schema.Link(public_links.text(link.label), url)
                 for link in links if (url := public_links.url(link.url)))


# The lane

def _write_lane(digest: schema.Digest, language: str, now: datetime, pusher: verbs.Pusher | None) -> Published:
    root, agent = store.path(), Agent(AGENT_NAME, config.digest_folder())
    output = digest.output
    opened = _open(root, agent, digest, language, now)
    for check in output.checks:
        _row(root, agent, digest.lane, check, now)
    asked = sum(_ask(root, agent, digest.lane, decision, now, pusher) for decision in output.decisions)
    return Published(opened, len(output.checks), asked, _close_earlier(root, agent, digest.lane, now))


def _open(root, agent: Agent, digest: schema.Digest, language: str, now: datetime) -> str:
    about = _about(digest, language)
    links = _lane_links(digest.output)
    verbs.open_lane(root, agent, {"lane": digest.lane, "title": f"Digest {digest.date}", "about": about,
                                  "phase": LANE_PHASE, "links": links}, now)
    return digest.lane


def _about(digest: schema.Digest, language: str) -> str:
    """The lane's text: the news that fit the store's limit, then the sources that failed; or the one line."""
    failed = [report.id for report in digest.sources if not report.ok]
    tail = FAILED_SOURCES.format(names=", ".join(failed)) if failed else ""
    news = [f"{item.title}: {item.text}" for item in digest.output.news]
    if not news:
        line = NOTHING_NEW.get(language, NOTHING_NEW["en"]).format(since=digest.window.since[:10])
        return f"{line} {tail}".strip()
    return _fitted(news, tail)


def _fitted(news: list[str], tail: str) -> str:
    """The news lines joined, as many as leave room for the tail and a count of the ones left out."""
    room = records.MAX_ABOUT - len(tail) - len(MORE_NEWS.format(count=len(news))) - 2
    kept, size = [], 0
    for line in news:
        if size + len(line) + 1 > room:
            break
        kept.append(line)
        size += len(line) + 1
    left = len(news) - len(kept)
    return "\n".join([*kept, *([MORE_NEWS.format(count=left)] if left else []), *([tail] if tail else [])])


def _lane_links(output: schema.ModelOutput) -> list[dict]:
    found = {}
    for entry in output.news:
        for link in entry.links:
            if link.url.startswith(records.HTTPS):
                found.setdefault(link.url, {"label": link.label, "url": link.url})
    return list(found.values())[:records.MAX_LINKS]


def _row(root, agent: Agent, lane: str, check: schema.Check, now: datetime) -> None:
    verbs.put_row(root, agent, {"lane": lane, "row": check.id, **row_fields(check)}, now)


def row_fields(check: schema.Check) -> dict:
    """The outside row content represented by a published check."""
    return {"title": check.title, "steps": [check.why], "note": _link_text(check.links, records.MAX_NOTE)}


def decision_text(decision: schema.Decision) -> str:
    """The outside question text represented by a published decision."""
    text = decision.question
    room = records.MAX_ABOUT - len(text) - 1
    return f"{text}\n{_link_text(decision.links, room)}".rstrip() if decision.links and room > 0 else text


def _ask(root, agent: Agent, lane: str, decision: schema.Decision, now: datetime, pusher) -> int:
    """Ask the decision; 0 when the person already answered it today (the same id is never asked twice)."""
    try:
        verbs.ask(root, agent, {"lane": lane, "question": decision.id, "text": decision_text(decision),
                                      "choices": list(decision.choices)},
                  now, pusher)
    except Refused as refusal:
        if "was answered" in str(refusal):
            return 0
        raise
    return 1


def _link_text(links: tuple[schema.Link, ...], limit: int) -> str:
    text = " ".join(LINK_LINE.format(label=link.label, url=link.url) for link in links)
    return text if len(text) <= limit else text[:limit].rsplit(" ", 1)[0]


def _close_earlier(root, agent: Agent, lane: str, now: datetime) -> tuple[str, ...]:
    """Close the digest lanes of earlier days that are still open (never another agent's lane)."""
    older = [item.id for item in store.lanes(root) if item.agent.name == agent.name and item.id != lane
             and schema.LANE_ID.fullmatch(item.id) and item.id < lane]
    for lane_id in older:
        verbs.close(root, agent, {"lane": lane_id, "outcome": "done"}, now)
    return tuple(older)
