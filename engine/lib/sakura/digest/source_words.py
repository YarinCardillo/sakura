"""What a person reads for a digest source: its name and what it reads, in plain words.

The person's own file may give both (`label`, `reads`). Without them a connector source is named after its connector and
described by its kind, a feed by its host, and any other source by its id made into words.
"""

from __future__ import annotations

from urllib.parse import urlsplit

from sakura.connectors import kinds, registry
from sakura.digest.config import Source

READS_BY_KIND = {
    kinds.GITHUB: "Pull requests and issues that wait on you",
    kinds.NOTION: "Pages of your Notion workspace changed since the last read",
    kinds.LINEAR: "Linear issues assigned to you and changed since the last read",
}


def label_of(source: Source) -> str:
    """The name of `source`: its own label, else its connector's, else its id as words (`ops-board` is `Ops board`)."""
    return source.label or _connector_label(source) or source.id.replace("-", " ").replace("_", " ").capitalize()


def reads_of(source: Source) -> str:
    """What `source` reads: its own text, else what its kind reads, else an empty string."""
    return source.reads or _default_reads(source)


def _connector_label(source: Source) -> str:
    found = _connector(source)
    return found.label if found else registry.LABELS.get(source.connector, "")


def _default_reads(source: Source) -> str:
    if source.feed:
        return f"New entries of {urlsplit(source.feed).hostname or 'the feed'}"
    found = _connector(source)
    return READS_BY_KIND.get(found.kind if found else source.connector, "")


def _connector(source: Source) -> registry.Connector | None:
    """The connector the source names, None when there is none or the connector file cannot be read."""
    if not source.connector:
        return None
    try:
        return registry.find(source.connector)
    except registry.ConnectorError:
        return None
