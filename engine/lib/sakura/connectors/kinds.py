"""What each kind of connector is and how it reads.

`github` is the `gh` command signed in as the person; `notion` and `linear` use an integration token the person stores
in the Keychain (`sakura-connector-notion`, `sakura-connector-linear`), read through `security find-generic-password` at
the moment of use, never printed, logged or stored by the engine; `feed` is an RSS or Atom address and `program` the
person's own program, which the digest reads as it reads a `feed` or `run` source. Everything a connector returns is
untrusted text: items pass the digest's own bounds (`digest.schema`). No error text carries a token, a response body or
a program's own output: only fixed sentences, a status code or a host name.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import shutil
import stat
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

from sakura import git_program, programs, text_limits
from sakura.digest import config, feed, installation, schema, supervise
from sakura.flow import process

GITHUB, NOTION, LINEAR, FEED, PROGRAM = "github", "notion", "linear", "feed", "program"
KINDS = (GITHUB, NOTION, LINEAR, FEED, PROGRAM)
READ_BY_DIGEST = (FEED, PROGRAM)  # read by the digest's own feed and program paths, not here
KEYCHAIN_SERVICES = {NOTION: "sakura-connector-notion", LINEAR: "sakura-connector-linear"}
KEYCHAIN_PROGRAM = "/usr/bin/security"
KEYCHAIN_TIMEOUT_SECONDS = 10
GITHUB_LIMIT = "30"
NOTION_URL, NOTION_VERSION, NOTION_PAGE_SIZE = "https://api.notion.com/v1/search", "2022-06-28", 50
LINEAR_URL, LINEAR_PAGE_SIZE = "https://api.linear.app/graphql", 50
LINEAR_QUERY = ("query($since: DateTimeOrDuration!) { viewer { assignedIssues(first: %d, filter: {updatedAt: {gt: $since}}) "
                "{ nodes { identifier title url updatedAt } } } }" % LINEAR_PAGE_SIZE)
GITHUB_SEARCHES = (("prs", "--review-requested=@me", "review requested"), ("prs", "--assignee=@me", "assigned"),
                   ("issues", "--assignee=@me", "assigned"))
HTTP_SCHEMES = ("https://", "http://")
UNWRITABLE_BY_OTHERS = 0o022
MAX_ADDRESS = 500
MAX_PAGES = 12
MAX_WRITE_TEXT = 1800
READ = "read"
WRITE_OPERATIONS = {GITHUB: "comment_pr", LINEAR: "comment_issue", NOTION: "append_page"}
EVENT_KEYS = {GITHUB: "check_failed", LINEAR: "issue_assigned", NOTION: "page_added", FEED: "new_item"}
SAFE_RESOURCE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,119}")


class ConnectorError(ValueError):
    """Raised, with one plain sentence, when a connector cannot be connected or read."""


Runner = Callable[[tuple[str, ...], Mapping[str, str], int], supervise.Execution]
Poster = Callable[[str, Mapping[str, str], bytes, int], bytes]


def read_keychain(service: str, run: Runner = supervise.run_program) -> str | None:
    """The secret of the Keychain item `service`, or None when there is none or it cannot be read."""
    outcome = run((KEYCHAIN_PROGRAM, "find-generic-password", "-s", service, "-w"), process.child_environment(os.environ),
                  KEYCHAIN_TIMEOUT_SECONDS)
    secret = outcome.stdout.decode("utf-8", "replace").strip()
    return secret if outcome.exit_code == 0 and secret else None


def post_json(url: str, headers: Mapping[str, str], body: bytes, timeout: int) -> bytes:
    """POST `body` and return the answer, at most `schema.MAX_SOURCE_BYTES` of it. Raises ConnectorError with a status
    code or the host name only: never a header, the body or the server's own text."""
    request = urllib.request.Request(url, data=body, headers=dict(headers), method="POST")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as answer:
            return answer.read(schema.MAX_SOURCE_BYTES)
    except urllib.error.HTTPError as error:
        raise ConnectorError(f"HTTP {error.code}") from None
    except (urllib.error.URLError, OSError):
        raise ConnectorError(f"could not reach {urllib.request.urlparse(url).hostname}") from None


def owner_of(path: Path) -> int:
    """The user id that owns `path`."""
    return os.stat(path).st_uid


@dataclass(frozen=True)
class Access:
    """Provider-verified principal, resource identity, display name and operation keys."""

    principal: str
    resource: str
    label: str = ""
    operations: tuple[str, ...] = ()
    session: object = field(default=None, repr=False, compare=False)


@dataclass(frozen=True)
class Page:
    """Bounded provider records, each with independent evidence of resource membership."""

    records: list[dict]
    after: str = ""


@dataclass(frozen=True)
class Ports:
    """What connectors run, read and call, injectable so no test starts a program, opens the Keychain or the network."""

    run: Runner = supervise.run_program
    post: Poster = post_json
    keychain: Callable[[str], str | None] = read_keychain
    which: Callable[[str], str | None] = shutil.which
    fetch_feed: feed.Fetcher = feed.fetch
    inspect: Callable[[str, dict, int], Access] | None = None
    page: Callable[..., Page] | None = None
    lookup: Callable[..., dict] | None = None
    send: Callable[..., None] | None = None
    http: Callable[..., object] | None = None
    prove_replacement: Callable[..., object] | None = None


@dataclass(frozen=True)
class Status:
    """Whether a connector can be used now, and in one sentence why not."""

    is_connected: bool
    detail: str
    code: str = ""


@dataclass(frozen=True)
class Fetched:
    """What a connector gave for one digest source: items, the new cursor and the number of entries skipped."""

    items: tuple[schema.Item, ...] = ()
    cursor: str = ""
    skipped: int = 0


# Sign-in checks

def check(kind: str, target: str, ports: Ports) -> Status:
    """Whether `kind` (with its `target` address or program) can be used now; reads no remote service."""
    if kind == GITHUB:
        return _check_github(ports)
    if kind in KEYCHAIN_SERVICES:
        service = KEYCHAIN_SERVICES[kind]
        found = ports.keychain(service)
        return Status(True, "the token is in the Keychain") if found else Status(
            False, f"no Keychain item {service}: add the integration token there")
    if kind == FEED:
        is_address = target.startswith(HTTP_SCHEMES) and len(target) <= MAX_ADDRESS and not re.search(r"\s", target)
        return Status(is_address, "an http or https address" if is_address else "a feed needs one http or https address")
    if kind == PROGRAM:
        problem = program_problem(target)
        return Status(problem is None, problem or "the program is in place")
    raise ConnectorError(f"unknown kind {kind!r}; the kinds are {', '.join(KINDS)}")


def prepare_scope(kind: str, fields: dict, ports: Ports) -> tuple[Access, dict, dict]:
    """Resolve an operator selector to a verified resource and safe display values."""
    if kind == FEED:
        try:
            address = urlsplit(fields["address"])
            if (not check(kind, fields["address"], ports).is_connected or not address.hostname or address.username
                    or address.password or address.fragment or address.port == 0):
                raise ValueError
        except ValueError:
            raise ConnectorError("invalid_choices") from None
        return Access("", ""), {"host": address.hostname}, {"host": address.hostname}
    if kind == PROGRAM:
        return Access("", program_identity(fields["program"], fields.get("args", ()))), {"reader": "custom"}, {}
    selector = {GITHUB: "repo", LINEAR: "team", NOTION: "database"}[kind]
    requested = {key: fields[key] for key in (selector, "branch", "whole_repo", "recipient") if key in fields}
    access = _inspect(kind, requested, KEYCHAIN_TIMEOUT_SECONDS, ports)
    return access, {**requested, selector: access.resource}, _display(access.label, requested)


def _display(label: str, selectors: dict) -> dict:
    display = {"resource": label} if not text_limits.one_line_problem(label, 160) else {}
    if "branch" in selectors:
        display["branch"] = selectors["branch"]
    return display


def _inspect(kind: str, selectors: dict, timeout: int, ports: Ports) -> Access:
    from sakura.connectors import providers

    try:
        access = ports.inspect(kind, selectors, timeout) if ports.inspect else providers.inspect(kind, selectors, timeout, ports)
        if not access.principal or not access.resource or text_limits.one_line_problem(access.resource, 200):
            raise ConnectorError("scope_unavailable")
        return access
    except ConnectorError as error:
        if str(error) in {"reconnect_required", "scope_unavailable"}:
            raise
        raise ConnectorError("connector_unavailable") from None


def verify_scope(connector, timeout: int, ports: Ports) -> Access:
    """Recheck the principal and resource before using a scoped provider."""
    if connector.revoked:
        raise ConnectorError("connector_unavailable")
    access = _inspect(connector.kind, connector.selectors, timeout, ports)
    if access.principal != connector.principal:
        raise ConnectorError("reconnect_required")
    selector = {GITHUB: "repo", LINEAR: "team", NOTION: "database"}[connector.kind]
    if access.resource != connector.selectors[selector]:
        raise ConnectorError("scope_unavailable")
    return access


def check_instance(connector, ports: Ports) -> Status:
    """Check the saved instance's scope and current sign-in."""
    if connector.revoked:
        return Status(False, "disconnected", "disconnected")
    if not connector.selectors or connector.kind == FEED:
        return check(connector.kind, connector.target, ports)
    try:
        if connector.kind == PROGRAM:
            if (program_identity(connector.target, connector.arguments) != connector.executable_identity
                    or installation.inspect((connector.target, *connector.arguments)).identity()
                    != connector.program_identity):
                raise ConnectorError("approval_required")
        else:
            verify_scope(connector, KEYCHAIN_TIMEOUT_SECONDS, ports)
    except installation.InstallationError:
        return Status(False, "approval_required", "approval_required")
    except ConnectorError as error:
        return Status(False, str(error), str(error))
    return Status(True, "connected", "connected")


def program_identity(target: str, arguments=()) -> str:
    """Fingerprint the executable approved by its owner."""
    try:
        if program_problem(target) or programs.untrusted(target):
            raise ConnectorError("approval_required")
        _check_arguments(arguments)
        path = Path(target).resolve(strict=True)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode):
                raise ConnectorError("approval_required")
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        return f"{info.st_dev}:{info.st_ino}:{digest}"
    except (OSError, RuntimeError, ValueError):
        raise ConnectorError("approval_required") from None


def _check_arguments(arguments: tuple[str, ...]) -> None:
    paths = []
    for word in arguments:
        value = word.partition("=")[2] if word.startswith("-") and "=" in word else word
        if "/" not in value or value.startswith(("https://", "http://")):
            continue
        if not os.path.isabs(value):
            raise ConnectorError("approval_required")
        paths.append(os.path.realpath(value))
    if paths:
        try:
            roots = programs.forbidden_roots()
        except Exception as error:
            raise ConnectorError("approval_required") from error
        if any(path in roots or git_program.holding_folder(path, roots) for path in paths):
            raise ConnectorError("approval_required")


def _check_github(ports: Ports) -> Status:
    if not ports.which("gh"):
        return Status(False, "gh is not installed: install the GitHub command line tool")
    outcome = ports.run(("gh", "auth", "status"), process.child_environment(os.environ), KEYCHAIN_TIMEOUT_SECONDS)
    if outcome.exit_code == 0:
        return Status(True, "signed in with gh")
    return Status(False, "gh is not signed in: run gh auth login")


def program_problem(target: str) -> str | None:
    """Why `target` cannot be a program connector, or None: absolute, an executable file the person owns, and not
    writable by anyone else."""
    if not os.path.isabs(target):
        return "the program path must be absolute"
    path = Path(target)
    if not path.is_file() or not os.access(path, os.X_OK):
        return "the program is not an executable file"
    if owner_of(path) != os.getuid():
        return "the program is not owned by you"
    if stat.S_IMODE(path.stat().st_mode) & UNWRITABLE_BY_OTHERS:
        return "the program is writable by others"
    try:
        installation.inspect((target,)).verify()
    except installation.InstallationError as error:
        return str(error)
    return None


# Reading

def read(connector, since: str, cursor: str, timeout: int, ports: Ports | None = None) -> Fetched:
    """The items a github, notion or linear connector gives since `since` (or after `cursor`).

    Raises ConnectorError with a fixed sentence when the service or the sign-in fails."""
    if connector.revoked:
        raise ConnectorError("connector_unavailable")
    ports = ports or Ports()
    if connector.kind == FEED or connector.kind == PROGRAM:
        return _local_read(connector, since, cursor, timeout, ports)
    if connector.selectors:
        records = scoped_records(connector, timeout, ports)
        return _bounded(_fetched([record["item"] for record in records], since, cursor))
    if connector.kind == GITHUB:
        return _read_github(since, cursor, timeout, ports)
    if connector.kind in KEYCHAIN_SERVICES:
        return _read_service(connector.kind, since, cursor, timeout, ports)
    raise ConnectorError(f"a {connector.kind} connector is read by the digest itself")


def scoped_records(connector, timeout: int, ports: Ports) -> list[dict]:
    """Return a complete bounded provider listing after checking every record's membership."""
    return scoped_listing(connector, time.monotonic() + timeout, ports)[1]


def scoped_listing(connector, deadline: float, ports: Ports) -> tuple[Access, list[dict]]:
    """Return the live session with the complete and scope-checked listing."""
    from sakura.connectors import providers

    access = verify_scope(connector, _remaining(deadline), ports)
    page = ports.page or providers.page
    records, seen, after = [], set(), ""
    for _ in range(MAX_PAGES):
        found = _provider_call(page, connector, access, after, _remaining(deadline))
        if not isinstance(found, Page) or len(found.records) > schema.MAX_ITEMS_PER_SOURCE:
            raise ConnectorError("connector_unavailable")
        for record in found.records:
            if in_scope(connector, access, record):
                records.append(record)
        after = found.after
        if not after:
            return access, records
        if after in seen:
            break
        seen.add(after)
    raise ConnectorError("connector_unavailable")


def in_scope(connector, access: Access, record: dict) -> bool:
    """Check canonical provider membership. Absent evidence invalidates the operation."""
    resource = record.get("resource")
    if not isinstance(resource, str) or not resource:
        raise ConnectorError("scope_unavailable")
    if resource != access.resource:
        return False
    if "branch" not in connector.selectors:
        return True
    if record.get("is_issue") is True:
        return False
    branch = record.get("branch")
    if not isinstance(branch, str) or not branch:
        raise ConnectorError("scope_unavailable")
    return branch == connector.selectors["branch"]


def write(connector, operation: str, target: str, text: str, timeout: int, ports: Ports | None = None) -> None:
    """Perform a fixed write after verifying live capability and target membership."""
    from sakura.connectors import providers

    if (not isinstance(target, str) or not SAFE_RESOURCE.fullmatch(target) or not isinstance(text, str)
            or text_limits.text_problem(text, MAX_WRITE_TEXT) or text_limits.hidden_character_problem(text)):
        raise ConnectorError("invalid_request")
    if not connector.selectors or operation != WRITE_OPERATIONS.get(connector.kind):
        raise ConnectorError("connector_unavailable")
    ports = ports or Ports()
    deadline = time.monotonic() + timeout
    access = verify_scope(connector, _remaining(deadline), ports)
    if operation not in access.operations or operation not in connector.permissions:
        raise ConnectorError("connector_unavailable")
    record = _provider_call(ports.lookup or providers.lookup, connector, access, target, _remaining(deadline))
    if not in_scope(connector, access, record) or record.get("target") != target:
        raise ConnectorError("scope_unavailable")
    _provider_call(ports.send or providers.send, connector, access, target, text, _remaining(deadline))


def operation_keys(connector) -> tuple[str, ...]:
    """The connector operations available under its saved scope and permissions."""
    if connector.revoked or not connector.connected or not connector.selectors:
        return ()
    return (READ, *(key for key in connector.permissions if key == WRITE_OPERATIONS.get(connector.kind)))


def event_keys(connector) -> tuple[str, ...]:
    """Event streams a supported scoped instance can provide."""
    key = EVENT_KEYS.get(connector.kind)
    return (key,) if key and operation_keys(connector) else ()


def _provider_call(call: Callable, *args):
    try:
        return call(*args)
    except (ConnectorError, OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        code = str(error)
        if code in {"reconnect_required", "scope_unavailable", "connector_unavailable", "invalid_request"}:
            raise ConnectorError(code) from None
        raise ConnectorError("connector_unavailable") from None


def _remaining(deadline: float) -> int:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise ConnectorError("connector_unavailable")
    return math.ceil(remaining)


def _bounded(fetched: Fetched) -> Fetched:
    if len(fetched.items) > schema.MAX_ITEMS_PER_SOURCE:
        raise ConnectorError("connector_unavailable")
    return fetched


def _local_read(connector, since: str, cursor: str, timeout: int, ports: Ports) -> Fetched:
    from sakura.digest import intake

    source_id = "connector"
    if connector.kind == FEED:
        source = config.Source(source_id, feed=connector.target, timeout_seconds=timeout)
        result = intake.run_source(source, since, cursor, fetch=ports.fetch_feed)
    else:
        target = str(Path(connector.target).resolve())
        if not connector.executable_identity or program_identity(target, connector.arguments) != connector.executable_identity:
            raise ConnectorError("approval_required")
        source = config.Source(source_id, run=(target, *connector.arguments), timeout_seconds=timeout)
        def run(argv, env, seconds):
            chosen = process.scrubbed_environment(env)
            chosen.update({key: env[key] for key in (intake.SINCE_VARIABLE, intake.CURSOR_VARIABLE)})
            return ports.run(argv, chosen, seconds)

        result = intake._from_program(source, since, cursor, run, connector.program_identity)
        if result.report.error == installation.CHANGED:
            raise ConnectorError("approval_required")
    if not result.report.ok:
        raise ConnectorError("connector_unavailable")
    return Fetched(result.items, result.cursor, result.report.skipped)


def _read_github(since: str, cursor: str, timeout: int, ports: Ports) -> Fetched:
    documents = []
    for what, flag, reason in GITHUB_SEARCHES:
        argv = ("gh", "search", what, flag, "--state=open", "--json", "number,title,url,repository,updatedAt",
                "--limit", GITHUB_LIMIT)
        outcome = ports.run(argv, process.child_environment(os.environ), timeout)
        if outcome.start_error or outcome.timed_out or outcome.exit_code != 0:
            raise ConnectorError("gh did not answer: run gh auth status")
        documents += [_github_item(found, reason) for found in _json_list(outcome.stdout.decode("utf-8", "replace"))]
    return _fetched(documents, since, cursor)


def _github_item(found: object, reason: str) -> dict:
    found = found if isinstance(found, dict) else {}
    repository = (found.get("repository") or {}).get("nameWithOwner", "") if isinstance(found.get("repository"), dict) else ""
    label = f"{repository} #{found.get('number')}"
    return {"id": f"github:{label}", "title": f"{label} {found.get('title', '')}"[:schema.MAX_ITEM_TITLE],
            "url": found.get("url", ""), "at": found.get("updatedAt", ""), "text": reason}


def _read_service(kind: str, since: str, cursor: str, timeout: int, ports: Ports) -> Fetched:
    token = ports.keychain(KEYCHAIN_SERVICES[kind])
    if not token:
        raise ConnectorError(f"no Keychain item {KEYCHAIN_SERVICES[kind]}: add the integration token there")
    if kind == NOTION:
        answer = ports.post(NOTION_URL, {"Authorization": f"Bearer {token}", "Notion-Version": NOTION_VERSION,
                                         "Content-Type": "application/json"},
                            json.dumps({"sort": {"direction": "descending", "timestamp": "last_edited_time"},
                                        "page_size": NOTION_PAGE_SIZE}).encode(), timeout)
        return _fetched(_notion_items(_json_object(answer).get("results")), since, cursor)
    body = json.dumps({"query": LINEAR_QUERY, "variables": {"since": cursor or since}}).encode()
    answer = ports.post(LINEAR_URL, {"Authorization": token, "Content-Type": "application/json"}, body, timeout)
    nodes = (((_json_object(answer).get("data") or {}).get("viewer") or {}).get("assignedIssues") or {}).get("nodes")
    return _fetched([{"id": f"linear:{node.get('identifier')}", "title": f"{node.get('identifier')} {node.get('title', '')}"
                      [:schema.MAX_ITEM_TITLE], "url": node.get("url", ""), "at": node.get("updatedAt", "")}
                     for node in nodes or [] if isinstance(node, dict)], since, cursor)


def _notion_items(results: object) -> list[dict]:
    items = []
    for page in results if isinstance(results, list) else []:
        if isinstance(page, dict):
            items.append({"id": f"notion:{page.get('id')}", "title": _notion_title(page), "url": page.get("url", ""),
                          "at": page.get("last_edited_time", "")})
    return items


def _notion_title(page: dict) -> str:
    properties = page.get("properties") if isinstance(page.get("properties"), dict) else {}
    pieces = next((value.get("title") for value in properties.values() if isinstance(value, dict)
                   and value.get("type") == "title"), page.get("title"))
    words = "".join(piece.get("plain_text", "") for piece in pieces if isinstance(piece, dict)) if isinstance(pieces, list) else ""
    return (words or "Untitled")[:schema.MAX_ITEM_TITLE]


def _fetched(documents: list[dict], since: str, cursor: str) -> Fetched:
    """The documents newer than the cursor (else `since`) as bounded items, newest cursor; invalid ones are counted."""
    threshold, items, skipped, newest, seen = cursor or since, [], 0, cursor, set()
    for document in documents:
        try:
            item = schema.parse_item({key: value for key, value in document.items() if value not in ("", None)})
        except schema.SchemaError:
            skipped += 1
            continue
        if item.id in seen or (item.at and threshold and item.at <= threshold):
            continue
        seen.add(item.id)
        items.append(item)
        newest = max(newest, item.at)
    return Fetched(tuple(items), newest, skipped)


def _json_list(text: str) -> list:
    try:
        value = json.loads(text)
    except ValueError:
        raise ConnectorError("the answer was not readable") from None
    return value if isinstance(value, list) else []


def _json_object(answer: bytes) -> dict:
    try:
        value = json.loads(answer.decode("utf-8", "replace"))
    except ValueError:
        raise ConnectorError("the answer was not readable") from None
    return value if isinstance(value, dict) else {}
