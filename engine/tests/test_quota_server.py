import io
import json
import threading
import urllib.error
import urllib.request
from http.client import HTTPResponse
from urllib.parse import urlsplit

import pytest
from quota_support import FakeRunner, clock, event

from sakura.accounts import registry
from sakura.quota import server
from sakura.quota.config import QuotaConfig
from sakura.quota.service import Service
from sakura.usage.archive import settings
from test_cockpit_server import Connection


@pytest.fixture
def served(tmp_path, monkeypatch):
    home = tmp_path / "isolated-home"
    work = home / ".claude-work"
    (work / "projects").mkdir(parents=True)
    config = home / ".config/sakura"
    config.mkdir(parents=True)
    (config / "accounts.toml").write_text(
        f'[[accounts]]\nprovider = "claude"\nid = "work"\nlabel = "Work"\nconfig_dir = "{work}"\nenabled = true\n')
    (config / "usage.toml").write_text(f'db = "{tmp_path / "usage.db"}"\n')
    run = FakeRunner({str(work): event(seven=0.5)})
    service = Service(settings.load(), registry.load(), QuotaConfig(), run, clock, home)
    class MemoryServer:
        def __init__(self, address, handler):
            self.server_address = (address[0], address[1] or 42003)
            self.handler = handler

        def open(self, request):
            uri = urlsplit(request.full_url)
            assert (uri.hostname, uri.port) == self.server_address
            headers = {"Host": uri.netloc, **dict(request.header_items())}
            body = request.data or b""
            if request.data is not None:
                headers["Content-Length"] = str(len(body))
            path = uri.path + (f"?{uri.query}" if uri.query else "")
            head = f"{request.get_method()} {path} HTTP/1.1\r\n"
            head += "".join(f"{name}: {value}\r\n" for name, value in headers.items())
            connection = Connection((head + "\r\n").encode() + body)
            self.handler(connection, (uri.hostname, 0), self)
            response = HTTPResponse(Connection(connection.output))
            response.begin()
            if response.status >= 400:
                raise urllib.error.HTTPError(request.full_url, response.status, response.reason, response.headers,
                                             io.BytesIO(response.read()))
            return response

    monkeypatch.setattr(server, "ThreadingHTTPServer", MemoryServer)
    httpd = server.make_server(service, 0)
    monkeypatch.setattr(urllib.request, "urlopen", httpd.open)
    return f"http://127.0.0.1:{httpd.server_address[1]}", httpd, service, run


def call(url, body=None):
    request = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(),
                                     method="GET" if body is None else "POST")
    try:
        with urllib.request.urlopen(request) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read())


def test_Server_BindsOnlyToLoopback(served):
    _, httpd, _, _ = served
    assert httpd.server_address[0] == "127.0.0.1"


def test_Status_ReturnsTheContractDocument(served):
    base, *_ = served
    code, document = call(f"{base}/status")
    assert code == 200 and document["accounts"][0]["seven_day"]["pct"] == 50.0


def test_Advise_PostedRequest_ReturnsAdvice(served):
    base, *_ = served
    body = {"repo": "/anywhere", "role": "builder", "task_class": "integration", "agent": "claude"}
    code, answer = call(f"{base}/advise", body)
    assert (code, answer["verdict"], answer["model"]) == (200, "go", "sonnet")


@pytest.mark.parametrize("body", [{"repo": "/x", "role": "boss", "task_class": "design"},
                                  {"repo": "", "role": "builder", "task_class": "design"},
                                  {"repo": "/x", "role": "builder", "task_class": "design", "agent": "gemini"}])
def test_Advise_InvalidRequest_Is400WithTheReason(served, body):
    code, answer = call(f"{served[0]}/advise", body)
    assert code == 400 and answer["error"]


def test_Advise_NotJson_Is400(served):
    request = urllib.request.Request(f"{served[0]}/advise", data=b"nope", method="POST")
    with pytest.raises(urllib.error.HTTPError) as caught:
        urllib.request.urlopen(request)
    assert caught.value.code == 400


def test_UnknownPath_Is404(served):
    assert call(f"{served[0]}/nothing")[0] == 404


def test_RefreshLoop_SurvivesAFailingTickAndStops(capsys):
    stop, ticks = threading.Event(), []

    class Broken:
        def status(self, refresh=False):
            ticks.append(refresh)
            if len(ticks) == 1:
                raise OSError("disk")
            stop.set()

    server.refresh_forever(Broken(), 0, stop)
    assert ticks == [True, True] and "quota refresh failed: OSError" in capsys.readouterr().err


def raw(base, method, path, headers=None, body=None):
    request = urllib.request.Request(base + path, data=body, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(request) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def test_Request_ForeignHostHeader_IsRefused(served):
    assert raw(served[0], "GET", "/status", {"Host": "evil.example:8799"}) == 403


def test_Request_WithAnOriginHeader_IsRefused(served):
    base = served[0]
    assert raw(base, "GET", "/status", {"Origin": "https://evil.example"}) == 403
    assert raw(base, "POST", "/advise", {"Origin": "https://evil.example"}, b"{}") == 403


def test_Request_LocalhostHostName_IsAccepted(served):
    port = served[1].server_address[1]
    assert raw(served[0], "GET", "/status", {"Host": f"localhost:{port}"}) == 200


def test_Refresh_OnlyByPost_AndGetNeverProbes(served):
    base, _, _, run = served
    raw(base, "GET", "/status")
    raw(base, "GET", "/status?refresh=1")
    probes = len(run.probed())
    assert probes == 1
    assert raw(base, "GET", "/refresh") == 404
    assert raw(base, "POST", "/refresh", body=b"") == 200
    assert len(run.probed()) == probes + 1
