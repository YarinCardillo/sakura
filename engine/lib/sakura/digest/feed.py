"""Feed sources: the URL of an RSS or Atom feed is a source with no program.

The engine fetches the URL in a child process that is ended at one absolute deadline (http or https, at most three
redirects, all within the configured origin, a bounded body; `fetch_worker.py`) and reads it with expat, which refuses a document type declaration and any entity declaration as soon as it
has decoded them, wherever they sit and whatever the encoding. Each entry becomes an item. The cursor is the time of
the newest entry seen, so the next run returns only entries after it.

The URL may hold a secret (a token in its query), so no message ever carries it: errors name the host and path, the
error class or the HTTP status, nothing else.
"""

from __future__ import annotations

import html
import os
import re
import sys
import urllib.parse
import xml.etree.ElementTree as ElementTree
from collections.abc import Callable
from datetime import timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from xml.parsers import expat

from sakura.digest import schema, supervise
from sakura.flow import process

ATOM = "{http://www.w3.org/2005/Atom}"
TAG = re.compile(r"<[^>]*>")
WORKER_MODULE = "sakura.digest.fetch_worker"
LIBRARY_FOLDER = Path(__file__).resolve().parents[2]
WORKER_TOKEN = re.compile(r"HTTP \d{3}|redirect refused|too many redirects|[A-Za-z]{1,40}")
NAMESPACE_SEPARATOR = " "
USER_AGENT = "sakura-digest"

Fetcher = Callable[[str, int], bytes]


class FeedError(ValueError):
    """Raised, with a one-line reason, when a feed cannot be fetched or read."""


def fetch(url: str, timeout_seconds: int) -> bytes:
    """The body of `url`, at most `schema.MAX_SOURCE_BYTES` of it, all within `timeout_seconds` in total.

    The fetch runs in a child process (`fetch_worker.py`) that is ended at the absolute deadline, whatever the server
    does with its headers or its body. Raises FeedError, never with the URL's userinfo or query in the text.
    """
    environment = {**process.child_environment(os.environ), "PYTHONPATH": str(LIBRARY_FOLDER)}
    outcome = supervise.run_program((sys.executable, "-P", "-m", WORKER_MODULE), environment, timeout_seconds,
                                    stdin_text=url.encode())
    if outcome.start_error:
        raise FeedError("cannot start the fetch")
    if outcome.timed_out:
        raise FeedError("the feed took longer than its time to arrive")
    if outcome.over_limit:
        raise FeedError(f"the feed is larger than {schema.MAX_SOURCE_BYTES} bytes")
    if outcome.exit_code != 0:
        raise FeedError(f"cannot fetch {shown_url(url)}: {_reason(outcome.stderr)}")
    return outcome.stdout


def _reason(token: str) -> str:
    """What the worker said it failed on, when that is one of its own short tokens; else a fixed word."""
    return token if WORKER_TOKEN.fullmatch(token) else "failed"


def shown_url(url: str) -> str:
    """`url` as it may be shown: scheme, host, port and path only, no userinfo, query or fragment."""
    try:
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname or ""
        port = f":{parts.port}" if parts.port else ""
    except ValueError:
        return "the feed URL"
    return f"{parts.scheme}://{host}{port}{parts.path}"


def entries(body: bytes, since: str, cursor: str) -> tuple[list[dict], int, str]:
    """The new entries of a feed as item documents (all of them: the intake applies the per-source limit), the number
    of entries that could not be read, and the new cursor.

    An entry is new when it is dated after the cursor (else after `since`); an undated entry only counts on a first
    run (no cursor). Raises FeedError for a document that is not an RSS or Atom feed.
    """
    root = _parse(body)
    threshold = cursor or since
    found, skipped, newest = [], 0, cursor
    for node in [*root.iter(f"{ATOM}entry"), *root.iter("item")]:
        try:
            item = _atom_item(node) if node.tag == f"{ATOM}entry" else _rss_item(node)
        except (ValueError, OverflowError, schema.SchemaError):
            skipped += 1
            continue
        newest = max(newest, item.get("at", ""))
        if _is_new(item.get("at", ""), threshold, bool(cursor)):
            found.append(item)
    return found, skipped, newest


def _parse(body: bytes) -> ElementTree.Element:
    """The document's root, built from expat's own events: a document type or entity declaration stops the parse."""
    builder = ElementTree.TreeBuilder()
    parser = expat.ParserCreate(namespace_separator=NAMESPACE_SEPARATOR)
    parser.buffer_text = True
    parser.StartElementHandler = lambda tag, attributes: builder.start(
        _name(tag), {_name(key): value for key, value in attributes.items()})
    parser.EndElementHandler = lambda tag: builder.end(_name(tag))
    parser.CharacterDataHandler = builder.data
    parser.StartDoctypeDeclHandler = _refuse_declaration
    parser.EntityDeclHandler = _refuse_declaration
    parser.ExternalEntityRefHandler = _refuse_declaration
    try:
        parser.Parse(body, True)
        root = builder.close()
    except FeedError:
        raise
    except expat.ExpatError as error:
        raise FeedError(f"the feed is not valid XML: {expat.ErrorString(error.code)}") from None
    except (ValueError, AssertionError):
        raise FeedError("the feed is not valid XML") from None
    if root.tag not in (f"{ATOM}feed", "rss", "{http://www.w3.org/1999/02/22-rdf-syntax-ns#}RDF"):
        raise FeedError(f"the document is not an RSS or Atom feed (it starts with <{root.tag.rsplit('}', 1)[-1]}>)")
    return root


def _name(tag: str) -> str:
    """expat's `uri local` name as ElementTree spells it, `{uri}local`."""
    namespace, found, local = tag.rpartition(NAMESPACE_SEPARATOR)
    return f"{{{namespace}}}{local}" if found else tag


def _refuse_declaration(*_arguments) -> None:
    raise FeedError("the feed carries a document type or entity declaration, which is refused")


def _is_new(at: str, threshold: str, has_cursor: bool) -> bool:
    if not at:
        return not has_cursor
    return not threshold or at > threshold


def _atom_item(node: ElementTree.Element) -> dict:
    link = next((child.get("href", "") for child in node.findall(f"{ATOM}link")
                 if child.get("rel", "alternate") == "alternate"), "")
    when = _first_text(node, f"{ATOM}updated", f"{ATOM}published")
    identity = _first_text(node, f"{ATOM}id") or link
    body = _first_text(node, f"{ATOM}content", f"{ATOM}summary")
    return _document(identity, _first_text(node, f"{ATOM}title"), link, _atom_time(when), body)


def _rss_item(node: ElementTree.Element) -> dict:
    link = _first_text(node, "link")
    identity = _first_text(node, "guid") or link
    return _document(identity, _first_text(node, "title"), link, _rss_time(_first_text(node, "pubDate")),
                     _first_text(node, "description"))


def _document(identity: str, title: str, link: str, at: str, body: str) -> dict:
    document = {"id": identity.strip()[:schema.MAX_ITEM_ID], "title": " ".join(html.unescape(TAG.sub(" ", title)).split())
                [:schema.MAX_ITEM_TITLE], "url": link.strip(), "at": at,
                "text": " ".join(html.unescape(TAG.sub(" ", body)).split())[:schema.MAX_ITEM_TEXT]}
    schema.parse_item(document)
    return document


def _first_text(node: ElementTree.Element, *names: str) -> str:
    for name in names:
        found = node.find(name)
        if found is not None and (found.text or "").strip():
            return found.text.strip()
    return ""


def _atom_time(value: str) -> str:
    return schema.time_text(value) if value else ""


def _rss_time(value: str) -> str:
    if not value:
        return ""
    moment = parsedate_to_datetime(value)
    if moment.tzinfo is None:
        moment = moment.replace(tzinfo=timezone.utc)
    return schema.time_text(moment.astimezone(timezone.utc).isoformat())
