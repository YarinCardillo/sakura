"""Local HTTP for the quota service: `GET /status`, `POST /advise` and `POST /refresh`, JSON, on 127.0.0.1 only.

A web page must not be able to reach it through the browser: a request whose Host is not the loopback address
(DNS rebinding) or that carries an Origin header (any cross-site request; local clients send none) is refused,
and the only call that spends quota, `/refresh`, is a POST.

A background thread refreshes every source on a timer so requests find fresh snapshots; a failed refresh is
logged to stderr and retried at the next tick, it never stops the server.
"""

from __future__ import annotations

import json
import sys
import threading
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

from sakura.quota import advise
from sakura.quota.service import Service

HOST = "127.0.0.1"
SECONDS_PER_MINUTE = 60
MAX_BODY_BYTES = 8192
LOOPBACK_HOSTS = ("127.0.0.1", "localhost")


class BadRequest(ValueError):
    """The request body is not a valid advice request; the message is safe to return to the caller."""


def parse_request(body: bytes) -> advise.Request:
    """The advice request in the JSON `body`. Raises BadRequest naming the problem."""
    try:
        raw = json.loads(body)
    except ValueError as error:
        raise BadRequest("body must be JSON") from error
    if not isinstance(raw, dict):
        raise BadRequest("body must be a JSON object")
    repo, role, task_class, agent = (raw.get(key) for key in ("repo", "role", "task_class", "agent"))
    for key, value, allowed in (("role", role, advise.ROLES), ("task_class", task_class, advise.TASK_CLASSES)):
        if value not in allowed:
            raise BadRequest(f"{key} must be one of {', '.join(allowed)}")
    if not isinstance(repo, str) or not repo:
        raise BadRequest("repo must be a path")
    if agent not in (None, "claude", "codex"):
        raise BadRequest("agent must be claude or codex")
    return advise.Request(Path(repo).expanduser(), role, task_class, agent)


def make_server(service: Service, port: int) -> ThreadingHTTPServer:
    """A server bound to 127.0.0.1:`port` (0 picks a free port) that answers from `service`."""

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            if not self._is_local_caller():
                return self._send(HTTPStatus.FORBIDDEN, {"error": "forbidden"})
            if urlparse(self.path).path != "/status":
                return self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})
            self._send(HTTPStatus.OK, service.status())

        def do_POST(self) -> None:
            if not self._is_local_caller():
                return self._send(HTTPStatus.FORBIDDEN, {"error": "forbidden"})
            path = urlparse(self.path).path
            if path == "/refresh":
                return self._send(HTTPStatus.OK, service.status(refresh=True))
            if path != "/advise":
                return self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})
            try:
                request = parse_request(self._body())
            except BadRequest as error:
                return self._send(HTTPStatus.BAD_REQUEST, {"error": str(error)})
            self._send(HTTPStatus.OK, service.advise(request))

        def _is_local_caller(self) -> bool:
            host = (self.headers.get("Host") or "").rpartition(":")[0]
            return host in LOOPBACK_HOSTS and self.headers.get("Origin") is None

        def _body(self) -> bytes:
            length = int(self.headers.get("Content-Length") or 0)
            return self.rfile.read(min(length, MAX_BODY_BYTES))

        def _send(self, code: HTTPStatus, payload: dict) -> None:
            data = json.dumps(payload).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format: str, *args) -> None:
            return None

    return ThreadingHTTPServer((HOST, port), Handler)


def refresh_forever(service: Service, minutes: int, stop: threading.Event) -> None:
    """Refresh every source each `minutes` until `stop` is set; errors are reported and survived."""
    while not stop.is_set():
        try:
            service.status(refresh=True)
        except Exception as error:  # a failed tick must not end the loop; the next one retries
            print(f"quota refresh failed: {type(error).__name__}: {error}", file=sys.stderr)
        stop.wait(minutes * SECONDS_PER_MINUTE)


def run(service: Service, port: int, refresh_minutes: int) -> int:
    """Serve until interrupted."""
    server, stop = make_server(service, port), threading.Event()
    threading.Thread(target=refresh_forever, args=(service, refresh_minutes, stop), daemon=True).start()
    print(f"quota service on http://{HOST}:{server.server_address[1]}", file=sys.stderr, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        server.server_close()
    return 0
