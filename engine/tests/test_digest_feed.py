"""Feed sources: Atom and RSS read with the standard library."""


import http.server
import socket
import sys
import threading

import pytest

from sakura.digest import config, feed, fetch_worker, intake, schema

ATOM = b"""<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry><id>tag:example.org,1</id><title>Old release</title><updated>2026-11-10T10:00:00Z</updated>
    <link rel="alternate" href="https://example.org/old"/><summary>Old</summary></entry>
  <entry><id>tag:example.org,2</id><title>New release</title><updated>2026-11-17T05:00:00+01:00</updated>
    <link href="https://example.org/new"/><content type="html">&lt;p&gt;Fixes &amp;amp; more&lt;/p&gt;</content></entry>
</feed>"""

RSS = b"""<?xml version="1.0"?>
<rss version="2.0"><channel>
  <item><guid>g1</guid><title>Old post</title><link>https://example.org/p1</link>
    <pubDate>Tue, 10 Nov 2026 10:00:00 GMT</pubDate><description>Old</description></item>
  <item><guid>g2</guid><title>New post</title><link>https://example.org/p2</link>
    <pubDate>Tue, 17 Nov 2026 05:00:00 +0000</pubDate><description>&lt;b&gt;Fresh&lt;/b&gt; news</description></item>
  <item><title>Undated</title><link>https://example.org/p3</link></item>
</channel></rss>"""

SINCE = "2026-11-16T06:30:00+00:00"


def test_Feed_AtomAndRssSinceCursor():
    found, skipped, newest = feed.entries(ATOM, SINCE, "")
    assert [item["title"] for item in found] == ["New release"] and skipped == 0
    assert found[0]["text"] == "Fixes & more" and found[0]["url"] == "https://example.org/new"
    assert newest == "2026-11-17T04:00:00+00:00"
    found, _, newest = feed.entries(RSS, SINCE, "")
    assert [item["title"] for item in found] == ["New post", "Undated"] and found[0]["text"] == "Fresh news"
    assert newest == "2026-11-17T05:00:00+00:00"
    # With the cursor of the last run, only what is newer than it comes back, and an undated entry never does.
    assert feed.entries(RSS, SINCE, newest)[0] == []
    assert feed.entries(ATOM, SINCE, "2026-11-10T10:00:00+00:00")[0][0]["id"] == "tag:example.org,2"


@pytest.mark.parametrize("body", [b"not xml at all", b"<html><body/></html>",
                                  b'<!DOCTYPE feed [<!ENTITY a "aaa">]><feed xmlns="http://www.w3.org/2005/Atom"/>'])
def test_Feed_NotAFeedOrWithADeclaration_Refused(body):
    with pytest.raises(feed.FeedError):
        feed.entries(body, SINCE, "")


def test_Feed_SourceThroughTheIntake_ReportsAndFailsAlone():
    source = config.Source("releases", feed="https://example.org/releases.atom")
    result = intake.run_source(source, SINCE, "", fetch=lambda url, timeout: ATOM)
    assert result.report == schema.SourceReport("releases", True, 1, 0) and result.cursor == "2026-11-17T04:00:00+00:00"

    def broken(url, timeout):
        raise feed.FeedError("cannot fetch the feed: down")

    failed = intake.run_source(source, SINCE, "", fetch=broken)
    assert not failed.report.ok and failed.report.error == "cannot fetch the feed: down"


def test_Feed_EntryWithBadLink_SkippedAndCounted():
    body = ATOM.replace(b"https://example.org/new", b"javascript:alert(1)")
    found, skipped, _ = feed.entries(body, SINCE, "")
    assert found == [] and skipped == 1


# Hardening: documents, deadlines and error text

DOCTYPE = '<!DOCTYPE feed [<!ENTITY a "aaa">]>'
FEED = '<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>x</id><title>t</title></entry></feed>'


@pytest.mark.parametrize("document", [
    ("<?xml version='1.0' encoding='utf-16'?>" + DOCTYPE + FEED).encode("utf-16"),
    ("<?xml version='1.0'?><!--" + "x" * 20000 + "-->" + DOCTYPE + FEED).encode(),
    ("<?xml version='1.0'?>\n\n   " + DOCTYPE + FEED).encode("utf-8"),
    ("<?xml version='1.0'?><!DOCTYPE feed SYSTEM 'http://example.org/x.dtd'>" + FEED).encode(),
], ids=["after-utf16-bom", "after-long-comment", "after-whitespace", "external-subset"])
def test_Feed_DoctypeAnywhereInAnyEncoding_Refused(document):
    with pytest.raises(feed.FeedError, match="document type"):
        feed.entries(document, SINCE, "")


def test_Feed_Utf16FeedWithoutDoctype_StillRead():
    body = ("<?xml version='1.0' encoding='utf-16'?>" + FEED).encode("utf-16")
    assert feed.entries(body, SINCE, "")[0][0]["id"] == "x"


class _Server:
    """A local HTTP server whose handler is a function of the request handler."""

    def __init__(self, respond):
        self.requests, self.stop = 0, threading.Event()
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                outer.requests += 1
                respond(self, outer)

            def log_message(self, *args):
                pass

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.httpd.daemon_threads = True
        self.port = self.httpd.server_address[1]
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.stop.set()
        self.httpd.shutdown()
        self.httpd.server_close()


def test_Feed_Fetch_TrickleEndsAtTheTotalDeadline(supervised_child):
    supervised_child.deadline = 2
    supervised_child.output[0] = [b" "] * 4
    supervised_child.seconds_per_read = 0.25
    with pytest.raises(feed.FeedError, match="time"):
        feed.fetch("http://127.0.0.1:42004/feed", 2)
    assert supervised_child.elapsed == pytest.approx(2)
    assert not supervised_child.group_alive


def test_Feed_Fetch_RedirectsAreCapped():
    def loop(handler, server):
        handler.send_response(302)
        handler.send_header("Location", f"http://127.0.0.1:{server.port}/again")
        handler.end_headers()

    with _Server(loop) as server:
        with pytest.raises(feed.FeedError):
            feed.fetch(f"http://127.0.0.1:{server.port}/feed", 5)
        assert server.requests <= fetch_worker.MAX_REDIRECTS + 1


def test_Feed_Fetch_RedirectToAnotherSchemeRefused():
    def to_file(handler, server):
        handler.send_response(302)
        handler.send_header("Location", "file:///etc/passwd")
        handler.end_headers()

    with _Server(to_file) as server:
        with pytest.raises(feed.FeedError, match="HTTP 302"):
            feed.fetch(f"http://127.0.0.1:{server.port}/feed", 5)


def test_Feed_Fetch_BodyOverTheCap_Refused(monkeypatch):
    monkeypatch.setattr(schema, "MAX_SOURCE_BYTES", 1000)

    def big(handler, server):
        handler.send_response(200)
        handler.send_header("Content-Length", "5000")
        handler.end_headers()
        handler.wfile.write(b"x" * 5000)

    with _Server(big) as server:
        with pytest.raises(feed.FeedError, match="larger than 1000 bytes"):
            feed.fetch(f"http://127.0.0.1:{server.port}/feed", 5)


def test_Feed_Errors_NeverCarryTheUrlOrItsSecrets():
    def broken(handler, server):
        handler.send_response(500, "server says token=SECRET")
        handler.end_headers()

    with _Server(broken) as server:
        for url in (f"http://127.0.0.1:{server.port}/feed?token=SECRET",
                    f"http://user:hunter2@127.0.0.1:{server.port}/feed?token=SECRET", "http://user:hunter2@127.0.0.1:1/feed?token=SECRET"):
            source = config.Source("releases", feed=url)
            result = intake.run_source(source, SINCE, "")
            report = result.report.error
            assert not result.report.ok and report
            assert not any(secret in report for secret in ("SECRET", "hunter2", "user", "token="))
        http_error = intake.run_source(config.Source("r", feed=f"http://127.0.0.1:{server.port}/feed?token=SECRET"), SINCE, "")
        assert "HTTP 500" in http_error.report.error


def test_Feed_ShownUrl_HasNoUserinfoOrQuery():
    assert feed.shown_url("https://user:pw@example.org:8080/a/feed.xml?token=T#frag") == "https://example.org:8080/a/feed.xml"


def test_Config_FeedWithUserinfo_Refused():
    for url in ("https://user:pw@example.org/feed", "https://user@example.org/feed"):
        with pytest.raises(config.ConfigError, match="credentials"):
            config.parse({"source": [{"id": "a", "feed": url}]})
    assert config.parse({"source": [{"id": "a", "feed": "https://example.org/feed?token=T"}]}).sources[0].feed


# Review 22: slow headers and redirects

def test_Feed_Fetch_SlowHeadersEndAtTheTotalDeadline(supervised_child):
    supervised_child.deadline = 2
    with pytest.raises(feed.FeedError, match="time"):
        feed.fetch("http://127.0.0.1:42004/feed", 2)
    assert supervised_child.elapsed == pytest.approx(2)
    assert not supervised_child.group_alive


def test_Feed_Fetch_RedirectToAnotherHostOrPort_NeverRequested():
    with _Server(lambda handler, server: handler.send_response(200) or handler.end_headers()) as target:
        def away(handler, server):
            handler.send_response(302)
            handler.send_header("Location", f"http://localhost:{target.port}/steal")
            handler.end_headers()

        with _Server(away) as origin:
            with pytest.raises(feed.FeedError, match="redirect"):
                feed.fetch(f"http://127.0.0.1:{origin.port}/feed", 5)
        assert target.requests == 0


def test_Feed_Fetch_RedirectWithinTheSameOrigin_Followed():
    def respond(handler, server):
        if handler.path == "/feed":
            handler.send_response(301)
            handler.send_header("Location", "/moved/feed.xml")
            handler.end_headers()
        else:
            body = FEED.encode()
            handler.send_response(200)
            handler.send_header("Content-Length", str(len(body)))
            handler.end_headers()
            handler.wfile.write(body)

    with _Server(respond) as server:
        assert feed.fetch(f"http://127.0.0.1:{server.port}/feed", 5) == FEED.encode()


@pytest.mark.parametrize("configured, target, allowed", [
    ("https://feeds.example/a", "https://feeds.example/b?x=1", True),
    ("https://feeds.example/a", "https://feeds.example:443/b", True),
    ("https://feeds.example/a", "http://feeds.example/a", False),          # never https to http
    ("http://feeds.example/a", "https://feeds.example/a", False),          # another origin, even an upgrade
    ("https://feeds.example/a", "https://other.example/a", False),
    ("https://feeds.example/a", "https://feeds.example:8443/a", False),
    ("https://feeds.example/a", "https://localhost/a", False),
    ("https://feeds.example/a", "https://127.0.0.1/a", False),
    ("https://feeds.example/a", "https://169.254.169.254/latest", False),
    ("https://feeds.example/a", "https://[::1]/a", False),
])
def test_FetchWorker_RedirectPolicy_SameOriginOnly(configured, target, allowed):
    public = lambda host, port, **kw: [(2, 1, 6, "", ("93.184.216.34", port or 0))]  # noqa: E731
    assert fetch_worker.redirect_allowed(configured, target, public) is allowed


def test_FetchWorker_RedirectPolicy_ASameOriginNameThatNowResolvesPrivate_Refused():
    private = lambda host, port, **kw: [(2, 1, 6, "", ("10.0.0.7", port or 0))]  # noqa: E731
    assert fetch_worker.redirect_allowed("https://feeds.example/a", "https://feeds.example/b", private) is False
    # A feed the person configured on a private address may redirect within itself.
    assert fetch_worker.redirect_allowed("http://10.0.0.7/a", "http://10.0.0.7/b", private) is True
    assert fetch_worker.redirect_allowed("http://127.0.0.1:9/a", "http://127.0.0.1:9/b", private) is True


# The URL may carry a token: the worker gets it on standard input, never in its command line

def test_Feed_Fetch_WorkerArgvNeverHoldsTheUrl(monkeypatch):
    url = "http://127.0.0.1:9/feed?token=SECRETTOKEN"
    seen = []
    real = feed.supervise.run_program

    def spy(argv, environment, timeout_seconds, **kwargs):
        seen.append((argv, kwargs))
        return real(argv, environment, timeout_seconds, **kwargs)

    monkeypatch.setattr(feed.supervise, "run_program", spy)
    with pytest.raises(feed.FeedError):
        feed.fetch(url, 5)
    argv, kwargs = seen[0]
    assert not [word for word in argv if "SECRETTOKEN" in word or "127.0.0.1" in word]
    assert kwargs["stdin_text"] == url.encode()


def test_Feed_Fetch_RunningWorkerShowsNoUrlInTheProcessList(supervised_child):
    supervised_child.deadline = 6
    url = "http://127.0.0.1:42004/feed?token=SECRETTOKEN"
    with pytest.raises(feed.FeedError, match="time"):
        feed.fetch(url, 6)
    (argv, options), = supervised_child.launches
    assert argv == [sys.executable, "-P", "-m", "sakura.digest.fetch_worker"]
    assert options["stdin"] == feed.supervise.subprocess.PIPE
    assert supervised_child.input == url.encode()


def test_Supervise_StdinText_ReachesTheProgramAndDefaultsToClosed(tmp_path):
    script = tmp_path / "echo.py"
    script.write_text("import sys; sys.stdout.write(repr(sys.stdin.read()))")
    from sakura.digest import supervise

    assert supervise.run_program((sys.executable, str(script)), {}, 10).stdout == b"''"
    assert supervise.run_program((sys.executable, str(script)), {}, 10, stdin_text=b"hello").stdout == b"'hello'"


# Review 23: the address that is checked is the address that is connected to

def _resolver(*answers):
    """A resolver that answers the given addresses in turn (the last one for every later call), recording its calls."""
    calls = []

    def resolve(host, port, *args, **kwargs):
        calls.append(host)
        address = answers[min(len(calls) - 1, len(answers) - 1)]
        return [(2, 1, 6, "", (address, port or 0))]

    resolve.calls = calls
    return resolve


def test_FetchWorker_Connection_PinsTheCheckedAddress_AndNeverResolvesAgain():
    connected = []
    resolve = _resolver("93.184.216.34")
    http_class, _https = fetch_worker.pinned_connections("http://feeds.example/a", resolve,
                                                         lambda address, timeout=None, source_address=None: connected.append(address) or socket.socket())
    connection = http_class("feeds.example", 80, timeout=1)
    connection.connect()
    assert connected == [("93.184.216.34", 80)] and resolve.calls == ["feeds.example"]  # one resolution, then the pin
    connection.close()


def test_FetchWorker_Connection_PublicThenLoopback_NeverReachesTheLoopback():
    connected = []
    resolve = _resolver("93.184.216.34", "127.0.0.1")  # the second answer is the rebinding
    http_class, _https = fetch_worker.pinned_connections("http://feeds.example/a", resolve,
                                                         lambda address, timeout=None, source_address=None: connected.append(address) or socket.socket())
    first = http_class("feeds.example", 80, timeout=1)
    first.connect()
    first.close()
    second = http_class("feeds.example", 80, timeout=1)  # a redirect, or a new connection to the same name
    with pytest.raises(fetch_worker.RedirectRefused):
        second.connect()
    assert connected == [("93.184.216.34", 80)]


@pytest.mark.parametrize("answers", [("10.1.2.3",), ("169.254.169.254",), ("::1",), ("93.184.216.34", "127.0.0.1")])
def test_FetchWorker_Connection_AnyCheckedAddressLocal_Refused(answers):
    connected = []
    infos = [(2, 1, 6, "", (address, 80)) for address in answers]
    http_class, _https = fetch_worker.pinned_connections("http://feeds.example/a", lambda *a, **k: infos,
                                                         lambda address, **kw: connected.append(address))
    with pytest.raises(fetch_worker.RedirectRefused):
        http_class("feeds.example", 80).connect()
    assert connected == []


def test_FetchWorker_Connection_HttpsKeepsTheNameForTheCertificate():
    wrapped = []

    class Context:
        def wrap_socket(self, sock, server_hostname=None):
            wrapped.append(server_hostname)
            return sock

    connected = []
    _http, https_class = fetch_worker.pinned_connections("https://feeds.example/a", _resolver("93.184.216.34"),
                                                         lambda address, **kw: connected.append(address) or socket.socket())
    connection = https_class("feeds.example", 443, timeout=1, context=Context())
    connection.connect()
    assert connected == [("93.184.216.34", 443)] and wrapped == ["feeds.example"]
    connection.close()


def test_Feed_Fetch_ANameThatResolvesToTheLoopback_NeverRequested(monkeypatch):
    with _Server(lambda handler, server: handler.send_response(200) or handler.end_headers()) as target:
        opener = fetch_worker.make_opener(f"http://feeds.example:{target.port}/feed", _resolver("127.0.0.1"))
        with pytest.raises(fetch_worker.RedirectRefused):
            opener.open(f"http://feeds.example:{target.port}/feed", timeout=3)
        assert target.requests == 0


def test_FetchWorker_ConfiguredLocalAddress_MayBeFetchedAndRedirectedWithin():
    def respond(handler, server):
        if handler.path == "/feed":
            handler.send_response(302)
            handler.send_header("Location", "/moved")
            handler.end_headers()
        else:
            body = b"ok"
            handler.send_response(200)
            handler.send_header("Content-Length", "2")
            handler.end_headers()
            handler.wfile.write(body)

    with _Server(respond) as server:
        url = f"http://127.0.0.1:{server.port}/feed"
        assert fetch_worker.make_opener(url).open(url, timeout=3).read() == b"ok"


# Only a globally routable address may be fetched: an allowlist, with the IPv4 inside an IPv6 spelling checked too

@pytest.mark.parametrize("address", [
    "100.96.0.1", "100.64.0.1", "100.127.255.254",                  # the overlay range: neither private nor global
    "::ffff:127.0.0.1", "::ffff:100.96.0.1", "::ffff:10.0.0.1",       # IPv4-mapped
    "64:ff9b::7f00:1", "64:ff9b::a00:1",                             # NAT64 around 127.0.0.1 and 10.0.0.1
    "64:ff9b::5db8:d822",                                            # NAT64 sits in ::/8, which is reserved, whatever it wraps
    "2002:7f00:1::", "2002:a00:1::1",                                # 6to4 around 127.0.0.1 and 10.0.0.1
    "2002:5db8:d822::1",                                             # 6to4 itself is not global, even around a public IPv4
    "127.0.0.1", "::1", "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.169.254", "fe80::1", "fc00::1", "0.0.0.0", "::",
    "192.0.2.1", "198.18.0.1", "240.0.0.1",                          # documentation, benchmarking, reserved
    "224.0.0.1", "239.255.255.250", "ff02::1", "ff0e::1",            # multicast, even where it is global in scope
])
def test_FetchWorker_Address_NotGloballyRoutable_Refused(address):
    import ipaddress

    assert fetch_worker.is_refused(ipaddress.ip_address(address)) is True


@pytest.mark.parametrize("address", ["93.184.216.34", "8.8.8.8", "2606:2800:220:1:248:1893:25c8:1946",
                                     "::ffff:93.184.216.34"])
def test_FetchWorker_Address_Public_Passes(address):
    import ipaddress

    assert fetch_worker.is_refused(ipaddress.ip_address(address)) is False


@pytest.mark.parametrize("answer", ["100.96.0.1", "::ffff:127.0.0.1", "64:ff9b::7f00:1", "2002:7f00:1::"])
def test_FetchWorker_Connection_OverlayOrWrappedLocalAddress_NeverConnectedTo(answer):
    connected = []
    http_class, _https = fetch_worker.pinned_connections("http://feeds.example/a", _resolver(answer),
                                                         lambda address, **kw: connected.append(address))
    with pytest.raises(fetch_worker.RedirectRefused):
        http_class("feeds.example", 80).connect()
    assert connected == []


@pytest.mark.parametrize("address", [
    "::127.0.0.1", "::10.0.0.1", "::93.184.216.34",                  # IPv4-compatible ::/96, even around a public IPv4
    "64:ff9b:1::7f00:1", "64:ff9b:1::5db8:d822",                      # the local-use NAT64 prefix, whatever it wraps
    "2606:2800:220:1:0:5efe:a00:1", "2606:2800:220:1:200:5efe:5db8:d822",  # ISATAP, around 10.0.0.1 and a public IPv4
    "2001:0:4136:e378:8000:63bf:3fff:fdd2", "2001:0:5db8:d822::1",    # Teredo 2001::/32
])
def test_FetchWorker_Address_SpecialPurposeIpv6_Refused(address):
    import ipaddress

    assert fetch_worker.is_refused(ipaddress.ip_address(address)) is True


def test_FetchWorker_Address_EveryFlagRefusesOnItsOwn_NotOnlyIsGlobal(monkeypatch):
    import ipaddress

    class Flagged(ipaddress.IPv4Address):
        """A public address that one flag calls special: `is_global` alone (which differs between Pythons) is not trusted."""

        @property
        def is_global(self):
            return True

    for flag in ("is_reserved", "is_unspecified", "is_loopback", "is_private", "is_link_local", "is_multicast"):
        with monkeypatch.context() as patched:
            patched.setattr(Flagged, flag, property(lambda self: True), raising=False)
            assert fetch_worker.is_refused(Flagged("93.184.216.34")) is True, flag
    assert fetch_worker.is_refused(Flagged("93.184.216.34")) is False
