"""The example collectors, run for real with `gh` and the Notion API faked."""

import http.server
import json
import os
import subprocess
import sys
import threading
from pathlib import Path

import pytest

from sakura.digest import intake

EXAMPLES = Path(__file__).resolve().parents[1] / "examples" / "digest"
GITHUB, NOTION = EXAMPLES / "github.sh", EXAMPLES / "notion.py"
SINCE = "2026-11-16T06:30:00+00:00"
GH_TOKEN, NOTION_TOKEN = "ghp_SECRETSECRETSECRET", "secret_notion_token_value"
FAKE_GH = f"""#!{sys.executable}
import json, os, sys
argv = " ".join(sys.argv[1:])
with open(os.environ["FAKE_GH_LOG"], "a") as log:
    log.write(argv + "\\n")
if os.environ.get("FAKE_GH_FAIL") or any(part and part in argv for part in os.environ.get("FAKE_GH_FAIL_ON", "").split("|")):
    sys.stderr.write("gh: HTTP 401: bad credentials\\n")
    sys.exit(1)
for entry in json.load(open(os.environ["FAKE_GH_RESPONSES"])):
    if all(part in argv for part in entry["match"]):
        sys.stdout.write(entry["stdout"])
        sys.exit(0)
sys.stderr.write("fake gh: no response for " + argv + "\\n")
sys.exit(1)
"""


def _run(*command: str, cwd: Path, **extra) -> subprocess.CompletedProcess:
    environment = {**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null", **extra}
    return subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True, check=False)


# number, title, url, base, head, head commit, failing checks, mergeable, mergeStateStatus, from a fork
PULL_REQUESTS = [(1, "clean", "main", "MERGEABLE", "CLEAN", "false"), (2, "behind", "main", "MERGEABLE", "BEHIND", "false"),
                 (3, "conflict", "main", "CONFLICTING", "DIRTY", "false"), (4, "computing", "main", "UNKNOWN", "UNKNOWN", "false"),
                 (5, "fork-behind", "main", "MERGEABLE", "BEHIND", "true")]


def _oid(number: int) -> str:
    return f"{number}" * 40


def _states() -> str:
    return "".join(f"{number}\tPR {name}\thttps://example.org/pr/{number}\t{base}\t{name}\t{_oid(number)}\t0\t{mergeable}\t{status}\t{fork}\n"
                   for number, name, base, mergeable, status, fork in PULL_REQUESTS)


def _fake_gh(tmp_path: Path, responses: list[dict]) -> dict:
    folder = tmp_path / "fakebin"
    folder.mkdir()
    (folder / "gh").write_text(FAKE_GH)
    (folder / "gh").chmod(0o755)
    (folder / "git").write_text(f"#!/bin/sh\necho called >> {tmp_path / 'git-was-called'}\nexit 99\n")
    (folder / "git").chmod(0o755)
    (tmp_path / "responses.json").write_text(json.dumps(responses))
    return {"PATH": f"{folder}{os.pathsep}{os.environ['PATH']}", "FAKE_GH_RESPONSES": str(tmp_path / "responses.json"),
            "FAKE_GH_LOG": str(tmp_path / "gh.log")}


def _item(identity: str, title: str) -> str:
    return json.dumps({"id": identity, "title": title, "url": "https://example.org/x", "at": "2026-11-17T05:00:00Z",
                       "text": "text"}) + "\n"


def _folder(parent: Path, name: str) -> Path:
    folder = parent / name
    folder.mkdir()
    return folder


def _github(tmp_path: Path, cursor: str = "", fail: bool = False, fail_on: str = ""):
    responses = [
        {"match": ["--author @me", "mergeStateStatus"], "stdout": _states()},
        {"match": ["compare/main...behind"], "stdout": "3\n"},
        {"match": ["--author @me", "--json number --jq"], "stdout": "2\n"},
        {"match": ["pr view 2"], "stdout": _item("review-1", "Review APPROVED by someone on PR #2")},
        {"match": ["pr list", "--assignee @me"], "stdout": _item("assigned-pr-9", "Assigned to you: #9")},
        {"match": ["issue list", "--assignee @me"], "stdout": ""},
        {"match": ["pr list", "--state all"], "stdout": _item("pr-4-x", "PR #4: change")},
        {"match": ["issue list", "--state all"], "stdout": _item("issue-5-x", "Issue #5: bug")},
        {"match": ["/commits?sha=main"], "stdout": _item("commit-abc", "A commit")},
        {"match": ["check-runs"], "stdout": _item("ci-abc", "Failing checks on main")}]
    fake = _fake_gh(tmp_path, responses)
    (tmp_path / "home").mkdir(exist_ok=True)
    return _run("bash", str(GITHUB), "demo/app", "main", cwd=tmp_path, HOME=str(tmp_path / "home"),
                SAKURA_DIGEST_SINCE=SINCE, SAKURA_DIGEST_CURSOR=cursor, GH_TOKEN=GH_TOKEN, **fake,
                **({"FAKE_GH_FAIL": "1"} if fail else {}), FAKE_GH_FAIL_ON=fail_on)


def test_GithubExample_FakeGh_PrintsProtocolLines(tmp_path):
    done = _github(tmp_path)
    assert done.returncode == 0, done.stderr
    items, cursor, skipped = intake.parse_output(done.stdout.encode())
    assert skipped == 0
    assert {"pr-4-x", "issue-5-x", "assigned-pr-9", "review-1", "commit-abc", "ci-abc"} <= {item.id for item in items}
    assert cursor.startswith("1=clean:0:")
    calls = (tmp_path / "gh.log").read_text()
    assert "demo/app" in calls and "updated:>=2026-11-16T06:30:00Z" in calls


def test_GithubExample_MergeState_CleanBehindConflict(tmp_path):
    done = _github(tmp_path)
    items, cursor, _ = intake.parse_output(done.stdout.encode())
    states = {item.id.split("-")[1]: item for item in items if item.id.startswith("state-")}
    assert set(states) == {"2", "3", "5"}  # the clean pull request is silent the first time, the unknown one always
    assert "is behind with main" in states["2"].title and "Behind by 3 commits" in states["2"].text
    assert "is conflict with main" in states["3"].title
    assert cursor.split(";")[:3] == [f"1=clean:0:{_oid(1)[:12]}", f"2=behind:0:{_oid(2)[:12]}",
                                     f"3=conflict:0:{_oid(3)[:12]}"] and "4=" not in cursor
    calls = (tmp_path / "gh.log").read_text().splitlines()
    # the count is asked for a branch of this repository, never for a fork's (its branch is not here)
    assert [call for call in calls if "compare/" in call] == ["api repos/demo/app/compare/main...behind --jq .behind_by"]
    assert "Behind by" not in states["5"].text
    # The same states again say nothing; a pull request that changed state is reported, even to say it is clean.
    again = _github(_folder(tmp_path, "again"), cursor)
    assert not [item for item in intake.parse_output(again.stdout.encode())[0] if item.id.startswith("state-")]
    changed = _github(_folder(tmp_path, "changed"), cursor.replace("1=clean:0:", "1=behind:0:"))
    assert [item.id for item in intake.parse_output(changed.stdout.encode())[0] if item.id.startswith("state-")] == [
        f"state-1-{_oid(1)[:12]}"]


def test_GithubExample_GithubStillComputing_ReportsNothingAndKeepsWhatItKnew(tmp_path):
    known = f"4=behind:0:{_oid(4)[:12]};"
    done = _github(tmp_path, known)
    items, cursor, _ = intake.parse_output(done.stdout.encode())
    assert not [item for item in items if item.id.startswith("state-4-")]
    assert known in cursor  # the next run compares with what was last known


def test_GithubExample_NoCloneNoGitNoCache(tmp_path):
    done = _github(tmp_path)
    assert done.returncode == 0 and not (tmp_path / "git-was-called").exists()
    assert not (tmp_path / "cache").exists() and not (tmp_path / "home" / ".cache").exists()
    source = GITHUB.read_text() + (EXAMPLES / "README.md").read_text()
    assert "git clone" not in source and "merge-tree" not in source and "2.38" not in source


class _Notion:
    """A local Notion API: records what it was asked and answers from a table."""

    def __init__(self, blocks: list[dict], tasks: list[dict], status: int = 200, echo_token: bool = False,
                 task_pages: list[list[dict]] | None = None, endless: str = "") -> None:
        self.seen: list[dict] = []
        pages = task_pages if task_pages is not None else [tasks]
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def _answer(self, body: dict) -> None:
                length = int(self.headers.get("Content-Length") or 0)
                outer.seen.append({"method": self.command, "path": self.path, "auth": self.headers.get("Authorization"),
                                   "version": self.headers.get("Notion-Version"),
                                   "body": json.loads(self.rfile.read(length) or b"null")})
                if status != 200:
                    body = {"message": f"unauthorized {NOTION_TOKEN}" if echo_token else "unauthorized"}
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                more = endless == "blocks"
                self._answer({"results": blocks, "has_more": more, "next_cursor": "more" if more else None})

            def do_POST(self):
                number = sum(1 for call in outer.seen if call["method"] == "POST")
                index = min(number, len(pages) - 1)
                more = endless == "tasks" or number + 1 < len(pages)
                self._answer({"results": pages[index], "has_more": more, "next_cursor": f"page-{number + 1}" if more else None})

            def log_message(self, *args):
                pass

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.httpd.daemon_threads = True
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.httpd.server_address[1]}"

    def close(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()


def _block(identifier: str, created: str, *parts: str) -> dict:
    return {"id": identifier, "type": "paragraph", "created_time": created,
            "paragraph": {"rich_text": [{"plain_text": part} for part in parts]}}


def _task(identifier: str, edited: str, title: str) -> dict:
    return {"id": identifier, "url": f"https://www.notion.so/{identifier}", "last_edited_time": edited,
            "properties": {"Name": {"type": "title", "title": [{"plain_text": title}]},
                           "Status": {"type": "status", "status": {"name": "In progress"}}}}


def _notion(api: str, **extra) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(NOTION), "--page", "PAGE", "--database", "DB", "--user-id", "USER",
                           "--api-base", api, "--token-env", "DIGEST_TEST_TOKEN"], capture_output=True, text=True,
                          check=False, env={**os.environ, "SAKURA_DIGEST_SINCE": SINCE, "SAKURA_DIGEST_CURSOR": "",
                                            "DIGEST_TEST_TOKEN": NOTION_TOKEN, **extra})


def test_NotionExample_FakeApi_PageMessagesAndTasks():
    api = _Notion([_block("b-old", "2026-11-15T10:00:00.000Z", "too old"),
                   _block("b-new", "2026-11-17T05:00:00.000Z", "Please check the ", "release")],
                  [_task("t1", "2026-11-17T06:00:00.000Z", "Write the notes")])
    try:
        done = _notion(api.base)
    finally:
        api.close()
    assert done.returncode == 0, done.stderr
    items, cursor, skipped = intake.parse_output(done.stdout.encode())
    assert skipped == 0 and [item.id for item in items] == ["message-b-new", "task-t1-2026-11-17T06:00:00.000Z"]
    assert items[0].text == "Please check the release" and items[1].title == "Task: Write the notes"
    assert "In progress" in items[1].text and cursor == "2026-11-17T06:00:00.000Z"
    page, query = api.seen
    assert page["path"].startswith("/v1/blocks/PAGE/children") and page["auth"] == f"Bearer {NOTION_TOKEN}"
    assert page["version"] and query["path"] == "/v1/databases/DB/query"
    assert {"property": "Assignee", "people": {"contains": "USER"}} in query["body"]["filter"]["and"]


def test_Examples_TokenNeverPrinted(tmp_path):
    api = _Notion([], [], status=401, echo_token=True)
    try:
        refused = _notion(api.base)
    finally:
        api.close()
    assert refused.returncode == 1 and "401" in refused.stderr
    missing = _notion("http://127.0.0.1:1", DIGEST_TEST_TOKEN="")
    assert missing.returncode == 1 and "DIGEST_TEST_TOKEN" in missing.stderr
    github = _github(tmp_path, fail=True)
    assert github.returncode != 0
    for done in (refused, missing, github, _github(_folder(tmp_path, "ok"))):
        assert NOTION_TOKEN not in done.stdout + done.stderr and GH_TOKEN not in done.stdout + done.stderr
    assert "***" in refused.stderr or "unauthorized" in refused.stderr


@pytest.mark.parametrize("failing", ["mergeStateStatus", "--json number --jq", "compare/", "check-runs", "/commits?sha",
                                     "pr view 2", "--state all", "--assignee @me"])
def test_GithubExample_AnyQueryFailing_NoCursorAndNonZero(tmp_path, failing):
    done = _github(tmp_path, fail_on=failing)
    assert done.returncode != 0, failing
    assert '"cursor"' not in done.stdout


def test_NotionExample_TaskPagination_FollowsEveryPage():
    api = _Notion([], [], task_pages=[[_task("t1", "2026-11-17T06:00:00.000Z", "First")],
                                      [_task("t2", "2026-11-17T05:00:00.000Z", "Second")],
                                      [_task("t3", "2026-11-17T04:00:00.000Z", "Third")]])
    try:
        done = _notion(api.base)
    finally:
        api.close()
    assert done.returncode == 0, done.stderr
    items, cursor, skipped = intake.parse_output(done.stdout.encode())
    assert [item.title for item in items] == ["Task: First", "Task: Second", "Task: Third"] and skipped == 0
    posts = [call["body"] for call in api.seen if call["method"] == "POST"]
    assert [body.get("start_cursor") for body in posts] == [None, "page-1", "page-2"]
    assert cursor == "2026-11-17T06:00:00.000Z"


@pytest.mark.parametrize("endless", ["tasks", "blocks"])
def test_NotionExample_MoreThanItWillPage_FailsWithoutACursorOrAnItem(endless):
    api = _Notion([_block("b1", "2026-11-17T05:00:00.000Z", "hello")], [_task("t1", "2026-11-17T06:00:00.000Z", "x")],
                  endless=endless)
    try:
        done = _notion(api.base)
    finally:
        api.close()
    assert done.returncode != 0 and done.stdout == "" and "pages" in done.stderr
