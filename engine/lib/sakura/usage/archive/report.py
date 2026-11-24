"""Per pull request usage rows read from the archive views."""

from __future__ import annotations

import sqlite3
from datetime import datetime, timezone
from typing import NamedTuple

TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
HEADER = ("PR", "branch", "state", "sessions", "turns", "input", "cache write", "cache read", "output",
          "cost", "files", "additions", "deletions")
LINKED_ROWS = """
SELECT p.repo, p.number, p.head_branch, p.merged_at, p.closed_at, p.is_draft,
  COUNT(DISTINCT t.provider || ':' || t.session_id), COUNT(*), SUM(t.input_tokens),
  SUM(t.cache_write_tokens), SUM(t.cache_read_tokens), SUM(t.output_tokens), SUM(t.cost_usd),
  p.changed_files, p.additions, p.deletions
FROM pull_requests p JOIN pr_turns t ON t.repo = p.repo AND t.number = p.number
WHERE (:repo IS NULL OR p.repo = :repo) AND (:since IS NULL OR p.created_at >= :since)
GROUP BY p.repo, p.number ORDER BY p.created_at DESC, p.number DESC
"""


class PrReport(NamedTuple):
    rows: list[tuple]
    linked_turns: int
    branch_turns: int
    pull_requests: int


def by_pr(conn: sqlite3.Connection, repo: str | None, created_since: datetime | None) -> PrReport:
    """Usage rows of pull requests (optionally one repo, created since a moment) plus the link totals."""
    since = None if created_since is None else created_since.astimezone(timezone.utc).strftime(TIME_FORMAT)
    rows = conn.execute(LINKED_ROWS, {"repo": repo, "since": since}).fetchall()
    linked = conn.execute("SELECT COUNT(*) FROM (SELECT DISTINCT provider, event_key FROM pr_turns)").fetchone()[0]
    on_branches = conn.execute("SELECT COUNT(*) FROM turns WHERE branch IS NOT NULL").fetchone()[0]
    total = conn.execute("SELECT COUNT(*) FROM pull_requests WHERE (:repo IS NULL OR repo = :repo)"
                         " AND (:since IS NULL OR created_at >= :since)", {"repo": repo, "since": since}).fetchone()[0]
    return PrReport(rows, linked, on_branches, total)


def render(report: PrReport) -> str:
    """The aligned table followed by the linkage summary line."""
    lines = [format_table(HEADER, [_format(row) for row in report.rows])]
    lines.append(f"linked turns {report.linked_turns} of {report.branch_turns} on branches, "
                 f"PRs with usage {len(report.rows)} of {report.pull_requests}")
    return "\n".join(lines)


def format_table(header: tuple[str, ...], rows: list[tuple[str, ...]]) -> str:
    """Columns aligned by the widest cell, two spaces apart, trailing blanks trimmed."""
    table = [header] + rows
    widths = [max(len(line[column]) for line in table) for column in range(len(header))]
    return "\n".join("  ".join(cell.ljust(width) for cell, width in zip(line, widths)).rstrip() for line in table)


def _format(row: tuple) -> tuple[str, ...]:
    repo, number, branch, merged_at, closed_at, is_draft, sessions, turns, *tokens, cost, files, added, deleted = row
    state = "merged" if merged_at else "closed" if closed_at else "draft" if is_draft else "open"
    return (f"{repo}#{number}", branch, state, str(sessions), str(turns), *(str(value) for value in tokens),
            "-" if cost is None else f"{cost:.2f}", str(files), str(added), str(deleted))
