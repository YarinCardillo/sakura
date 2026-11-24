"""The digest's data: the items a source prints, what the model may write, and the JSON file of one run (schema 1).

Everything here comes from outside the engine (a person's program, a feed, a model that read text others wrote), so
every field is bounded and checked, a hidden character is refused, and an unknown key is refused. A violation is a
`SchemaError` with a one-line reason; nothing is repaired.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from datetime import datetime, timezone

from sakura import text_limits

SCHEMA_VERSION = 1
MAX_ITEMS_PER_SOURCE = 200
MAX_SOURCE_BYTES = 1_000_000
MAX_ITEM_ID, MAX_ITEM_TITLE, MAX_ITEM_TEXT, MAX_URL, MAX_LABEL = 200, 200, 4000, 500, 40
MAX_LINKS = 4
MAX_NEWS, MAX_NEWS_TITLE, MAX_NEWS_TEXT = 12, 120, 400
MAX_CHECKS, MAX_CHECK_TITLE, MAX_CHECK_WHY = 15, 200, 300
MAX_DECISIONS, MAX_QUESTION, MAX_CHOICES, MAX_CHOICE = 10, 1000, 6, 80
MAX_ERROR = 300
ENTRY_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")
SOURCE_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
LANE_ID = re.compile(r"digest-\d{4}-\d{2}-\d{2}")
DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
HTTP_SCHEMES = ("https://", "http://")


class SchemaError(ValueError):
    """Raised, with a one-line reason that names the field, when data breaks the schema."""


@dataclass(frozen=True)
class Item:
    """One thing a source found; `at` is a UTC ISO time, empty when the source gave none."""

    id: str
    title: str
    url: str = ""
    at: str = ""
    text: str = ""


@dataclass(frozen=True)
class Link:
    """A reference the person can open: a short label and the URL of an item."""

    label: str
    url: str


@dataclass(frozen=True)
class News:
    """News that needs no action."""

    source: str
    title: str
    text: str
    links: tuple[Link, ...] = ()


@dataclass(frozen=True)
class Check:
    """A check only the person can do."""

    id: str
    title: str
    why: str
    links: tuple[Link, ...] = ()


@dataclass(frozen=True)
class Decision:
    """A real decision, asked as a question; no choices means an answer in words."""

    id: str
    question: str
    choices: tuple[str, ...] = ()
    links: tuple[Link, ...] = ()


@dataclass(frozen=True)
class ModelOutput:
    """What the model may write: news, checks and decisions, nothing else."""

    news: tuple[News, ...] = ()
    checks: tuple[Check, ...] = ()
    decisions: tuple[Decision, ...] = ()

    def urls(self) -> set[str]:
        """Every URL the output links to."""
        entries = (*self.news, *self.checks, *self.decisions)
        return {link.url for entry in entries for link in entry.links}


@dataclass(frozen=True)
class SourceReport:
    """What one source did in this run: `error` is empty when it ran well."""

    id: str
    ok: bool
    items: int
    skipped: int
    error: str = ""


@dataclass(frozen=True)
class Window:
    """The time the digest covers, UTC ISO times."""

    since: str
    until: str


@dataclass(frozen=True)
class Digest:
    """One run's result, the document written to `<date>.json` and `latest.json`."""

    date: str
    generated_at: str
    window: Window
    model: str
    lane: str
    sources: tuple[SourceReport, ...]
    output: ModelOutput


# Items

def parse_item(value: object) -> Item:
    """The item of one decoded JSON line; raises SchemaError for anything that is not a bounded, clean item."""
    data = _object(value, "item", {"id", "title", "url", "at", "text"})
    url = _optional(data, "url", "")
    return Item(_line(data, "id", MAX_ITEM_ID), _line(data, "title", MAX_ITEM_TITLE), _checked_url(url, "url") if url else "",
                _time(_optional(data, "at", "")), _text(data, "text", MAX_ITEM_TEXT, required=False))


def time_text(value: str) -> str:
    """`value` as the UTC ISO text the digest writes, or "" for an empty value; raises SchemaError for a bad time."""
    return _time(value)


# Model output

def parse_model_output(text: str) -> ModelOutput:
    """The model's output file, decoded and checked; raises SchemaError."""
    try:
        data = json.loads(text)
    except (ValueError, RecursionError) as error:
        raise SchemaError(f"output is not JSON: {error}") from error
    return output_from_json(data)


def output_from_json(value: object) -> ModelOutput:
    """The model output of a decoded document; raises SchemaError."""
    data = _object(value, "output", {"news", "checks", "decisions"})
    return ModelOutput(tuple(_news(item) for item in _list(data, "news", MAX_NEWS)),
                       _unique(tuple(_check(item) for item in _list(data, "checks", MAX_CHECKS)), "checks"),
                       _unique(tuple(_decision(item) for item in _list(data, "decisions", MAX_DECISIONS)), "decisions"))


def output_to_json(output: ModelOutput) -> dict:
    """The JSON document of a model output."""
    return {"news": [{"source": item.source, "title": item.title, "text": item.text, "links": _links_json(item.links)}
                     for item in output.news],
            "checks": [{"id": item.id, "title": item.title, "why": item.why, "links": _links_json(item.links)}
                       for item in output.checks],
            "decisions": [{"id": item.id, "question": item.question, "choices": list(item.choices),
                           "links": _links_json(item.links)} for item in output.decisions]}


# The file of one run

def digest_to_json(digest: Digest) -> dict:
    """The JSON document of a run, schema version 1."""
    return {"schema": SCHEMA_VERSION, "date": digest.date, "generated_at": digest.generated_at,
            "window": {"since": digest.window.since, "until": digest.window.until}, "model": digest.model,
            "lane": digest.lane,
            "sources": [{"id": item.id, "ok": item.ok, "items": item.items, "skipped": item.skipped,
                         "error": item.error} for item in digest.sources],
            **output_to_json(digest.output)}


def digest_from_json(value: object) -> Digest:
    """The run of a decoded document; raises SchemaError for another version or a document that breaks the schema."""
    data = _object(value, "digest", {"schema", "date", "generated_at", "window", "model", "lane", "sources",
                                     "news", "checks", "decisions"})
    if data.get("schema") != SCHEMA_VERSION:
        raise SchemaError(f"schema must be {SCHEMA_VERSION}; got {str(data.get('schema'))[:20]!r}")
    window = _object(data.get("window"), "window", {"since", "until"})
    return Digest(_matching(data, "date", DATE), _time(_required(data, "generated_at")),
                  Window(_time(_required(window, "since")), _time(_required(window, "until"))),
                  _line(data, "model", 64), _matching(data, "lane", LANE_ID),
                  tuple(_report(item) for item in _list(data, "sources", 100)),
                  output_from_json({name: data.get(name, []) for name in ("news", "checks", "decisions")}))


# Entries

def _news(value: object) -> News:
    data = _object(value, "news entry", {"source", "title", "text", "links"})
    return News(_matching(data, "source", SOURCE_ID), _line(data, "title", MAX_NEWS_TITLE),
                _text(data, "text", MAX_NEWS_TEXT), _links(data))


def _check(value: object) -> Check:
    data = _object(value, "check", {"id", "title", "why", "links"})
    return Check(_matching(data, "id", ENTRY_ID), _line(data, "title", MAX_CHECK_TITLE),
                 _line(data, "why", MAX_CHECK_WHY), _links(data))


def _decision(value: object) -> Decision:
    data = _object(value, "decision", {"id", "question", "choices", "links"})
    choices = tuple(_line_value(choice, "choice", MAX_CHOICE) for choice in _list(data, "choices", MAX_CHOICES))
    if len(set(choices)) != len(choices):
        raise SchemaError("decision choices must be distinct")
    return Decision(_matching(data, "id", ENTRY_ID), _text(data, "question", MAX_QUESTION), choices, _links(data))


def _report(value: object) -> SourceReport:
    data = _object(value, "source report", {"id", "ok", "items", "skipped", "error"})
    flag = data.get("ok")
    if not isinstance(flag, bool):
        raise SchemaError("source report ok must be true or false")
    return SourceReport(_matching(data, "id", SOURCE_ID), flag, _count(data, "items"), _count(data, "skipped"),
                        _text(data, "error", MAX_ERROR, required=False))


def _links(data: dict) -> tuple[Link, ...]:
    entries = _list(data, "links", MAX_LINKS)
    found = []
    for entry in entries:
        link = _object(entry, "link", {"label", "url"})
        found.append(Link(_line(link, "label", MAX_LABEL), _checked_url(_line(link, "url", MAX_URL), "url")))
    return tuple(found)


def _links_json(links: tuple[Link, ...]) -> list[dict]:
    return [{"label": link.label, "url": link.url} for link in links]


def _unique(entries: tuple, name: str) -> tuple:
    ids = [entry.id for entry in entries]
    if len(set(ids)) != len(ids):
        raise SchemaError(f"{name} ids must be distinct")
    return entries


# Values

def _object(value: object, name: str, keys: set[str]) -> dict:
    if not isinstance(value, dict):
        raise SchemaError(f"{name} must be an object")
    unknown = sorted(set(value) - keys)
    if unknown:
        raise SchemaError(f"{name} has unknown field {str(unknown[0])[:40]!r}")
    return value


def _list(data: dict, name: str, limit: int) -> list:
    value = data.get(name, [])
    if not isinstance(value, list):
        raise SchemaError(f"{name} must be a list")
    if len(value) > limit:
        raise SchemaError(f"{name} has {len(value)} entries; the limit is {limit}")
    return value


def _required(data: dict, name: str) -> str:
    value = data.get(name)
    if not isinstance(value, str):
        raise SchemaError(f"{name} must be text")
    return value


def _optional(data: dict, name: str, default: str) -> str:
    value = data.get(name, default)
    if not isinstance(value, str):
        raise SchemaError(f"{name} must be text")
    return value


def _line(data: dict, name: str, limit: int) -> str:
    return _line_value(data.get(name), name, limit)


def _line_value(value: object, name: str, limit: int) -> str:
    if not isinstance(value, str):
        raise SchemaError(f"{name} must be text")
    problem = text_limits.one_line_problem(value, limit)
    if problem:
        raise SchemaError(f"{name} {problem}")
    return value


def _text(data: dict, name: str, limit: int, *, required: bool = True) -> str:
    value = data.get(name, "" if not required else None)
    if not isinstance(value, str):
        raise SchemaError(f"{name} must be text")
    if not required and not value.strip():
        return ""
    problem = text_limits.text_problem(value, limit) or text_limits.hidden_character_problem(value)
    if problem:
        raise SchemaError(f"{name}: {problem}")
    return value


def _matching(data: dict, name: str, pattern: re.Pattern) -> str:
    value = data.get(name)
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise SchemaError(f"{name} must match {pattern.pattern}; got {str(value)[:40]!r}")
    return value


def _checked_url(url: str, name: str) -> str:
    if not url.startswith(HTTP_SCHEMES) or re.search(r"\s", url) or len(url) > MAX_URL:
        raise SchemaError(f"{name} must be one http(s) URL of at most {MAX_URL} characters without whitespace")
    problem = text_limits.one_line_problem(url, MAX_URL)
    if problem:
        raise SchemaError(f"{name} {problem}")
    return url


def _count(data: dict, name: str) -> int:
    value = data.get(name)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise SchemaError(f"{name} must be a whole number of 0 or more")
    return value


def _time(value: str) -> str:
    if not value:
        return ""
    try:
        moment = datetime.fromisoformat(value)
    except ValueError:
        raise SchemaError(f"time {value[:40]!r} is not an ISO 8601 time") from None
    if moment.tzinfo is None:
        raise SchemaError(f"time {value[:40]!r} has no time zone; give it as UTC, like 2026-11-17T06:12:00Z")
    try:
        return text_limits.utc_text(moment.astimezone(timezone.utc))
    except (OverflowError, ValueError):  # a date at the edge of the calendar has no UTC spelling
        raise SchemaError(f"time {value[:40]!r} is outside the years that can be converted to UTC") from None
