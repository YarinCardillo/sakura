"""Pull request facts from `gh api graphql`, refreshed incrementally per repository."""

from __future__ import annotations

import json
import sqlite3
import subprocess
from collections.abc import Callable, Iterator, Sequence
from datetime import datetime, timedelta, timezone

Runner = Callable[[list[str]], subprocess.CompletedProcess]

WINDOW = timedelta(days=7)
SEARCH_LIMIT = 1000
MIN_SPLIT_SECONDS = 2
OVERLAP = timedelta(hours=1)
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
# GitHub answers HTTP 502 to a page of 100 pull requests with their timeline items.
PAGE_SIZE = 50
QUERY = f"""
query($q: String!, $cursor: String) {{
  search(query: $q, type: ISSUE, first: {PAGE_SIZE}, after: $cursor) {{
    issueCount pageInfo {{ hasNextPage endCursor }}
    nodes {{ ... on PullRequest {{
      number headRefName baseRefName changedFiles additions deletions commits {{ totalCount }}
      createdAt mergedAt closedAt isDraft labels(first: 20) {{ nodes {{ name }} }}
      timelineItems(itemTypes: [READY_FOR_REVIEW_EVENT], last: 1) {{ nodes {{ ... on ReadyForReviewEvent {{ createdAt }} }} }}
    }} }}
  }}
}}
"""
UPSERT = """
INSERT INTO pull_requests (repo, number, head_branch, base_branch, changed_files, additions, deletions, commits,
  created_at, ready_at, merged_at, closed_at, is_draft, labels, fetched_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (repo, number) DO UPDATE SET head_branch = excluded.head_branch, base_branch = excluded.base_branch,
  changed_files = excluded.changed_files, additions = excluded.additions, deletions = excluded.deletions,
  commits = excluded.commits, created_at = excluded.created_at, ready_at = excluded.ready_at,
  merged_at = excluded.merged_at, closed_at = excluded.closed_at, is_draft = excluded.is_draft,
  labels = excluded.labels, fetched_at = excluded.fetched_at
"""


class PullRequestError(RuntimeError):
    """A pull request fetch failed; the message names the repository and the reason."""


def refresh(conn: sqlite3.Connection, repos: Sequence[str], days: int, now: datetime, run: Runner) -> int:
    """Upsert the pull requests updated since the last fetch of each repository; returns rows upserted."""
    total = 0
    for repo in repos:
        key = f"prs:{repo}"
        row = conn.execute("SELECT value FROM meta WHERE key = ?", (key,)).fetchone()
        for start, end in _windows(row[0] if row else None, days, now):
            total += _fetch_window(conn, repo, start, end, now, run)
        with conn:
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)", (key, now.isoformat()))
    return total


def _windows(last_fetch: str | None, days: int, now: datetime) -> Iterator[tuple[datetime, datetime]]:
    if last_fetch is not None:
        yield datetime.fromisoformat(last_fetch) - OVERLAP, now
        return
    start = now - timedelta(days=days)
    while start < now:
        end = min(start + WINDOW, now)
        yield start, end
        start = end


def _fetch_window(conn: sqlite3.Connection, repo: str, start: datetime, end: datetime,
                  now: datetime, run: Runner) -> int:
    """Upsert every pull request of the window; a window the search would truncate is split in halves."""
    search = f"repo:{repo} is:pr updated:{_stamp(start)}..{_stamp(end)}"
    page = _search(repo, search, None, run)
    seconds = int((end - start).total_seconds())
    if _issue_count(page) > SEARCH_LIMIT and seconds >= MIN_SPLIT_SECONDS:
        middle = start + timedelta(seconds=seconds // 2)
        return _fetch_window(conn, repo, start, middle, now, run) + _fetch_window(conn, repo, middle, end, now, run)
    count = 0
    while True:
        rows = _rows(repo, page["nodes"], now)
        with conn:
            conn.executemany(UPSERT, rows)
        count += len(rows)
        if not page["pageInfo"]["hasNextPage"]:
            return count
        page = _search(repo, search, _end_cursor(repo, page), run)


def _issue_count(page: dict) -> int:
    count = page.get("issueCount")
    return count if isinstance(count, int) and not isinstance(count, bool) else 0


def _end_cursor(repo: str, page: dict) -> str:
    cursor = page["pageInfo"].get("endCursor")
    if not isinstance(cursor, str) or not cursor:
        raise PullRequestError(f"{repo}: gh reported another page without an endCursor")
    return cursor


def _search(repo: str, search: str, cursor: str | None, run: Runner) -> dict:
    argv = ["gh", "api", "graphql", "-f", f"query={QUERY}", "-f", f"q={search}"]
    if cursor:
        argv += ["-f", f"cursor={cursor}"]
    done = run(argv)
    if done.returncode != 0:
        reason = (done.stderr or "").strip().splitlines()
        raise PullRequestError(f"{repo}: {reason[0] if reason else f'gh exited {done.returncode}'}")
    try:
        page = json.loads(done.stdout)["data"]["search"]
        if not isinstance(page["nodes"], list) or not isinstance(page["pageInfo"]["hasNextPage"], bool):
            raise TypeError("search page shape")
    except (ValueError, KeyError, TypeError) as error:
        raise PullRequestError(f"{repo}: unexpected gh response ({error.__class__.__name__})") from error
    return page


def _stamp(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).strftime(TIME_FORMAT)


def _rows(repo: str, nodes: list, now: datetime) -> list[tuple]:
    try:
        return [row for node in nodes if (row := _row(repo, node, now))]
    except (KeyError, TypeError, AttributeError) as error:
        raise PullRequestError(f"{repo}: unexpected pull request shape ({error.__class__.__name__})") from error


def _row(repo: str, node: dict, now: datetime) -> tuple | None:
    if not isinstance(node.get("number"), int):
        return None
    ready = _ready_at(node)
    labels = sorted(label["name"] for label in node["labels"]["nodes"])
    return (repo, node["number"], node["headRefName"], node["baseRefName"], node["changedFiles"],
            node["additions"], node["deletions"], node["commits"]["totalCount"], node["createdAt"], ready,
            node["mergedAt"], node["closedAt"], int(node["isDraft"]), json.dumps(labels), now.isoformat())


def _ready_at(node: dict) -> str | None:
    events = node["timelineItems"]["nodes"]
    if events:
        return events[0]["createdAt"]
    return None if node["isDraft"] else node["createdAt"]
