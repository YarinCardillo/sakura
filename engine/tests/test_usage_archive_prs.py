import json
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura.usage.archive import prs, store

PAGE = json.loads((Path(__file__).parent / "fixtures" / "usage" / "archive" / "gh_search_page.json").read_text())
NOW = datetime(2026, 10, 27, 12, 0, tzinfo=timezone.utc)
REPO = "acme/widgets"


class FakeGh:
    def __init__(self, pages=None, returncode=0, stderr=""):
        self.pages = list(pages or [PAGE])
        self.returncode, self.stderr = returncode, stderr
        self.calls: list[list[str]] = []

    def __call__(self, argv):
        self.calls.append(argv)
        if self.returncode:
            return subprocess.CompletedProcess(argv, self.returncode, "", self.stderr)
        page = self.pages.pop(0) if len(self.pages) > 1 else self.pages[0]
        return subprocess.CompletedProcess(argv, 0, json.dumps(page), "")

    def variable(self, call, name):
        prefix = f"{name}="
        return next((arg[len(prefix):] for arg in call if arg.startswith(prefix)), None)


def conn_for(tmp_path):
    return store.open_db(tmp_path / "usage.db")


def page_with(has_next, cursor):
    page = json.loads(json.dumps(PAGE))
    page["data"]["search"]["pageInfo"] = {"hasNextPage": has_next, "endCursor": cursor}
    return page


def test_refresh_upserts_rows_and_ready_time(tmp_path):
    conn = conn_for(tmp_path)

    count = prs.refresh(conn, [REPO], 7, NOW, FakeGh())

    rows = {row[1]: row for row in conn.execute(
        "SELECT repo, number, head_branch, base_branch, changed_files, additions, deletions, commits,"
        " created_at, ready_at, merged_at, closed_at, is_draft, labels FROM pull_requests")}
    assert count == 3 and len(rows) == 3
    assert rows[11][0:8] == (REPO, 11, "feature/x", "main", 4, 120, 30, 3)
    assert rows[11][9] == "2026-10-20T09:00:00Z"
    assert rows[11][10] == "2026-10-21T09:00:00Z" and rows[11][12] == 0
    assert rows[11][13] == '["alpha", "zeta"]'
    assert rows[12][9] == "2026-10-22T12:00:00Z"
    assert rows[13][9] is None and rows[13][12] == 1


def test_first_fetch_uses_weekly_windows(tmp_path):
    fake = FakeGh()

    prs.refresh(conn_for(tmp_path), [REPO], 15, NOW, fake)

    queries = [fake.variable(call, "q") for call in fake.calls]
    start = NOW - timedelta(days=15)
    assert len(queries) == 3
    assert queries[0].startswith(f"repo:{REPO} is:pr updated:{start:%Y-%m-%dT%H:%M:%SZ}..")
    assert queries[0].endswith(f"{start + timedelta(days=7):%Y-%m-%dT%H:%M:%SZ}")
    assert queries[2].endswith(f"{NOW:%Y-%m-%dT%H:%M:%SZ}")


def test_later_fetch_uses_one_window_from_last_fetch(tmp_path):
    conn = conn_for(tmp_path)
    prs.refresh(conn, [REPO], 7, NOW, FakeGh())
    later = NOW + timedelta(hours=2)
    fake = FakeGh()

    prs.refresh(conn, [REPO], 7, later, fake)

    assert len(fake.calls) == 1
    window = fake.variable(fake.calls[0], "q").split("updated:")[1]
    assert window == f"{NOW - timedelta(hours=1):%Y-%m-%dT%H:%M:%SZ}..{later:%Y-%m-%dT%H:%M:%SZ}"


def test_pagination_follows_end_cursor(tmp_path):
    fake = FakeGh([page_with(True, "CUR1"), page_with(False, "CUR2")])

    prs.refresh(conn_for(tmp_path), [REPO], 7, NOW, fake)

    assert [fake.variable(call, "cursor") for call in fake.calls] == [None, "CUR1"]


def test_search_pages_hold_at_most_fifty_pull_requests(tmp_path):
    # GitHub answers HTTP 502 to a page of 100 pull requests with their timeline items.
    fake = FakeGh()

    prs.refresh(conn_for(tmp_path), [REPO], 7, NOW, fake)

    query = fake.variable(fake.calls[0], "query")
    assert "first: 50," in query and "first: 100" not in query


def test_gh_failure_raises_and_keeps_meta(tmp_path):
    conn = conn_for(tmp_path)

    with pytest.raises(prs.PullRequestError, match="acme/widgets: gh: not logged in"):
        prs.refresh(conn, [REPO], 7, NOW, FakeGh(returncode=4, stderr="gh: not logged in\nmore detail"))

    assert conn.execute("SELECT COUNT(*) FROM meta WHERE key LIKE 'prs:%'").fetchone()[0] == 0


def test_next_page_without_end_cursor_raises_instead_of_looping(tmp_path):
    fake = FakeGh([page_with(True, None)])

    with pytest.raises(prs.PullRequestError, match="acme/widgets.*endCursor"):
        prs.refresh(conn_for(tmp_path), [REPO], 7, NOW, fake)

    assert len(fake.calls) == 1


def window_of(call):
    start, end = FakeGh().variable(call, "q").split("updated:")[1].split("..")
    parse = lambda text: datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    return parse(start), parse(end)


def test_window_over_the_search_limit_is_split_in_halves(tmp_path):
    seen = []

    def runner(argv):
        seen.append(argv)
        start, end = window_of(argv)
        page = json.loads(json.dumps(PAGE))
        page["data"]["search"]["issueCount"] = 1500 if end - start > timedelta(days=4) else 40
        return subprocess.CompletedProcess(argv, 0, json.dumps(page), "")

    conn = conn_for(tmp_path)
    prs.refresh(conn, [REPO], 7, NOW, runner)

    windows = [window_of(call) for call in seen]
    start = NOW - timedelta(days=7)
    middle = start + timedelta(days=3, hours=12)
    assert windows == [(start, NOW), (start, middle), (middle, NOW)]
    assert conn.execute("SELECT COUNT(*) FROM pull_requests").fetchone()[0] == 3


def test_split_recurses_until_each_window_fits(tmp_path):
    def runner(argv):
        start, end = window_of(argv)
        page = json.loads(json.dumps(PAGE))
        page["data"]["search"]["issueCount"] = 1500 if end - start > timedelta(days=1) else 10
        return subprocess.CompletedProcess(argv, 0, json.dumps(page), "")

    fake_calls = []
    prs.refresh(conn_for(tmp_path), [REPO], 7, NOW, lambda argv: fake_calls.append(argv) or runner(argv))

    small = [call for call in fake_calls if window_of(call)[1] - window_of(call)[0] <= timedelta(days=1)]
    assert len(small) == 8
    assert all(window_of(call)[1] - window_of(call)[0] <= timedelta(days=1) for call in small)


def test_invalid_json_raises(tmp_path):
    def broken(argv):
        return subprocess.CompletedProcess(argv, 0, "not json", "")

    with pytest.raises(prs.PullRequestError, match="acme/widgets"):
        prs.refresh(conn_for(tmp_path), [REPO], 7, NOW, broken)


def add_pr(conn, repo, number, branch, created, merged=None, closed=None):
    conn.execute(
        "INSERT INTO pull_requests VALUES (?, ?, ?, 'main', 1, 1, 1, 1, ?, NULL, ?, ?, 0, '[]', 't')",
        (repo, number, branch, created, merged, closed))


def add_turn(conn, key, ts, cwd, branch="feature/x"):
    conn.execute("INSERT INTO turns VALUES ('claude', ?, 'a', 's', NULL, ?, ?, ?, 'm', 1, 0, 0, 1, NULL)",
                 (key, ts, cwd, branch))


def links(conn):
    return {row[0]: (row[1], row[2]) for row in conn.execute("SELECT event_key, repo, number FROM pr_turns")}


def test_pr_turns_view_links_by_branch_and_repo(tmp_path):
    conn = conn_for(tmp_path)
    prs.refresh(conn, [REPO], 7, NOW, FakeGh())
    conn.executemany("INSERT INTO cwds VALUES (?, ?, 't')", [("/w/widgets", "widgets"), ("/w/gadgets", "gadgets")])
    add_turn(conn, "same-repo", "2026-10-21T00:00:00+00:00", "/w/widgets")
    add_turn(conn, "other-repo", "2026-10-21T00:00:00+00:00", "/w/gadgets")
    conn.commit()

    assert links(conn) == {"same-repo": (REPO, 11)}


def test_pr_turns_branch_reused_by_sequential_prs_links_each_turn_once(tmp_path):
    conn = conn_for(tmp_path)
    add_pr(conn, REPO, 1, "feature/x", "2026-10-01T09:00:00Z", merged="2026-10-05T09:00:00Z")
    add_pr(conn, REPO, 2, "feature/x", "2026-10-10T09:00:00Z", merged="2026-10-12T09:00:00Z")
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    add_turn(conn, "first", "2026-10-03T10:00:00.5+00:00", "/w/widgets")
    add_turn(conn, "second", "2026-10-08T10:00:00+00:00", "/w/widgets")
    add_turn(conn, "after-close", "2026-10-15T10:00:00+00:00", "/w/widgets")

    assert links(conn) == {"first": (REPO, 1), "second": (REPO, 2)}


def test_pr_turns_open_pr_window_is_open_ended(tmp_path):
    conn = conn_for(tmp_path)
    add_pr(conn, REPO, 1, "feature/x", "2026-10-10T09:00:00Z")
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    add_turn(conn, "late", "2030-01-01T00:00:00+00:00", "/w/widgets")

    assert links(conn) == {"late": (REPO, 1)}


def test_pr_turns_unresolved_cwd_needs_the_repo_name_as_a_path_component(tmp_path):
    conn = conn_for(tmp_path)
    add_pr(conn, "acme/widgets", 1, "feature/x", "2026-10-10T09:00:00Z")
    add_pr(conn, "acme/gadgets", 1, "feature/x", "2026-10-10T09:00:00Z")
    add_turn(conn, "lane", "2026-10-11T00:00:00+00:00", "/x/widgets/lane-a")
    add_turn(conn, "substring", "2026-10-11T00:00:00+00:00", "/x/widgets-old/lane-a")
    add_turn(conn, "no-cwd", "2026-10-11T00:00:00+00:00", None)

    assert links(conn) == {"lane": ("acme/widgets", 1)}
