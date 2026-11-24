"""Path patterns with `**` support, as used by `brakes.test_patterns`."""

from __future__ import annotations

import re
from functools import lru_cache


def matches_any(path: str, patterns: list[str]) -> bool:
    """Return whether the repo-relative `path` matches any pattern.

    `**/` matches any number of directories (including none), `**` matches
    anything, `*` and `?` stay inside one path segment.
    """
    return any(_compile(pattern).fullmatch(path) for pattern in patterns)


@lru_cache(maxsize=None)
def _compile(pattern: str) -> re.Pattern[str]:
    out, index = [], 0
    while index < len(pattern):
        if pattern.startswith("**/", index):
            out.append("(?:.*/)?")
            index += 3
        elif pattern.startswith("**", index):
            out.append(".*")
            index += 2
        elif pattern[index] == "*":
            out.append("[^/]*")
            index += 1
        elif pattern[index] == "?":
            out.append("[^/]")
            index += 1
        else:
            out.append(re.escape(pattern[index]))
            index += 1
    return re.compile("".join(out))
