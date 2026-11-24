"""The cockpit's request gate: who is asking, from where, and with what proof of intent. Every doubt denies."""

from __future__ import annotations

import hashlib
import hmac
import ipaddress
import json
import re
import subprocess
import threading
import time
from dataclasses import dataclass
from typing import Callable, Mapping, Sequence

from sakura.cockpit.settings import LOGIN_PATTERN, NODE_PATTERN, CockpitRefused, CockpitSettings
from sakura.flow.render import strip_controls

TAILSCALE_TIMEOUT_SECONDS = 3
WHOIS_CONCURRENCY = 4
WHOIS_CACHE_SECONDS = 60
CSRF_TTL_SECONDS = 1800
OVERLAY_V4 = ipaddress.ip_network("100.64.0.0/10")
OVERLAY_V6 = ipaddress.ip_network("fd7a:115c:a1e0::/48")
DNS_NAME = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?)+")
ORIGIN = re.compile(r"http://([^/?#@\s]+)")
MAX_DEVICE_CHARS = 64

Run = Callable[..., subprocess.CompletedProcess]


class Denied(Exception):
    """Raised by `check` with the reason a request is refused; the reason goes to the access log only."""


@dataclass(frozen=True)
class Self:
    """The server's own overlay address, MagicDNS name (empty when MagicDNS is off) and node id (StableID)."""

    address: str
    dns_name: str
    node_id: str = ""


@dataclass(frozen=True)
class Identity:
    """A overlay peer: login, immutable node id, and a display-only device name."""

    login: str
    node_id: str
    device: str


@dataclass(frozen=True)
class Request:
    """What the gate needs from an HTTP request: headers in order with duplicates, and the form's CSRF token."""

    method: str
    peer_ip: str
    headers: tuple[tuple[str, str], ...]
    csrf: str = ""


# -- the server's own address -------------------------------------------------------------------

def require_overlay_address(address: str) -> str:
    """Return `address` when it is a overlay address of this machine's family, else raise CockpitRefused."""
    try:
        parsed = ipaddress.ip_address(address)
    except ValueError:
        parsed = None
    if parsed is None or parsed.version != 4 or parsed not in OVERLAY_V4:
        raise CockpitRefused(f"refusing to listen on {strip_controls(address)!r}: not a overlay address")
    return address


def read_self(settings: CockpitSettings, run: Run = subprocess.run) -> Self:
    """Ask the trusted Tailscale CLI for this machine's overlay address, DNS name and node id."""
    lines = _tailscale_output(settings, run, "ip", "-4").split()
    address = require_overlay_address(lines[0] if lines else "")
    status = _tailscale_output(settings, run, "status", "--json")
    return Self(address, _dns_name(status), _node_id(status))


def authorities(own: Self, port: int) -> frozenset[str]:
    """Return the `Host` values the cockpit answers to: address and, when known, DNS name, with the port."""
    names = [own.address] + ([own.dns_name] if own.dns_name else [])
    return frozenset(f"{name}:{port}" for name in names)


def _tailscale_output(settings: CockpitSettings, run: Run, *arguments: str) -> str:
    try:
        done = run([str(settings.tailscale), *arguments], capture_output=True, text=True,
                   timeout=TAILSCALE_TIMEOUT_SECONDS, stdin=subprocess.DEVNULL, check=False)
    except subprocess.TimeoutExpired as error:
        raise CockpitRefused(f"tailscale {arguments[0]} did not answer in {TAILSCALE_TIMEOUT_SECONDS} s") from error
    except OSError as error:
        raise CockpitRefused(f"cannot run tailscale: {strip_controls(str(error))}") from error
    if done.returncode != 0:
        raise CockpitRefused(f"tailscale {arguments[0]} failed with exit code {done.returncode}")
    return done.stdout


def _dns_name(status_json: str) -> str:
    try:
        name = json.loads(status_json)["Self"]["DNSName"]
    except (ValueError, KeyError, TypeError) as error:
        raise CockpitRefused("tailscale status has no Self.DNSName") from error
    name = name[:-1] if isinstance(name, str) and name.endswith(".") else name
    if name == "":
        return ""
    if not isinstance(name, str) or not DNS_NAME.fullmatch(name):
        raise CockpitRefused(f"tailscale reported an unusable DNS name: {strip_controls(str(name))[:80]!r}")
    return name


def _node_id(status_json: str) -> str:
    try:
        node_id = json.loads(status_json)["Self"]["ID"]
    except (ValueError, KeyError, TypeError) as error:
        raise CockpitRefused("tailscale status has no Self.ID") from error
    if not isinstance(node_id, str) or not NODE_PATTERN.fullmatch(node_id):
        raise CockpitRefused("tailscale status has no usable Self.ID")
    return node_id


# -- who is asking ------------------------------------------------------------------------------

def identify(peer_ip: str, settings: CockpitSettings, run: Run = subprocess.run) -> Identity | None:
    """Resolve a peer with `tailscale whois --json`; None on any failure, and no helper for a non-overlay peer."""
    if not _is_overlay_peer(peer_ip):
        return None
    try:
        document = json.loads(_tailscale_output(settings, run, "whois", "--json", peer_ip))
        return _identity_from(document)
    except (CockpitRefused, ValueError, LookupError, TypeError, AttributeError):
        return None


def _is_overlay_peer(peer_ip: str) -> bool:
    try:
        parsed = ipaddress.ip_address(peer_ip)
    except ValueError:
        return False
    return parsed in (OVERLAY_V4 if parsed.version == 4 else OVERLAY_V6)


def _identity_from(document: Mapping) -> Identity | None:
    login = document["UserProfile"]["LoginName"]
    node = document["Node"]
    node_id = node["StableID"]
    if not (isinstance(login, str) and LOGIN_PATTERN.fullmatch(login) and isinstance(node_id, str) and NODE_PATTERN.fullmatch(node_id)):
        return None
    device = strip_controls(str(node.get("ComputedName") or node.get("Name") or ""))[:MAX_DEVICE_CHARS]
    return Identity(login, node_id, device)


class CachedIdentifier:
    """`identify` with at most WHOIS_CONCURRENCY helpers at once and successes cached by peer address."""

    def __init__(self, settings: CockpitSettings, run: Run = subprocess.run,
                 clock: Callable[[], float] = time.monotonic) -> None:
        self._settings, self._run, self._clock = settings, run, clock
        self._slots = threading.BoundedSemaphore(WHOIS_CONCURRENCY)
        self._lock = threading.Lock()
        self._cache: dict[str, tuple[float, Identity]] = {}

    def __call__(self, peer_ip: str) -> Identity | None:
        """Return the peer's identity, from the cache when fresh; None when unknown or all helper slots stay busy."""
        cached = self._cached(peer_ip)
        if cached is not None or not _is_overlay_peer(peer_ip):
            return cached
        if not self._slots.acquire(timeout=TAILSCALE_TIMEOUT_SECONDS):
            return None
        try:
            found = identify(peer_ip, self._settings, self._run)
        finally:
            self._slots.release()
        if found is not None:
            self._remember(peer_ip, found)
        return found

    def _cached(self, peer_ip: str) -> Identity | None:
        with self._lock:
            entry = self._cache.get(peer_ip)
            return entry[1] if entry and self._clock() < entry[0] else None

    def _remember(self, peer_ip: str, found: Identity) -> None:
        now = self._clock()
        with self._lock:
            self._cache = {peer: item for peer, item in self._cache.items() if now < item[0]}
            self._cache[peer_ip] = (now + WHOIS_CACHE_SECONDS, found)


# -- the decision -------------------------------------------------------------------------------

def check(request: Request, settings: CockpitSettings, authorities: frozenset[str],
          identify: Callable[[str], Identity | None], *, csrf_secret: bytes, now: float) -> Identity:
    """Return the caller's identity or raise Denied; address checks run before any helper is asked."""
    host = _host(request.headers, authorities)
    if request.method == "POST":
        _require_origin(request.headers, host)
    who = identify(request.peer_ip)
    if who is None:
        raise Denied("unidentified peer")
    if who.login not in settings.allow_logins:
        raise Denied(f"login {who.login} is not allowed")
    if who.node_id not in settings.allow_nodes:
        raise Denied(f"node {who.node_id} is not allowed for {who.login}")
    if request.method == "POST" and not csrf_valid(csrf_secret, request.csrf, who, now):
        raise Denied("missing or invalid CSRF token")
    return who


def single_header(headers: Sequence[tuple[str, str]], name: str) -> str | None:
    """Return the value of `name` (lowercase) when exactly one such header is present, else None."""
    values = [value for key, value in headers if key.lower() == name]
    return values[0] if len(values) == 1 else None


def _host(headers: Sequence[tuple[str, str]], authorities: frozenset[str]) -> str:
    host = single_header(headers, "host")
    if host is None or host not in authorities:
        raise Denied("Host is missing, duplicated or not one of the cockpit's authorities")
    return host


def _require_origin(headers: Sequence[tuple[str, str]], host: str) -> None:
    origin = single_header(headers, "origin")
    match = ORIGIN.fullmatch(origin) if origin is not None else None
    if match is None or match.group(1) != host:
        raise Denied("Origin is missing, duplicated or not the cockpit's own http origin")


# -- proof of intent ----------------------------------------------------------------------------

def csrf_token(secret: bytes, identity: Identity, issued_at: float) -> str:
    """Return `<issued_at>.<hmac>` bound to the login and node id."""
    stamp = str(int(issued_at))
    return f"{stamp}.{_mac(secret, identity, stamp)}"


def csrf_valid(secret: bytes, token: str, identity: Identity, now: float) -> bool:
    """True when `token` was issued to this login and node within CSRF_TTL_SECONDS; garbage is just False."""
    stamp, _, mac = token.partition(".")
    if not mac.isascii():  # compare_digest raises on non-ASCII text
        return False
    if not (stamp.isascii() and stamp.isdigit() and len(stamp) <= 12) or not 0 <= now - int(stamp) <= CSRF_TTL_SECONDS:
        return False
    return hmac.compare_digest(mac, _mac(secret, identity, stamp))


def _mac(secret: bytes, identity: Identity, stamp: str) -> str:
    message = "\n".join((identity.login, identity.node_id, stamp)).encode()
    return hmac.new(secret, message, hashlib.sha256).hexdigest()
