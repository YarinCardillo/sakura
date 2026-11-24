"""Provider requests for scoped connectors. Session data lives only for the current operation."""

from __future__ import annotations

import json
import re
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from urllib.parse import quote, urlencode

from sakura import programs
from sakura.connectors import kinds
from sakura.digest import schema, supervise

GITHUB_REPO = re.compile(r"(?:(?P<host>[a-zA-Z0-9][a-zA-Z0-9.-]*)/)?(?P<owner>[a-zA-Z0-9_.-]+)/(?P<repo>[a-zA-Z0-9_.-]+)")
GITHUB_WRITE_SCOPES = {"repo", "public_repo"}
GITHUB_CANONICAL = re.compile(r"([a-zA-Z0-9][a-zA-Z0-9.-]*):([1-9][0-9]*)")
PROVIDER_ID = re.compile(r"[A-Za-z0-9-]{1,120}")
PAGE_SIZE = 50
NOTION_BASE = "https://api.notion.com/v1"
LINEAR_FIELDS = "id identifier title url updatedAt team { id } assignee { id }"


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def http(method: str, url: str, headers: dict, data: dict | None, timeout: int) -> object:
    """Fetch bounded JSON without forwarding a provider header to a redirect."""
    body = json.dumps(data).encode() if data is not None else None
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.build_opener(_NoRedirect).open(request, timeout=timeout) as response:
            raw = response.read(schema.MAX_SOURCE_BYTES + 1)
        if len(raw) > schema.MAX_SOURCE_BYTES:
            raise ValueError
        return json.loads(raw)
    except urllib.error.HTTPError as error:
        code = {401: "reconnect_required", 403: "scope_unavailable", 404: "scope_unavailable"}.get(
            error.code, "connector_unavailable")
        raise kinds.ConnectorError(code) from None
    except (OSError, ValueError, RecursionError):
        raise kinds.ConnectorError("connector_unavailable") from None


@dataclass(frozen=True)
class _Session:
    call: Callable
    label: str = ""
    recipient: str = ""


def inspect(kind: str, selectors: dict, timeout: int, ports: kinds.Ports) -> kinds.Access:
    """Resolve identity and verify that the integration can access its selected scope."""
    try:
        if kind == kinds.GITHUB:
            return _github_access(selectors, timeout, ports)
        if kind == kinds.LINEAR:
            return _linear_access(selectors, timeout, ports)
        return _notion_access(selectors, timeout, ports)
    except (AttributeError, IndexError, KeyError, TypeError, ValueError) as error:
        if isinstance(error, kinds.ConnectorError):
            raise
        raise kinds.ConnectorError("scope_unavailable") from None


def _github_access(selectors: dict, timeout: int, ports: kinds.Ports) -> kinds.Access:
    if saved := GITHUB_CANONICAL.fullmatch(selectors["repo"]):
        host, expected = saved.groups()
        path = f"repositories/{expected}"
    elif requested := GITHUB_REPO.fullmatch(selectors["repo"]):
        host, owner, name = requested.groups()
        host = (host or "github.com").lower()
        path = f"repos/{_path_segment(owner)}/{_path_segment(name)}"
    else:
        raise kinds.ConnectorError("scope_unavailable")
    call = _github_session(host, timeout, ports)
    user, scopes = call("GET", "user", None, timeout, include_scopes=True)
    repo = call("GET", path, None, timeout)
    principal = f"{host}:{_numeric(user['id'])}"
    resource = f"{host}:{_numeric(repo['id'])}"
    label = repo["full_name"]
    if "branch" in selectors:
        call("GET", f"{_github_path(label)}/branches/{quote(selectors['branch'], safe='')}", None, timeout)
    may_comment = repo.get("permissions", {}).get("pull") is True and bool(GITHUB_WRITE_SCOPES & scopes)
    if repo.get("private") is not False:
        may_comment = may_comment and "repo" in scopes
    operations = ("comment_pr",) if may_comment and repo.get("archived") is False else ()
    return kinds.Access(principal, resource, label, operations, _Session(call, label))


def _github_session(host: str, timeout: int, ports: kinds.Ports) -> Callable:
    try:
        executable = programs.path("gh") if ports.run is supervise.run_program else ports.which("gh")
    except programs.ProgramError:
        raise kinds.ConnectorError("reconnect_required") from None
    if not executable:
        raise kinds.ConnectorError("reconnect_required")
    environment = programs.environment("gh", executable)
    token_var = "GH_TOKEN" if host == "github.com" or host.endswith(".ghe.com") else "GH_ENTERPRISE_TOKEN"
    token = environment.get(token_var) or environment.get("GITHUB" + token_var[2:])
    if not token:
        result = ports.run((executable, "auth", "token", "--hostname", host), environment, timeout)
        if result.exit_code != 0 or result.over_limit or result.timed_out:
            raise kinds.ConnectorError("reconnect_required")
        token = result.stdout.decode("utf-8", "replace").strip()
    if not token:
        raise kinds.ConnectorError("reconnect_required")
    environment[token_var] = token

    def call(method, path, data, seconds, *, include_scopes=False):
        argv = [executable, "api", "--hostname", host, "--method", method]
        if include_scopes:
            argv.append("--include")
        argv.append(path)
        for key, value in (data or {}).items():
            argv.extend(("--raw-field", f"{key}={value}"))
        result = ports.run(tuple(argv), environment, seconds)
        if result.exit_code != 0 or result.timed_out or result.over_limit:
            raise kinds.ConnectorError("connector_unavailable")
        if include_scopes:
            return _github_response(result.stdout)
        return json.loads(result.stdout)

    return call


def _github_response(raw: bytes) -> tuple[dict, set[str]]:
    headers, separator, body = raw.replace(b"\r\n", b"\n").partition(b"\n\n")
    if not separator:
        raise ValueError
    scopes = next((line.partition(b":")[2] for line in headers.splitlines()
                   if line.lower().startswith(b"x-oauth-scopes:")), b"")
    return json.loads(body), {word.strip() for word in scopes.decode("ascii").split(",")}


def _numeric(value: object) -> int:
    if type(value) is not int or value < 1:
        raise ValueError
    return value


def _path_segment(value: str) -> str:
    if value in (".", ".."):
        raise ValueError
    return quote(value, safe="")


def _github_path(label: str) -> str:
    match = GITHUB_REPO.fullmatch(label)
    if match is None or match["host"]:
        raise ValueError
    return f"repos/{_path_segment(match['owner'])}/{_path_segment(match['repo'])}"


def _integration(kind: str, ports: kinds.Ports) -> Callable:
    token = ports.keychain(kinds.KEYCHAIN_SERVICES[kind])
    if not token:
        raise kinds.ConnectorError("reconnect_required")
    headers = {"Content-Type": "application/json", "Authorization": token}
    if kind == kinds.NOTION:
        headers.update({"Authorization": f"Bearer {token}", "Notion-Version": kinds.NOTION_VERSION})
    return lambda method, url, data, seconds: (ports.http or http)(method, url, headers, data, seconds)


def _linear(call, query: str, variables: dict, timeout: int) -> dict:
    response = call("POST", kinds.LINEAR_URL, {"query": query, "variables": variables}, timeout)
    if response.get("errors") or not isinstance(response.get("data"), dict):
        raise kinds.ConnectorError("scope_unavailable")
    return response["data"]


def _linear_access(selectors: dict, timeout: int, ports: kinds.Ports) -> kinds.Access:
    call = _integration(kinds.LINEAR, ports)
    query = ("query($team: String!) { viewer { id organization { id } } team(id: $team) { id name } }")
    data = _linear(call, query, {"team": selectors["team"]}, timeout)
    viewer, team = data["viewer"], data["team"]
    principal = f"{viewer['organization']['id']}:{viewer['id']}"
    recipient = selectors.get("recipient", viewer["id"])
    return kinds.Access(principal, team["id"], team["name"], session=_Session(call, recipient=recipient))


def _notion_access(selectors: dict, timeout: int, ports: kinds.Ports) -> kinds.Access:
    call = _integration(kinds.NOTION, ports)
    database = selectors["database"]
    if not PROVIDER_ID.fullmatch(database):
        raise kinds.ConnectorError("scope_unavailable")
    viewer = call("GET", f"{NOTION_BASE}/users/me", None, timeout)
    doc = call("GET", f"{NOTION_BASE}/databases/{database}", None, timeout)
    if viewer.get("type") != "bot" or doc.get("object") != "database" or doc.get("archived"):
        raise kinds.ConnectorError("scope_unavailable")
    return kinds.Access(viewer["id"], doc["id"], kinds._notion_title(doc), session=_Session(call))


def page(connector, access: kinds.Access, after: str, timeout: int) -> kinds.Page:
    """Request one page from the verified provider session."""
    if connector.kind == kinds.GITHUB:
        return _github_page(connector, access, after, timeout)
    if connector.kind == kinds.LINEAR:
        return _linear_page(access, after, timeout)
    return _notion_page(access, after, timeout)


def _github_page(connector, access: kinds.Access, after: str, timeout: int) -> kinds.Page:
    stream, _, number = (after or "pulls:1").partition(":")
    criteria = {"state": "open", "per_page": PAGE_SIZE, "page": int(number), "sort": "updated", "direction": "desc"}
    if branch := connector.selectors.get("branch"):
        criteria["base"] = branch
    path = f"{_github_path(access.session.label)}/{stream}?{urlencode(criteria)}"
    documents = access.session.call("GET", path, None, timeout)
    if not isinstance(documents, list):
        raise TypeError
    records = [_github_record(access, row, stream == "pulls") for row in documents
               if stream == "pulls" or "pull_request" not in row]
    next_page = f"{stream}:{int(number) + 1}" if len(documents) == PAGE_SIZE else ""
    if not next_page and stream == "pulls" and connector.selectors.get("whole_repo"):
        next_page = "issues:1"
    return kinds.Page(records, next_page)


def _github_record(access: kinds.Access, row: dict, is_pr: bool) -> dict:
    host, _, _ = access.resource.partition(":")
    target = str(_numeric(row["number"]))
    if is_pr:
        base = row["base"]
        resource = f"{host}:{_numeric(base['repo']['id'])}"
        branch = base["ref"]
        if not isinstance(branch, str) or not branch:
            raise ValueError
    else:
        url = row["repository_url"]
        prefix = "https://api.github.com/" if host == "github.com" else f"https://{host}/api/v3/"
        resource = access.resource if url == prefix + _github_path(access.session.label) else ""
        branch = ""
    item = {"id": f"github:{resource}:{target}", "title": row["title"], "url": row["html_url"], "at": row["updated_at"]}
    return {"resource": resource, "branch": branch, "target": target, "item": item,
            "is_issue": not is_pr, "head": row.get("head")}


def _linear_page(access: kinds.Access, after: str, timeout: int) -> kinds.Page:
    query = ("query($id: String!, $after: String, $first: Int!) { team(id: $id) { issues(after: $after, first: $first) { "
             "pageInfo { hasNextPage endCursor } nodes { " + LINEAR_FIELDS + " } } } }")
    args = {"id": access.resource, "after": after or None, "first": PAGE_SIZE}
    result = _linear(access.session.call, query, args, timeout)["team"]["issues"]
    page_info = result["pageInfo"]
    return kinds.Page([_linear_record(row) for row in result["nodes"]], _next(page_info, "hasNextPage", "endCursor"))


def _linear_record(row: dict) -> dict:
    item = {"id": f"linear:{row['id']}", "title": f"{row['identifier']} {row['title']}"[:schema.MAX_ITEM_TITLE],
            "url": row["url"], "at": row["updatedAt"]}
    return {"resource": row["team"]["id"], "target": row["id"], "item": item, "assignee": (row["assignee"] or {}).get("id")}


def _notion_page(access: kinds.Access, after: str, timeout: int) -> kinds.Page:
    path = f"{NOTION_BASE}/databases/{access.resource}/query"
    args = {"page_size": PAGE_SIZE, **({"start_cursor": after} if after else {})}
    result = access.session.call("POST", path, args, timeout)
    return kinds.Page([_notion_record(row) for row in result["results"]], _next(result, "has_more", "next_cursor"))


def _notion_record(row: dict) -> dict:
    parent = row["parent"]
    if parent.get("type") != "database_id" or row.get("object") != "page" or row.get("archived"):
        raise kinds.ConnectorError("scope_unavailable")
    item = kinds._notion_items([row])[0]
    return {"resource": parent["database_id"], "target": row["id"], "item": item, "created": row["created_time"]}


def _next(page_info: dict, has_key: str, cursor_key: str) -> str:
    has_next = page_info[has_key]
    if has_next is False:
        return ""
    if has_next is not True or not isinstance(page_info[cursor_key], str) or not page_info[cursor_key]:
        raise ValueError
    return page_info[cursor_key]


def lookup(connector, access: kinds.Access, target: str, timeout: int) -> dict:
    """Load target membership from the same authenticated session as the write."""
    call = access.session.call
    if connector.kind == kinds.GITHUB:
        if not target.isdecimal():
            raise kinds.ConnectorError("invalid_request")
        row = call("GET", f"{_github_path(access.session.label)}/pulls/{target}", None, timeout)
        return _github_record(access, row, True)
    if connector.kind == kinds.LINEAR:
        query = "query($id: String!) { issue(id: $id) { " + LINEAR_FIELDS + " } }"
        return _linear_record(_linear(call, query, {"id": target}, timeout)["issue"])
    return _notion_record(call("GET", f"{NOTION_BASE}/pages/{target}", None, timeout))


def send(connector, access: kinds.Access, target: str, text: str, timeout: int) -> None:
    """Send an allowlisted write using the authenticated session."""
    call = access.session.call
    if connector.kind == kinds.GITHUB:
        call("POST", f"{_github_path(access.session.label)}/issues/{target}/comments", {"body": text}, timeout)
    elif connector.kind == kinds.LINEAR:
        query = "mutation($input: CommentCreateInput!) { commentCreate(input: $input) { success } }"
        outcome = _linear(call, query, {"input": {"issueId": target, "body": text}}, timeout)
        if outcome["commentCreate"]["success"] is not True:
            raise kinds.ConnectorError("connector_unavailable")
    else:
        paragraph = {"type": "paragraph", "paragraph": {"rich_text": [{"type": "text", "text": {"content": text}}]}}
        call("PATCH", f"{NOTION_BASE}/blocks/{target}/children", {"children": [paragraph]}, timeout)


def failed_checks(access: kinds.Access, row: dict, deadline: float) -> list[dict]:
    """Read all check runs for the verified PR's head, keeping only completed failures."""
    head = row["head"]["sha"]
    if not re.fullmatch(r"[a-fA-F0-9]{40,64}", head):
        raise ValueError
    path = f"{_github_path(access.session.label)}/commits/{head}/check-runs"
    found = []
    for page in range(1, kinds.MAX_PAGES + 1):
        query = urlencode({"filter": "all", "per_page": PAGE_SIZE, "page": page})
        body = access.session.call("GET", f"{path}?{query}", None, kinds._remaining(deadline))
        records = body["check_runs"]
        for run in records:
            if run["head_sha"] != head:
                raise kinds.ConnectorError("scope_unavailable")
            if run["conclusion"] == "failure" and run["status"] == "completed":
                found.append(run)
        if len(records) < PAGE_SIZE:
            return found
    raise kinds.ConnectorError("connector_unavailable")
