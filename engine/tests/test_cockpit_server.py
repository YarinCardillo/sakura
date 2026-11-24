import io
import re
import socket
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urlencode

import pytest

from sakura.cockpit import gate, pages, server
from sakura.cockpit.settings import CockpitRefused, CockpitSettings
from cockpit_support import OWN_DNS_NAME, write_tailscale

SECRET = b"k" * 32
OWNER = gate.Identity("owner@example.com", "nMAC", "mac")
LOOPBACK = "127.0.0.1"
FORBIDDEN_BODY = b"forbidden\n"
SECURITY_HEADERS = {
    "content-security-policy": "default-src 'none'; style-src 'self'; form-action 'self'; frame-ancestors 'none'",
    "x-frame-options": "DENY",
    "referrer-policy": "no-referrer",
    "cache-control": "no-store",
}


class LoopbackServer(server.CockpitServer):
    """The test seam: only a test subclass can bind anywhere but the overlay address."""

    @staticmethod
    def _check_bind_address(address):
        return None


class MemoryServer(LoopbackServer):
    def server_bind(self):
        self.server_address = (self.server_address[0], self.server_address[1] or 8787)

    def server_activate(self):
        pass

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            request.done.set()

    def _refuse_overload(self, request, address):
        try:
            super()._refuse_overload(request, address)
        finally:
            request.done.set()

    def submit(self, connection):
        self.process_request(connection, (LOOPBACK, 0))
        return connection


class Connection:
    def __init__(self, raw=b"", *, read=None):
        self.input = io.BytesIO(raw) if read is None else SimpleNamespace(readline=read, close=lambda: None)
        self.output = bytearray()
        self.done = threading.Event()
        self.timeout = None
        self.aborted = False

    def settimeout(self, timeout):
        self.timeout = timeout

    def makefile(self, mode, _buffering=-1):
        assert mode == "rb"
        return self.input

    def sendall(self, data):
        self.output.extend(data)

    def shutdown(self, how):
        if how == socket.SHUT_RDWR:
            self.aborted = True

    def close(self):
        pass


class Probe:
    """Counts calls and the peak of simultaneous ones."""

    def __init__(self, result=None, delay=0.0):
        self.result, self.delay = result, delay
        self.lock = threading.Lock()
        self.active = self.peak = self.calls = 0

    def __call__(self, *args, **kwargs):
        with self.lock:
            self.active += 1
            self.calls += 1
            self.peak = max(self.peak, self.active)
        try:
            time.sleep(self.delay)
            return self.result(*args, **kwargs) if callable(self.result) else self.result
        finally:
            with self.lock:
                self.active -= 1


@pytest.fixture
def start(tmp_path):
    running = []

    def _start(*, identify=lambda _ip: OWNER, snapshot="all quiet", runner=None, settings=None, **kwargs):
        settings = settings or CockpitSettings(0, frozenset({OWNER.login}), frozenset({OWNER.node_id}),
                                               tmp_path / "tailscale")
        runner = runner or Probe("ran\n")
        instance = MemoryServer(gate.Self(LOOPBACK, OWN_DNS_NAME), settings, snapshot if callable(snapshot) else
                                  (lambda identity, csrf: snapshot), runner, identify=identify, access_log=tmp_path / "access.log",
                                  csrf_secret=SECRET, **kwargs)
        running.append(instance)
        instance.runner, instance.log = runner, tmp_path / "access.log"
        instance.port = instance.server_address[1]
        instance.host = f"{LOOPBACK}:{instance.port}"
        return instance

    yield _start
    for instance in running:
        instance.server_close()


def _exchange(instance, raw):
    """Pass raw HTTP through the real dispatcher and wait for the handler to release its slot."""
    httpd = getattr(instance, "server", instance)
    connection = httpd.submit(Connection(raw))
    assert connection.done.wait(10)
    data = bytes(connection.output)
    head, _, body = data.partition(b"\r\n\r\n")
    lines = head.decode("latin-1").split("\r\n")
    status = int(lines[0].split()[1]) if lines and lines[0] else 0
    headers = {line.split(":", 1)[0].lower(): line.split(":", 1)[1].strip() for line in lines[1:] if ":" in line}
    return status, headers, body


def _get(instance, path="/", host=None, extra=""):
    raw = f"GET {path} HTTP/1.1\r\nHost: {host or instance.host}\r\n{extra}\r\n".encode()
    return _exchange(instance, raw)


def _post(instance, fields, *, host=None, origin=None, csrf=True, path="/action", content_type=None, extra=""):
    fields = dict(fields)
    if csrf is True:
        fields["csrf"] = gate.csrf_token(SECRET, OWNER, getattr(instance, "server", instance).clock())
    elif csrf:
        fields["csrf"] = csrf
    body = urlencode(fields).encode()
    origin = origin if origin is not None else f"http://{instance.host}"
    head = (f"POST {path} HTTP/1.1\r\nHost: {host or instance.host}\r\nOrigin: {origin}\r\n"
            f"Content-Type: {content_type or 'application/x-www-form-urlencoded'}\r\nContent-Length: {len(body)}\r\n"
            f"{extra}\r\n")
    return _exchange(instance, head.encode() + body)


# -- constants and addresses --------------------------------------------------------------------

def test_limits_are_the_documented_ones():
    assert (server.SOCKET_TIMEOUT_SECONDS, server.MAX_HEADER_BYTES, server.MAX_HEADERS) == (10, 8192, 50)
    assert (server.MAX_BODY_BYTES, server.MAX_HANDLERS) == (16384, 8)


@pytest.mark.parametrize("address", ["0.0.0.0", "127.0.0.1", "::", "::1", "", "localhost", "192.168.1.5", "100.63.0.1"])
def test_never_binds_wildcard_or_loopback(tmp_path, address):
    settings = CockpitSettings(8787, frozenset({"a"}), frozenset({"b"}), tmp_path / "tailscale")
    with pytest.raises(CockpitRefused, match="overlay"):
        server.CockpitServer(gate.Self(address, OWN_DNS_NAME), settings, lambda: "", lambda *_: "")


def test_serve_refuses_a_helper_that_reports_a_non_overlay_address(tmp_path):
    for address in ("0.0.0.0", "127.0.0.1"):
        helper = write_tailscale(tmp_path, address=address)
        settings = CockpitSettings(8787, frozenset({"a"}), frozenset({"b"}), helper)
        with pytest.raises(CockpitRefused, match="overlay"):
            server.serve(settings, lambda: "", lambda *_: "")


def test_production_sources_hold_no_wildcard_or_loopback_literal():
    sources = Path(server.__file__).parent.glob("*.py")
    forbidden = re.compile(r"0\.0\.0\.0|127\.\d+\.\d+\.\d+|localhost|::1|INADDR_ANY|bind\(\s*\(\s*[\"']{2}")
    for source in sources:
        assert not forbidden.search(source.read_text()), source.name


# -- serving ------------------------------------------------------------------------------------

def test_authorized_get_is_served_as_html_without_controls(start):
    instance = start(snapshot="<p>line one\x1b]2;evil\x07 two</p>\n")
    status, headers, body = _get(instance)
    assert status == 200
    assert headers["content-type"] == "text/html; charset=utf-8"
    assert body == b"<p>line one two</p>\n"


def test_a_lone_surrogate_in_the_page_is_dropped_not_a_dropped_connection(start):
    instance = start(snapshot="<p>task a\udcffb</p>\n")
    status, _headers, body = _get(instance)
    assert status == 200 and body == b"<p>task ab</p>\n"


def test_index_page_receives_the_identity_and_a_csrf_token_for_it(start):
    seen = []
    instance = start(snapshot=lambda identity, csrf: seen.append((identity, csrf)) or "page\n")
    assert _get(instance)[0] == 200
    (identity, token), = seen
    assert identity == OWNER and gate.csrf_valid(SECRET, token, OWNER, time.time())


def test_stylesheet_is_served_through_the_same_gate(start):
    instance = start()
    status, headers, body = _get(instance, path="/style.css")
    assert (status, headers["content-type"], body) == (200, "text/css; charset=utf-8", pages.STYLESHEET.read_bytes())
    assert headers["content-security-policy"] == SECURITY_HEADERS["content-security-policy"]
    assert _get(instance, path="/style.css", host="evil.example")[0] == 403


def test_security_headers_on_every_response(start):
    instance = start()
    responses = [
        _get(instance),
        _get(instance, path="/style.css"),
        _get(instance, path="/nowhere"),
        _get(instance, host="evil.example"),
        _exchange(instance, f"DELETE / HTTP/1.1\r\nHost: {instance.host}\r\n\r\n".encode()),
        _exchange(instance, f"POST /action HTTP/1.1\r\nHost: {instance.host}\r\nContent-Length: 99999\r\n\r\n".encode()),
        _exchange(instance, b"GET / HTTP/1.1\r\n" + b"X: y\r\n" * 60 + b"\r\n"),
        _exchange(instance, b"garbage\r\n\r\n"),
    ]
    assert [item[0] for item in responses] == [200, 200, 404, 403, 405, 413, 431, 400]
    for _status, headers, _body in responses:
        for name, value in SECURITY_HEADERS.items():
            assert headers[name] == value, name


def test_security_headers_on_the_overload_response(start, monkeypatch):
    monkeypatch.setattr(server, "MAX_HANDLERS", 1)
    instance = start()
    occupied = threading.Event()
    release = threading.Event()
    blocker = instance.submit(Connection(read=_hold_read(occupied.set, release)))
    try:
        assert occupied.wait(10)
        status, headers, _ = _get(instance)
        assert status == 503
        for name, value in SECURITY_HEADERS.items():
            assert headers[name] == value
    finally:
        release.set()
        assert blocker.done.wait(10)


def _hold_read(entered, release):
    def read(_size):
        entered()
        assert release.wait(10)
        return b""
    return read


# -- denials ------------------------------------------------------------------------------------

def test_denial_is_a_fixed_403_and_a_log_line(start):
    instance = start(identify=lambda _ip: None)
    status, _, body = _get(instance, path="/secret?token=1")
    assert (status, body) == (403, FORBIDDEN_BODY)
    line = instance.log.read_text().strip()
    assert "unidentified" in line and "GET" in line and LOOPBACK in line


def test_log_lines_carry_no_terminal_controls_from_the_request(start):
    instance = start(identify=lambda _ip: None)
    _exchange(instance, b"GET /a\x1b]2;pwn\x07b HTTP/1.1\r\nHost: x\r\n\r\n")
    _exchange(instance, b"GET /\x9b31m HTTP/1.1\r\nHost: x\r\nX-A: \x1b[2J\r\n\r\n")
    text = instance.log.read_text()
    assert text and not re.search(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]", text.replace("\n", ""))


def test_default_identifier_fails_closed_for_a_non_overlay_peer(start, tmp_path):
    marker = tmp_path / "helper-ran"
    helper = write_tailscale(tmp_path, marker=marker)
    settings = CockpitSettings(0, frozenset({OWNER.login}), frozenset({OWNER.node_id}), helper)
    instance = start(identify=None, settings=settings)
    assert _get(instance)[0] == 403
    assert not marker.exists()


def test_an_identifier_that_raises_denies(start):
    def broken(_ip):
        raise RuntimeError("whois exploded")
    instance = start(identify=broken)
    status, _, body = _get(instance)
    assert (status, body) == (403, FORBIDDEN_BODY)
    assert "RuntimeError" in instance.log.read_text()


def test_wrong_host_is_denied(start):
    instance = start()
    assert _get(instance, host=f"evil.example:{instance.port}")[0] == 403
    assert _get(instance, extra=f"Host: {instance.host}\r\n")[0] == 403


# -- POST ---------------------------------------------------------------------------------------

def test_valid_post_runs_the_action_runner_once_with_the_identity(start):
    instance = start(runner=Probe("did it\x1b[2J\n"))
    status, _, body = _post(instance, {"key": "s", "target": "t"})
    assert (status, body) == (200, b"did it\n")
    assert instance.runner.calls == 1


def test_post_runner_receives_fields_and_identity(start):
    seen = []
    instance = start(runner=lambda fields, identity, csrf: seen.append((fields, identity, csrf)) or "ok\n")
    _post(instance, {"key": "s", "text": "a;b 'c'\nd"})
    fields, identity, token = seen[0]
    assert fields == {"key": "s", "text": "a;b 'c'\nd"} and identity == OWNER
    assert gate.csrf_valid(SECRET, token, OWNER, time.time())  # fresh, for the form on the page that comes back


def test_a_runner_that_denies_is_a_403_with_the_fixed_body_and_a_log_line(start):
    def deny(fields, identity, csrf):
        raise gate.Denied("confirmation proof rejected")

    instance = start(runner=deny)
    status, _, body = _post(instance, {"key": "s"})
    assert (status, body) == (403, FORBIDDEN_BODY)
    assert "confirmation proof rejected" in instance.log.read_text()


@pytest.mark.parametrize("variant", [
    {"csrf": False}, {"csrf": "forged"}, {"origin": "http://evil.example"}, {"origin": "null"},
    {"host": "evil.example"},
])
def test_post_without_proof_of_intent_never_reaches_the_runner(start, variant):
    instance = start()
    status, _, body = _post(instance, {"key": "s"}, **variant)
    assert (status, body) == (403, FORBIDDEN_BODY)
    assert instance.runner.calls == 0


def test_post_with_a_token_of_another_node_is_denied(start):
    instance = start()
    token = gate.csrf_token(SECRET, gate.Identity(OWNER.login, "nPHONE", "phone"), time.time())
    assert _post(instance, {"key": "s"}, csrf=token)[0] == 403
    assert instance.runner.calls == 0


def test_post_shape_errors(start):
    instance = start()
    assert _post(instance, {"key": "s"}, content_type="text/plain")[0] == 415
    assert _post(instance, {"key": "s"}, path="/elsewhere")[0] == 404
    no_length = f"POST /action HTTP/1.1\r\nHost: {instance.host}\r\nOrigin: http://{instance.host}\r\n\r\n".encode()
    assert _exchange(instance, no_length)[0] == 411
    chunked = no_length[:-2] + b"Transfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n"
    assert _exchange(instance, chunked)[0] == 400
    assert instance.runner.calls == 0


@pytest.mark.parametrize("content_type", [
    "application/x-www-form-urlencoded", "application/x-www-form-urlencoded;charset=UTF-8",
    "application/x-www-form-urlencoded; charset=utf-8", 'Application/X-WWW-Form-Urlencoded ; charset = "UTF-8" ',
])
def test_post_accepts_the_form_type_bare_or_with_only_a_utf8_charset(start, content_type):
    instance = start()
    assert _post(instance, {"key": "s"}, content_type=content_type)[0] == 200
    assert instance.runner.calls == 1


@pytest.mark.parametrize("content_type", [
    "application/x-www-form-urlencoded; charset=latin-1", "application/x-www-form-urlencoded; boundary=x",
    "application/x-www-form-urlencoded; charset=utf-8; foo=1", "application/x-www-form-urlencoded;",
    "application/x-www-form-urlencoded-x", "multipart/form-data; charset=utf-8", "text/plain;charset=utf-8",
])
def test_post_refuses_any_other_content_type_or_parameter(start, content_type):
    instance = start()
    assert _post(instance, {"key": "s"}, content_type=content_type)[0] == 415
    assert instance.runner.calls == 0


def test_a_failing_runner_is_a_500_without_details(start):
    def boom(fields, identity, csrf):
        raise RuntimeError("secret detail /home/someone")
    instance = start(runner=boom)
    status, _, body = _post(instance, {"key": "s"})
    assert status == 500 and b"secret" not in body and b"Users" not in body


# -- limits -------------------------------------------------------------------------------------

def test_oversized_body_is_refused_before_it_is_read(start):
    instance = start()
    head = (f"POST /action HTTP/1.1\r\nHost: {instance.host}\r\nOrigin: http://{instance.host}\r\n"
            f"Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {server.MAX_BODY_BYTES + 1}\r\n\r\n")
    assert _exchange(instance, head.encode())[0] == 413
    assert instance.runner.calls == 0


def test_body_at_the_limit_is_read(start):
    instance = start()
    pad = "a" * (server.MAX_BODY_BYTES - len("x=") - len("&csrf=") - len(gate.csrf_token(SECRET, OWNER, time.time())))
    assert _post(instance, {"x": pad})[0] == 200


def test_too_many_headers_or_header_bytes_fail_closed(start):
    instance = start()
    many = b"GET / HTTP/1.1\r\nHost: " + instance.host.encode() + b"\r\n" + b"X: y\r\n" * server.MAX_HEADERS + b"\r\n"
    assert _exchange(instance, many)[0] == 431
    big = f"GET / HTTP/1.1\r\nHost: {instance.host}\r\nX: {'a' * server.MAX_HEADER_BYTES}\r\n\r\n".encode()
    assert _exchange(instance, big)[0] == 431
    exact = b"GET / HTTP/1.1\r\nHost: " + instance.host.encode() + b"\r\n" + b"X: y\r\n" * (server.MAX_HEADERS - 1) + b"\r\n"
    assert _exchange(instance, exact)[0] == 200
    assert instance.runner.calls == 0


def test_idle_client_is_dropped_at_the_socket_timeout(start):
    instance = start()
    read_timeouts = []

    def idle_read(_size):
        read_timeouts.append(connection.timeout)
        raise TimeoutError

    connection = Connection(read=idle_read)
    instance.submit(connection)
    assert connection.done.wait(10)
    assert read_timeouts == [server.SOCKET_TIMEOUT_SECONDS]
    assert connection.output == b""


def test_a_client_dripping_bytes_is_dropped_at_the_overall_deadline(start, monkeypatch):
    instance = start()
    timers = []
    elapsed = 0

    class Deadline:
        def __init__(self, delay, abort):
            self.delay, self.abort = delay, abort
            self.active = False
            timers.append(self)

        def start(self):
            self.active = True

        def cancel(self):
            self.active = False

    def drip(_size):
        nonlocal elapsed
        delay = server.SOCKET_TIMEOUT_SECONDS // 2
        for _ in range(3):
            elapsed += delay
            for timer in timers:
                if timer.active and elapsed >= timer.delay:
                    timer.abort()
            if connection.aborted:
                return b"G"
        return b"GET / HTTP/1.1\r\n"

    monkeypatch.setattr(server, "threading", SimpleNamespace(Timer=Deadline))
    connection = Connection(read=drip)
    instance.submit(connection)
    assert connection.done.wait(10)
    assert [timer.delay for timer in timers] == [server.SOCKET_TIMEOUT_SECONDS]
    assert connection.aborted and connection.output == b""
    assert elapsed == server.SOCKET_TIMEOUT_SECONDS


def test_limits_fail_closed_and_owner_still_served(start):
    identify = Probe(lambda _ip: OWNER)
    instance = start(identify=identify)
    occupied = threading.Semaphore(0)
    release = threading.Event()
    slow = [instance.submit(Connection(read=_hold_read(occupied.release, release)))
            for _ in range(server.MAX_HANDLERS)]
    try:
        for _ in slow:
            assert occupied.acquire(timeout=10)
        assert _get(instance)[0] == 503
        assert identify.calls == 0 and instance.runner.calls == 0
    finally:
        release.set()
        for connection in slow:
            assert connection.done.wait(10)
    assert _get(instance)[0] == 200


def test_hung_identification_never_exceeds_the_handler_limit(start):
    hung = Probe(lambda _ip: None, delay=0.8)
    instance = start(identify=hung)
    results = []
    threads = [threading.Thread(target=lambda: results.append(_get(instance)[0])) for _ in range(20)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(15)
    assert hung.peak <= server.MAX_HANDLERS
    assert set(results) <= {403, 503} and 403 in results
    assert instance.runner.calls == 0


def test_requests_after_a_flood_are_served(start):
    hung = Probe(lambda _ip: None, delay=0.3)
    instance = start(identify=hung)
    flood = [threading.Thread(target=_get, args=(instance,)) for _ in range(20)]
    for thread in flood:
        thread.start()
    for thread in flood:
        thread.join(15)
    hung.result = lambda _ip: OWNER
    hung.delay = 0
    assert _get(instance)[0] == 200


# -- the access log is private ------------------------------------------------------------------

def test_access_log_is_created_0600_in_a_0700_directory(tmp_path):
    log = tmp_path / "state" / "cockpit" / "access.log"
    settings = CockpitSettings(0, frozenset({OWNER.login}), frozenset({OWNER.node_id}), tmp_path / "tailscale")
    instance = MemoryServer(gate.Self(LOOPBACK, OWN_DNS_NAME), settings, lambda *_: "", lambda *_: "",
                              access_log=log, csrf_secret=SECRET)
    try:
        instance.log_access("100.64.0.9", "GET / 403 denied")
    finally:
        instance.server_close()
    assert log.read_text().endswith("GET / 403 denied\n")
    assert log.stat().st_mode & 0o777 == 0o600
    assert log.parent.stat().st_mode & 0o777 == 0o700
