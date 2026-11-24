"""The cockpit's HTTP server: overlay address only, bounded requests, every denial a fixed 403.

It is transport and gate only: the pages and the actions behind them are supplied as two callables."""

from __future__ import annotations

import fcntl
import os
import re
import secrets
import socket
import socketserver
import sys
import threading
import time
from collections.abc import Callable
from http import HTTPStatus
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import Protocol
from urllib.parse import parse_qsl

from sakura import engine_home
from sakura.cockpit import actions, gate, pages, track_actions
from sakura.cockpit import api as api_documents
from sakura.cockpit.controller import Refused, overlay_actor
from sakura.cockpit.settings import CockpitRefused, CockpitSettings
from sakura.flow.render import strip_controls
from sakura.tracks.domain import ANSWER_LIMIT

SOCKET_TIMEOUT_SECONDS = 10
MAX_HEADER_BYTES = 8192
MAX_HEADERS = 50
MAX_BODY_BYTES = 16384
# A Unicode character can occupy four UTF-8 bytes, each percent-encoded as three form bytes.
MAX_TRACK_BODY_BYTES = ANSWER_LIMIT * 12 + 4096
TRACK_ANSWER_ROUTES = {f"{path}?{track_actions.PHONE_FORM_QUERY}" for path in ("/action", "/api/action")}
MAX_HANDLERS = 8
MAX_RUNNING_ACTIONS = 2
MAX_FORM_FIELDS = 50
OVERLOAD_WRITE_SECONDS = 1
MAX_LOG_CHARS = 300
LOG_FILE_MODE, LOG_DIRECTORY_MODE = 0o600, 0o700
ACCESS_LOG = Path("cockpit") / "access.log"
FORBIDDEN_BODY = b"forbidden\n"
ACTION_BUSY_REFUSAL = "another action is running"
OWN_NODE_REFUSAL = "writes from the cockpit's own machine go through sakura cockpit act"
FORM_TYPE = re.compile(r'\s*application/x-www-form-urlencoded\s*(?:;\s*charset\s*=\s*("?)utf-8\1\s*)?', re.IGNORECASE)
STYLESHEET_PATH = "/style.css"
TEXT_TYPE = "text/plain; charset=utf-8"
HTML_TYPE = "text/html; charset=utf-8"
CSS_TYPE = "text/css; charset=utf-8"
JSON_TYPE = "application/json; charset=utf-8"
API_ROUTES = (("GET", "/api/session"), ("GET", "/api/snapshot"), ("POST", "/api/action"))
API_PREFIX = "/api/"
HTTP_VERSIONS = ("HTTP/1.0", "HTTP/1.1")
SECURITY_HEADERS = (
    ("Content-Security-Policy", "default-src 'none'; style-src 'self'; form-action 'self'; frame-ancestors 'none'"),
    ("X-Frame-Options", "DENY"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
    ("X-Content-Type-Options", "nosniff"),
)
METHOD = re.compile(r"[A-Z]{1,16}")
TARGET = re.compile(r"/[\x21-\x7e]{0,2000}")
HEADER_LINE = re.compile(r"([!#$%&'*+.^_`|~0-9A-Za-z-]+):[ \t]*([^\x00-\x08\x0a-\x1f\x7f]*?)[ \t]*")
LENGTH = re.compile(r"[0-9]{1,6}")

IndexPage = Callable[[gate.Identity, str], str]
ActionRunner = Callable[[dict, gate.Identity, str], str]


class ApiRoutes(Protocol):
    """What the /api/ routes need: three calls that answer with JSON-ready documents."""

    def session(self, identity: gate.Identity, csrf: str) -> dict: ...

    def snapshot(self) -> dict: ...

    def action(self, fields: dict, identity: gate.Identity) -> tuple[int, dict]: ...


class _HttpError(Exception):
    """A request refused with a status; `reason` goes to the access log, `body` to the client."""

    def __init__(self, status: int, reason: str, body: bytes | None = None) -> None:
        super().__init__(reason)
        self.status, self.reason = status, reason
        self.body = body if body is not None else HTTPStatus(status).phrase.encode() + b"\n"


class _Drop(Exception):
    """The client went away or stalled; nothing is answered."""


WATCH_SECONDS = 1.0


class SettingsWatcher:
    """Keeps a running server's settings the ones in `cockpit.toml`: who may open the page changes the moment the file does, and a new
    port makes the server bind again. A file that cannot be read (an edit half done by another tool) changes nothing."""

    def __init__(self, httpd: CockpitServer, reload: Callable[[], CockpitSettings]) -> None:
        self._httpd, self._reload = httpd, reload
        self.settings = httpd.settings

    def check(self) -> str:
        """Read the file once: `same`, `kept` (unreadable), `updated` (who may open the page changed) or `rebind` (the port did)."""
        try:
            fresh = self._reload()
        except CockpitRefused:
            return "kept"
        if fresh == self.settings:
            return "same"
        self.settings = fresh
        if fresh.port != self._httpd.settings.port:
            return "rebind"
        self._httpd.settings = fresh
        return "updated"


def serve(settings: CockpitSettings, index_page: IndexPage, action_runner: ActionRunner,
          access_log: Path | None = None, *, api: ApiRoutes | None = None, action_log: Path | None = None,
          stop: threading.Event | None = None, reload: Callable[[], CockpitSettings] | None = None) -> None:
    """Listen on this machine's overlay address and port until interrupted, or until `stop` is set.

    `index_page(identity, csrf)` returns the overview, `action_runner(fields, identity, csrf)` the page that
    answers a POST to /action; either may raise gate.Denied to refuse. `csrf` is a fresh token for that identity.
    `api` serves the JSON routes of `API_ROUTES`; without it every /api/ path is a 404. A write request from
    this machine's own overlay node is refused before `action_runner` or `api` see it, and the refusal is
    appended to `action_log` (default: the cockpit action log). With `reload`, the settings are read again every
    `WATCH_SECONDS`: a change of who may open the page applies at once, a new port binds again.

    Raises CockpitRefused when the Tailscale CLI cannot name a overlay address.
    """
    current = settings
    while current is not None:
        own = gate.read_self(current)
        httpd = CockpitServer(own, current, index_page, action_runner, access_log=access_log, api=api,
                              action_log=action_log)
        current = _serve_until_done(httpd, stop, reload)


def _serve_until_done(httpd: CockpitServer, stop: threading.Event | None,
                      reload: Callable[[], CockpitSettings] | None) -> CockpitSettings | None:
    """Serve until `stop` or a new port; the settings to bind again with, or None when the serving is over."""
    again: list[CockpitSettings] = []
    ended = threading.Event()
    if stop is not None:
        threading.Thread(target=lambda: (stop.wait(), httpd.shutdown()), name="cockpit-stop", daemon=True).start()
    if reload is not None:
        threading.Thread(target=_watch, args=(SettingsWatcher(httpd, reload), httpd, again, ended),
                         name="cockpit-settings", daemon=True).start()
    try:
        httpd.serve_forever()
    finally:
        ended.set()
        httpd.server_close()
    return again[0] if again else None


def _watch(watcher: SettingsWatcher, httpd: CockpitServer, again: list, ended: threading.Event) -> None:
    while not ended.wait(WATCH_SECONDS):
        if watcher.check() == "rebind":
            again.append(watcher.settings)
            httpd.shutdown()
            return


class CockpitServer(ThreadingHTTPServer):
    """Threaded server bound to the overlay address only, with a fixed number of handler slots."""

    daemon_threads = True

    def __init__(self, own: gate.Self, settings: CockpitSettings, index_page: IndexPage,
                 action_runner: ActionRunner, *, identify: Callable[[str], gate.Identity | None] | None = None,
                 access_log: Path | None = None, csrf_secret: bytes | None = None,
                 clock: Callable[[], float] = time.time, api: ApiRoutes | None = None,
                 action_log: Path | None = None) -> None:
        self._check_bind_address(own.address)
        self.settings, self.index_page, self.action_runner, self.api = settings, index_page, action_runner, api
        self.own_node_id, self.action_log = own.node_id, action_log
        self.identify = identify or gate.CachedIdentifier(settings)
        self.csrf_secret = csrf_secret or secrets.token_bytes(32)
        self.clock = clock
        self._access_log = Path(access_log).expanduser() if access_log else engine_home.state_dir() / ACCESS_LOG
        self._handlers = threading.BoundedSemaphore(MAX_HANDLERS)
        self.action_slots = threading.BoundedSemaphore(MAX_RUNNING_ACTIONS)
        super().__init__((own.address, settings.port), _Handler)
        self.authorities = gate.authorities(own, self.server_address[1])

    @staticmethod
    def _check_bind_address(address: str) -> None:
        gate.require_overlay_address(address)

    def server_bind(self) -> None:
        """Bind without the reverse DNS lookup HTTPServer does for a name nobody reads."""
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[0], self.server_address[1]

    def process_request(self, request, client_address) -> None:
        """Hand the connection to a thread, or answer 503 at once when every handler slot is busy."""
        if not self._handlers.acquire(blocking=False):
            self._refuse_overload(request, client_address)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self._handlers.release()
            raise

    def process_request_thread(self, request, client_address) -> None:
        try:
            super().process_request_thread(request, client_address)
        finally:
            self._handlers.release()

    def log_access(self, peer: str, summary: str) -> None:
        """Append one line to the access log; a failing log is reported on stderr, never hidden."""
        stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        line = strip_controls(f"{stamp} {peer} {summary}").replace("\n", " ")[:MAX_LOG_CHARS] + "\n"
        try:
            self._access_log.parent.mkdir(mode=LOG_DIRECTORY_MODE, parents=True, exist_ok=True)
            descriptor = os.open(self._access_log, os.O_WRONLY | os.O_APPEND | os.O_CREAT, LOG_FILE_MODE)
            with os.fdopen(descriptor, "a", encoding="utf-8") as log:
                fcntl.flock(log, fcntl.LOCK_EX)
                log.write(line)
        except OSError as error:
            print(f"cockpit: cannot write {self._access_log}: {error}: {line.strip()}", file=sys.stderr)

    def _refuse_overload(self, request, client_address) -> None:
        try:
            request.settimeout(OVERLOAD_WRITE_SECONDS)
            request.sendall(_response(HTTPStatus.SERVICE_UNAVAILABLE, b"busy\n"))
        except OSError:
            pass
        self.shutdown_request(request)
        self.log_access(client_address[0], f"503 every handler is busy ({MAX_HANDLERS})")


def _response(status: int, body: bytes, content_type: str = TEXT_TYPE) -> bytes:
    head = [f"HTTP/1.1 {status} {HTTPStatus(status).phrase}", f"Content-Type: {content_type}",
            f"Content-Length: {len(body)}", "Connection: close"]
    head += [f"{name}: {value}" for name, value in SECURITY_HEADERS]
    return ("\r\n".join(head) + "\r\n\r\n").encode("ascii") + body


class _Handler(socketserver.StreamRequestHandler):
    """Reads one bounded request, runs it through the gate, answers once and closes."""

    def setup(self) -> None:
        self.timeout = SOCKET_TIMEOUT_SECONDS
        super().setup()

    def handle(self) -> None:
        deadline = threading.Timer(SOCKET_TIMEOUT_SECONDS, self._abort)
        deadline.daemon = True
        deadline.start()
        self._summary = "-"
        try:
            self._answer(deadline)
        except _HttpError as error:
            self._finish(error.status, error.body, TEXT_TYPE, error.reason)
        except (_Drop, OSError):
            pass
        finally:
            deadline.cancel()

    def _abort(self) -> None:
        try:
            self.request.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

    # -- reading ------------------------------------------------------------------------------

    def _answer(self, deadline: threading.Timer) -> None:
        method, target, headers = self._read_head()
        self._summary = f"{method} {target}"
        form = self._read_body(method, headers, track_answer=target in TRACK_ANSWER_ROUTES)
        deadline.cancel()
        csrf = form.pop("csrf", "")
        request = gate.Request(method, self.client_address[0], tuple(headers), csrf)
        try:
            who = gate.check(request, self.server.settings, self.server.authorities, self.server.identify,
                             csrf_secret=self.server.csrf_secret, now=self.server.clock())
        except gate.Denied as denied:
            raise _HttpError(403, f"denied: {denied}", FORBIDDEN_BODY) from denied
        except Exception as error:  # a gate that breaks denies
            raise _HttpError(403, f"denied: gate failed: {type(error).__name__}", FORBIDDEN_BODY) from error
        self._route(method, target.split("?", 1)[0], form, who)

    def _read_head(self) -> tuple[str, str, list[tuple[str, str]]]:
        lines, used = [], 0
        while True:
            line = self.rfile.readline(MAX_HEADER_BYTES - used + 1)
            used += len(line)
            if used > MAX_HEADER_BYTES:
                raise _HttpError(431, f"request head over {MAX_HEADER_BYTES} bytes")
            if not line.endswith(b"\n"):
                raise _Drop()
            if line == b"\r\n":
                break
            lines.append(line)
            if len(lines) > MAX_HEADERS + 1:
                raise _HttpError(431, f"more than {MAX_HEADERS} headers")
        if not lines:
            raise _HttpError(400, "empty request")
        method, target = _parse_request_line(lines[0])
        return method, target, [_parse_header(line) for line in lines[1:]]

    def _read_body(self, method: str, headers: list[tuple[str, str]], *, track_answer: bool) -> dict:
        names = [name.lower() for name, _ in headers]
        if "transfer-encoding" in names:
            raise _HttpError(400, "Transfer-Encoding is not supported")
        lengths = [value for name, value in headers if name.lower() == "content-length"]
        if method != "POST":
            if lengths and lengths != ["0"]:
                raise _HttpError(400, f"{method} with a body")
            return {}
        size = _body_size(lengths, limit=MAX_TRACK_BODY_BYTES if track_answer else MAX_BODY_BYTES)
        if not FORM_TYPE.fullmatch(gate.single_header(headers, "content-type") or ""):
            raise _HttpError(415, "POST needs a form body")
        body = self.rfile.read(size)
        if len(body) != size:
            raise _Drop()
        form = _parse_form(body)
        if size > MAX_BODY_BYTES and form.get("key") != track_actions.SETTLE:
            raise _HttpError(413, f"body over {MAX_BODY_BYTES} bytes")
        return form

    # -- answering ----------------------------------------------------------------------------

    def _route(self, method: str, path: str, form: dict, who: gate.Identity) -> None:
        if path.startswith(API_PREFIX):
            self._route_api(method, path, form, who)
        elif method == "GET" and path == "/":
            self._finish(200, _text(self._produce(self.server.index_page, who, self._csrf_for(who))), HTML_TYPE)
        elif method == "GET" and path == STYLESHEET_PATH:
            self._finish(200, pages.STYLESHEET.read_bytes(), CSS_TYPE)
        elif method == "POST" and path == "/action":
            self._write(form, who, as_json=False)
        elif path in ("/", "/action", STYLESHEET_PATH):
            raise _HttpError(405, f"{method} not allowed on {path}")
        else:
            raise _HttpError(404, "no such page")

    def _route_api(self, method: str, path: str, form: dict, who: gate.Identity) -> None:
        api = self.server.api
        if api is None or path not in {served for _, served in API_ROUTES}:
            raise _HttpError(404, "no such page")
        if (method, path) not in API_ROUTES:
            raise _HttpError(405, f"{method} not allowed on {path}")
        if path == "/api/session":
            self._finish_json(200, self._produce(api.session, who, self._csrf_for(who)))
        elif path == "/api/snapshot":
            self._finish_json(200, self._produce(api.snapshot))
        else:
            self._write(form, who, as_json=True)

    def _write(self, form: dict, who: gate.Identity, *, as_json: bool) -> None:
        if who.node_id == self.server.own_node_id:  # an engine process on this machine looks like its owner
            self._refuse_write(OWN_NODE_REFUSAL, form, who, as_json)
        elif not self.server.action_slots.acquire(blocking=False):  # running actions must not starve the reads
            self._refuse_write(ACTION_BUSY_REFUSAL, form, who, as_json, html_status=HTTPStatus.SERVICE_UNAVAILABLE)
        else:
            try:
                self._run_write(form, who, as_json)
            finally:
                self.server.action_slots.release()

    def _run_write(self, form: dict, who: gate.Identity, as_json: bool) -> None:
        if as_json:
            status, document = self._produce(self.server.api.action, form, who)
            self._finish_json(status, document)
        else:
            self._finish(200, _text(self._produce(self.server.action_runner, form, who, self._csrf_for(who))),
                         HTML_TYPE)

    def _refuse_write(self, reason: str, form: dict, who: gate.Identity, as_json: bool,
                      html_status: int = HTTPStatus.OK) -> None:
        actions.log_refusal(self.server.action_log, str(form.get("key", "")), (), overlay_actor(who), reason)
        if as_json:
            self._finish_json(200, api_documents.action_document(Refused(reason)))
        else:
            self._finish(html_status, _text(pages.refused(reason)), HTML_TYPE, reason)

    def _finish_json(self, status: int, document: dict) -> None:
        self._finish(status, api_documents.dumps(document).encode("ascii"), JSON_TYPE)

    def _csrf_for(self, who: gate.Identity) -> str:
        return gate.csrf_token(self.server.csrf_secret, who, self.server.clock())

    @staticmethod
    def _produce(source: Callable[..., object], *arguments: object) -> object:
        try:
            return source(*arguments)
        except gate.Denied as denied:
            raise _HttpError(403, f"denied: {denied}", FORBIDDEN_BODY) from denied
        except Exception as error:  # the page never shows why; the log names the exception type
            raise _HttpError(500, f"handler failed: {type(error).__name__}") from error

    def _finish(self, status: int, body: bytes, content_type: str = TEXT_TYPE, reason: str = "") -> None:
        try:
            self.wfile.write(_response(status, body, content_type))
        except OSError:
            pass
        if status >= 400:
            self.server.log_access(self.client_address[0], f"{self._summary} {status} {reason}")


def _parse_request_line(line: bytes) -> tuple[str, str]:
    try:
        parts = line.decode("ascii").removesuffix("\r\n").split(" ")
    except UnicodeDecodeError:
        parts = []
    if len(parts) != 3 or not METHOD.fullmatch(parts[0]) or not TARGET.fullmatch(parts[1]):
        raise _HttpError(400, "malformed request line")
    if parts[2] not in HTTP_VERSIONS:
        raise _HttpError(505, "unsupported HTTP version")
    return parts[0], parts[1]


def _parse_header(line: bytes) -> tuple[str, str]:
    try:
        match = HEADER_LINE.fullmatch(line.decode("ascii").removesuffix("\r\n"))
    except UnicodeDecodeError:
        match = None
    if match is None:
        raise _HttpError(400, "malformed header line")
    return match.group(1), match.group(2)


def _body_size(lengths: list[str], *, limit: int = MAX_BODY_BYTES) -> int:
    if not lengths:
        raise _HttpError(411, "POST without Content-Length")
    if len(lengths) != 1 or not LENGTH.fullmatch(lengths[0]):
        raise _HttpError(400, "bad Content-Length")
    if int(lengths[0]) > limit:
        raise _HttpError(413, f"body over {limit} bytes")
    return int(lengths[0])


def _parse_form(body: bytes) -> dict:
    try:
        pairs = parse_qsl(body.decode("utf-8"), keep_blank_values=True, strict_parsing=True,
                          max_num_fields=MAX_FORM_FIELDS)
    except (UnicodeDecodeError, ValueError) as error:
        raise _HttpError(400, "malformed form body") from error
    form = dict(pairs)
    if len(form) != len(pairs):
        raise _HttpError(400, "repeated form field")
    return form


def _text(value: object) -> bytes:
    return strip_controls(str(value)).encode("utf-8", errors="replace")
