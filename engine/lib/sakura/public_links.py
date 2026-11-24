"""URLs suitable for publication."""

from __future__ import annotations

import re
from urllib.parse import urlsplit, urlunsplit

LINK = re.compile(r"""https?://[^\s<>"'\])]+""", re.IGNORECASE)


def url(value: str) -> str:
    """Return an HTTP(S) URL with user info, query and fragment removed."""
    try:
        parts = urlsplit(value)
        host = parts.netloc.rsplit("@", 1)[-1]
        if parts.scheme not in ("http", "https") or not parts.hostname or "\\" in parts.netloc:
            return ""
        return urlunsplit((parts.scheme, host, parts.path, "", ""))
    except ValueError:
        return ""


def text(value: str) -> str:
    """Clean URLs embedded in free text."""
    return LINK.sub(lambda match: url(match[0]) or "[link omitted]", value)
