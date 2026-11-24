#!/usr/bin/env python3
"""A digest collector for Notion: new messages on one page used as a channel, and tasks assigned to you that changed.

Usage:
    notion.py --page PAGE_ID --database DATABASE_ID --user-id USER_ID [--assignee-property Assignee]
              [--token-env NAME | --keychain-item NAME] [--api-base URL]

The token is read when the program runs, and only from where you say: the environment variable `--token-env` names
(default NOTION_TOKEN), or the macOS Keychain item `--keychain-item` names (a generic password). It is sent to Notion
and nowhere else, never printed, and removed from every error message. Either id may be left out to skip that part.

It prints one JSON object per line as the digest protocol asks (see README.md), for what is newer than
SAKURA_DIGEST_SINCE or the cursor, and a last line with the newest time it saw. It follows every page Notion has (up to
ten for each of the two parts); when there are more, or any call fails, it prints nothing and no cursor, and exits with
status 1, so the next run looks again from the same place. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

API_BASE = "https://api.notion.com"
NOTION_VERSION = "2022-06-28"
DEFAULT_TOKEN_VARIABLE = "NOTION_TOKEN"
MAX_PAGES = 10
TASKS_PER_PAGE = 50
TIMEOUT_SECONDS = 30
TITLE_LENGTH = 100


class CollectorError(Exception):
    """A failure the person should read; its text never holds the token."""


def main(argv: list[str]) -> int:
    args = _parser().parse_args(argv)
    try:
        token = _token(args)
        notion = _Notion(args.api_base, token)
        since = _newer_of(os.environ.get("SAKURA_DIGEST_SINCE", ""), os.environ.get("SAKURA_DIGEST_CURSOR", ""))
        newest = since
        for item in [*_messages(notion, args, since), *_tasks(notion, args, since)]:
            print(json.dumps(item))
            newest = max(newest, item["at"], key=_moment)
    except CollectorError as error:
        print(f"notion: {error}", file=sys.stderr)
        return 1
    if newest:
        print(json.dumps({"cursor": newest}))
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="notion.py", description=__doc__.splitlines()[0])
    parser.add_argument("--page", default="", help="the page whose new blocks are messages")
    parser.add_argument("--database", default="", help="the database of tasks")
    parser.add_argument("--user-id", default="", help="your Notion user id, for the tasks assigned to you")
    parser.add_argument("--assignee-property", default="Assignee")
    parser.add_argument("--token-env", default=DEFAULT_TOKEN_VARIABLE)
    parser.add_argument("--keychain-item", default="")
    parser.add_argument("--api-base", default=API_BASE)
    return parser


def _token(args: argparse.Namespace) -> str:
    if args.keychain_item:
        done = subprocess.run(["/usr/bin/security", "find-generic-password", "-s", args.keychain_item, "-w"],
                              capture_output=True, text=True, check=False)
        token = done.stdout.strip()
        if done.returncode != 0 or not token:
            raise CollectorError(f"no Keychain item named {args.keychain_item!r}")
        return token
    token = os.environ.get(args.token_env, "").strip()
    if not token:
        raise CollectorError(f"set the environment variable {args.token_env} to your Notion token")
    return token


class _Notion:
    """The few calls of the Notion REST API the collector makes."""

    def __init__(self, base: str, token: str) -> None:
        self._base, self._token = base.rstrip("/"), token

    def get(self, path: str) -> dict:
        return self._call("GET", path, None)

    def post(self, path: str, body: dict) -> dict:
        return self._call("POST", path, body)

    def _call(self, method: str, path: str, body: dict | None) -> dict:
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(self._base + path, data=data, method=method, headers={
            "Authorization": f"Bearer {self._token}", "Notion-Version": NOTION_VERSION,
            "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise CollectorError(f"Notion answered {error.code} for {path.split('?')[0]}: {self._reason(error)}") from None
        except (urllib.error.URLError, OSError, ValueError) as error:
            raise CollectorError(self._clean(f"cannot reach Notion: {error}")) from None

    def _reason(self, error: urllib.error.HTTPError) -> str:
        try:
            return self._clean(str(json.load(error).get("message", "")))[:200]
        except (ValueError, OSError):
            return ""

    def _clean(self, text: str) -> str:
        return text.replace(self._token, "***")


def _messages(notion: _Notion, args: argparse.Namespace, since: str) -> list[dict]:
    if not args.page:
        return []
    found, cursor = [], ""
    for _ in range(MAX_PAGES):
        page = notion.get(f"/v1/blocks/{args.page}/children?page_size=100" + (f"&start_cursor={cursor}" if cursor else ""))
        found += [_message(block) for block in page.get("results", []) if _is_newer(block.get("created_time", ""), since)]
        cursor = page.get("next_cursor") or ""
        if not page.get("has_more") or not cursor:
            return [item for item in found if item["text"]]
    raise CollectorError(f"the page has more than {MAX_PAGES} pages of blocks; nothing was collected, so the next run "
                         "looks again from the same place")


def _message(block: dict) -> dict:
    text = "".join(part.get("plain_text", "") for part in block.get(block.get("type", ""), {}).get("rich_text", []))
    identifier = block["id"]
    return {"id": f"message-{identifier}", "title": "Channel: " + " ".join(text.split())[:TITLE_LENGTH],
            "url": "https://www.notion.so/" + identifier.replace("-", ""), "at": block["created_time"], "text": text}


def _tasks(notion: _Notion, args: argparse.Namespace, since: str) -> list[dict]:
    if not args.database or not args.user_id:
        return []
    query = {"page_size": TASKS_PER_PAGE, "filter": {"and": [
        {"property": args.assignee_property, "people": {"contains": args.user_id}},
        {"timestamp": "last_edited_time", "last_edited_time": {"on_or_after": since or "1970-01-01T00:00:00Z"}}]}}
    found = []
    for _ in range(MAX_PAGES):
        answer = notion.post(f"/v1/databases/{args.database}/query", query)
        found += [_task(task) for task in answer.get("results", [])]
        if not answer.get("has_more") or not answer.get("next_cursor"):
            return found
        query = {**query, "start_cursor": answer["next_cursor"]}
    raise CollectorError(f"the database has more than {MAX_PAGES} pages of changed tasks; nothing was collected, so the "
                         "next run looks again from the same place")


def _task(task: dict) -> dict:
    properties = task.get("properties", {})
    title = next(("".join(part.get("plain_text", "") for part in value.get("title", []))
                  for value in properties.values() if value.get("type") == "title"), "")
    states = [value[value["type"]]["name"] for value in properties.values()
              if value.get("type") in ("status", "select") and value.get(value["type"])]
    return {"id": f"task-{task['id']}-{task['last_edited_time']}", "title": "Task: " + (title or "untitled")[:TITLE_LENGTH],
            "url": task.get("url", ""), "at": task["last_edited_time"], "text": "Changed task. " + ", ".join(states)}


def _moment(value: str) -> datetime:
    if not value:
        return datetime.min.replace(tzinfo=timezone.utc)
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _is_newer(value: str, since: str) -> bool:
    return bool(value) and _moment(value) > _moment(since)


def _newer_of(first: str, second: str) -> str:
    return max(first, second, key=_moment)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
