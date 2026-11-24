"""Parse skipped-test reports from test runner output."""

from __future__ import annotations

import re
from dataclasses import dataclass, field

PYTEST_SKIP = re.compile(r"^SKIPPED \[(?P<n>\d+)\] (?P<path>[^:]+):\d+: (?P<reason>.+)$", re.MULTILINE)
SKIPPED_COUNT = re.compile(r"(\d+)\s+skipped")
UNREPORTED_REASON = "reason not reported"


@dataclass(frozen=True)
class SkipEntry:
    """One skipped test location and its reason; an empty path means unknown."""

    path: str
    reason: str


@dataclass(frozen=True)
class SkipReport:
    """Total skipped tests and the entries that explain them."""

    count: int = 0
    entries: list[SkipEntry] = field(default_factory=list)


def parse(output: str) -> SkipReport:
    """Parse runner output; skips without a reported reason add one unknown entry."""
    matches = list(PYTEST_SKIP.finditer(output))
    entries = [SkipEntry(m["path"], m["reason"].strip()) for m in matches]
    explained = sum(int(m["n"]) for m in matches)
    count = max([int(n) for n in SKIPPED_COUNT.findall(output)] + [explained])
    if count > explained:
        entries.append(SkipEntry("", UNREPORTED_REASON))
    return SkipReport(count, entries)


def merge(reports: list[SkipReport]) -> SkipReport:
    """Combine the reports of several commands."""
    return SkipReport(sum(r.count for r in reports), [e for r in reports for e in r.entries])
