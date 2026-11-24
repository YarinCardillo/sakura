"""The child process that fetches one feed URL: `python -P -m sakura.digest.fetch_worker`, the URL on standard input.

The URL may carry a token, so it is read from standard input and never put in the command line, where any process
could list it. The engine runs it under supervision (`feed.fetch`), which ends it at an absolute deadline, whatever a server does
with its headers or its body. It writes the body to standard output and nothing else there; when it fails it writes one
short token to standard error (`HTTP 404`, `redirect refused`, `too many redirects` or an error class name), never the
URL or what the server said. It uses the standard library only and imports nothing else of the engine, so it starts
fast and stays small.

Addresses: a name is resolved once for each connection, every address it gave is checked, and the connection goes to
the address that was checked (not to the name, which could now answer another address), with the TLS check and the
Host header still on the name. The same holds for the first request and for every redirect. A loopback, private or
link-local or otherwise not globally routable address (the overlay range included) is refused unless the person
wrote the feed's own host as one. No proxy is used: it could not be
checked this way.

Redirects: at most three, each only within the origin the person configured (same scheme, host and port), so a feed
cannot send the engine to another host, to a local service or from https to http. (A feed on a private address the person wrote as that address may still redirect within itself.)
"""

from __future__ import annotations

import http.client
import ipaddress
import socket
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable
from typing import TextIO

MAX_REDIRECTS = 3
MAX_URL_CHARACTERS = 2000
NAT64_NETWORK = ipaddress.IPv6Network("64:ff9b::/96")
LOCAL_NAT64_NETWORK = ipaddress.IPv6Network("64:ff9b:1::/48")
TEREDO_NETWORK = ipaddress.IPv6Network("2001::/32")
READ_CHUNK = 65536
SOCKET_TIMEOUT_SECONDS = 30
USER_AGENT = "sakura-digest"
ACCEPT = "application/atom+xml, application/rss+xml, application/xml, text/xml"
DEFAULT_PORTS = {"http": 80, "https": 443}

Resolver = Callable[..., list]


class RedirectRefused(Exception):
    """A redirect the policy does not allow."""


def redirect_allowed(configured: str, target: str, resolve: Resolver = socket.getaddrinfo) -> bool:
    """Whether a redirect from the configured feed URL to `target` may be followed."""
    first, second = urllib.parse.urlsplit(configured), urllib.parse.urlsplit(target)
    try:
        if _origin(first) != _origin(second):
            return False
        return _is_private_literal(first.hostname) or not _resolves_private(second.hostname, resolve)
    except ValueError:
        return False


def pinned_connections(configured: str, resolve: Resolver = socket.getaddrinfo,
                       connect: Callable = socket.create_connection) -> tuple[type, type]:
    """The HTTP and HTTPS connection classes that open every socket to an address they checked themselves.

    `connect` opens the socket (`socket.create_connection` by default). A connection to a name that gives any local
    address is refused with RedirectRefused before a socket exists, unless the person wrote `configured`'s host as a
    local address."""
    allow_local = _is_private_literal(urllib.parse.urlsplit(configured).hostname)

    def pinned(host: str, port: int, timeout, source_address) -> socket.socket:
        found = [info[4][0].split("%")[0] for info in resolve(host, port, type=socket.SOCK_STREAM)]
        if not found or not (allow_local or not any(_is_local(ipaddress.ip_address(address)) for address in found)):
            raise RedirectRefused("address refused")
        return connect((found[0], port), timeout=timeout, source_address=source_address)

    class Http(http.client.HTTPConnection):
        def connect(self) -> None:
            self.sock = pinned(self.host, self.port, self.timeout, self.source_address)

    class Https(http.client.HTTPSConnection):
        def connect(self) -> None:
            raw = pinned(self.host, self.port, self.timeout, self.source_address)
            self.sock = self._context.wrap_socket(raw, server_hostname=self.host)

    return Http, Https


def make_opener(configured: str, resolve: Resolver = socket.getaddrinfo,
                connect: Callable = socket.create_connection) -> urllib.request.OpenerDirector:
    """An opener for the feed at `configured`: pinned connections, no proxy, redirects within the origin."""
    http_class, https_class = pinned_connections(configured, resolve, connect)

    class HttpHandler(urllib.request.HTTPHandler):
        def http_open(self, req):
            return self.do_open(http_class, req)

    class HttpsHandler(urllib.request.HTTPSHandler):
        def https_open(self, req):
            return self.do_open(https_class, req, context=self._context)

    return urllib.request.build_opener(urllib.request.ProxyHandler({}), HttpHandler, HttpsHandler,
                                       _SameOriginRedirects(configured))


def main(stdin: TextIO | None = None) -> int:
    """Fetch the URL on standard input to standard output; 0 when done, 3 after writing the reason to standard error."""
    url = (stdin or sys.stdin).read(MAX_URL_CHARACTERS + 1).strip()
    if not url or len(url) > MAX_URL_CHARACTERS:
        return _failed("no URL")
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": ACCEPT})
    opener = make_opener(url)
    try:
        with opener.open(request, timeout=SOCKET_TIMEOUT_SECONDS) as response:
            while chunk := response.read1(READ_CHUNK):
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
    except urllib.error.HTTPError as error:
        return _failed(f"HTTP {error.code}")
    except RedirectRefused as error:
        return _failed(str(error))
    except BaseException as error:  # noqa: BLE001 (the parent needs a reason, and only the class name is safe to give)
        return _failed(type(error).__name__)
    return 0


def _failed(token: str) -> int:
    sys.stderr.write(token + "\n")
    return 3


def _origin(parts: urllib.parse.SplitResult) -> tuple[str, str, int]:
    scheme = parts.scheme.lower()
    if scheme not in DEFAULT_PORTS or not parts.hostname:
        raise ValueError("not an http(s) URL")
    return scheme, parts.hostname.lower(), parts.port or DEFAULT_PORTS[scheme]


def _is_private_literal(host: str | None) -> bool:
    """Whether the person wrote the feed's host as a local address (an IP literal, or `localhost`)."""
    if host is None:
        return False
    if host.lower() == "localhost":
        return True
    try:
        return _is_local(ipaddress.ip_address(host))
    except ValueError:
        return False


def _resolves_private(host: str | None, resolve: Resolver) -> bool:
    if host is None:
        return True
    try:
        return any(_is_local(ipaddress.ip_address(found[4][0].split("%")[0])) for found in resolve(host, None))
    except (OSError, ValueError):
        return True  # a name that cannot be resolved or read is not followed


def is_refused(address: ipaddress.IPv4Address | ipaddress.IPv6Address) -> bool:
    """Whether the engine may not fetch from `address`: an allowlist, so anything that is not globally routable is
    refused (loopback, private, link-local, the shared range 100.64.0.0/10 that overlays use, documentation and reserved
    ranges); each of the special flags also refuses on its own, so the answer does not hang on how one Python version
    computes `is_global`; and every multicast address. An IPv6 address that wraps an IPv4 one (IPv4-mapped, 6to4, NAT64,
    Teredo, ISATAP, IPv4-compatible) is checked as itself and by what it wraps, and the special-purpose IPv6 ranges are
    refused outright (IPv4-compatible `::/96`, `64:ff9b:1::/48`, ISATAP, Teredo `2001::/32`)."""
    if address.version == 6 and _is_special_purpose(address):
        return True
    return any(_flagged(candidate) for candidate in (address, *_wrapped(address)))


def _flagged(address: ipaddress.IPv4Address | ipaddress.IPv6Address) -> bool:
    return (not address.is_global or address.is_reserved or address.is_unspecified or address.is_loopback
            or address.is_private or address.is_link_local or address.is_multicast)


def _is_special_purpose(address: ipaddress.IPv6Address) -> bool:
    return (int(address) >> 32 == 0 or address in LOCAL_NAT64_NETWORK or address in TEREDO_NETWORK
            or _is_isatap(address))


def _is_isatap(address: ipaddress.IPv6Address) -> bool:
    """An interface id of `0000:5efe` or `0200:5efe` (the universal/local bit) followed by an IPv4 address."""
    return (int(address) >> 32) & 0xFFFFFFFF in (0x00005EFE, 0x02005EFE)


def _wrapped(address: ipaddress.IPv4Address | ipaddress.IPv6Address) -> list[ipaddress.IPv4Address]:
    if address.version != 6:
        return []
    found = [address.ipv4_mapped, address.sixtofour]
    if address in NAT64_NETWORK or _is_isatap(address) or int(address) >> 32 == 0:
        found.append(ipaddress.IPv4Address(int(address) & 0xFFFFFFFF))
    if address.teredo:
        found += list(address.teredo)
    return [candidate for candidate in found if candidate is not None]


def _is_local(address: ipaddress.IPv4Address | ipaddress.IPv6Address) -> bool:
    return is_refused(address)


class _SameOriginRedirects(urllib.request.HTTPRedirectHandler):
    """Follow at most `MAX_REDIRECTS` redirects, each only where `redirect_allowed` says."""

    def __init__(self, configured: str) -> None:
        self._configured, self._followed = configured, 0

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self._followed += 1
        if self._followed > MAX_REDIRECTS:
            raise RedirectRefused("too many redirects")
        if not redirect_allowed(self._configured, newurl):
            raise RedirectRefused("redirect refused")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


if __name__ == "__main__":
    sys.exit(main())
