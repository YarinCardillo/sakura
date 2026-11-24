import json
import os
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest

from sakura.cockpit import gate
from sakura.cockpit.settings import CockpitRefused, CockpitSettings
from cockpit_support import (OWN_ADDRESS, OWN_DNS_NAME, OWN_NODE_ID, PEER_ADDRESS, whois_document, write_hanging_tailscale,
                             write_tailscale)

SECRET = b"s" * 32
OWNER = gate.Identity("owner@example.com", "nMAC", "mac")
PORT = 8787
HOST_IP = f"{OWN_ADDRESS}:{PORT}"
HOST_NAME = f"{OWN_DNS_NAME}:{PORT}"
AUTHORITIES = gate.authorities(gate.Self(OWN_ADDRESS, OWN_DNS_NAME), PORT)
NOW = 1_000_000.0


def _settings(tmp_path, logins=("owner@example.com",), nodes=("nMAC", "nPHONE")):
    return CockpitSettings(PORT, frozenset(logins), frozenset(nodes), tmp_path / "tailscale")


def _request(method="GET", host=HOST_IP, origin=None, csrf="", peer=PEER_ADDRESS, extra=()):
    headers = [("Host", host)] if host is not None else []
    if origin is not None:
        headers.append(("Origin", origin))
    return gate.Request(method, peer, tuple(headers) + tuple(extra), csrf)


def _check(tmp_path, request, identity=OWNER, **settings_kwargs):
    return gate.check(request, _settings(tmp_path, **settings_kwargs), AUTHORITIES, lambda _ip: identity,
                      csrf_secret=SECRET, now=NOW)


def _post(csrf=None, host=HOST_IP, origin=f"http://{HOST_IP}", identity=OWNER, **kwargs):
    token = gate.csrf_token(SECRET, identity, NOW) if csrf is None else csrf
    return _request("POST", host=host, origin=origin, csrf=token, **kwargs)


def _denied(tmp_path, request, reason, **kwargs):
    with pytest.raises(gate.Denied, match=reason):
        _check(tmp_path, request, **kwargs)


# -- the server's own overlay address -----------------------------------------------------------

def test_reads_own_address_and_dns_name_without_trailing_dot(tmp_path):
    helper = write_tailscale(tmp_path)
    own = gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), helper))
    assert own == gate.Self(OWN_ADDRESS, OWN_DNS_NAME, OWN_NODE_ID)
    assert gate.authorities(own, PORT) == {HOST_IP, HOST_NAME}


@pytest.mark.parametrize("node_id", [None, "", "bad id", "x" * 65, 7])
def test_refuses_to_start_without_a_usable_own_node_id(tmp_path, node_id):
    helper = write_tailscale(tmp_path, node_id=node_id)
    with pytest.raises(CockpitRefused, match="Self.ID"):
        gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), helper))


def test_refuses_to_start_without_overlay_address(tmp_path):
    for address in ("", "127.0.0.1", "0.0.0.0", "::", "192.168.1.5", "100.63.255.255", "100.128.0.1", "not-an-ip", "-h"):
        helper = write_tailscale(tmp_path, address=address)
        with pytest.raises(CockpitRefused, match="overlay"):
            gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), helper))


def test_refuses_to_start_when_the_helper_fails_or_hangs(tmp_path, monkeypatch):
    failing = tmp_path / "failing"
    failing.write_text("#!/bin/sh\nexit 3\n")
    failing.chmod(0o700)
    with pytest.raises(CockpitRefused, match="tailscale"):
        gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), failing))
    monkeypatch.setattr(gate, "TAILSCALE_TIMEOUT_SECONDS", 0.3)
    hanging = write_hanging_tailscale(tmp_path, tmp_path / "started")
    started = time.monotonic()
    with pytest.raises(CockpitRefused, match="did not answer"):
        gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), hanging))
    assert time.monotonic() - started < 5


def test_refuses_a_malformed_dns_name(tmp_path):
    helper = write_tailscale(tmp_path, dns_name="evil host.tail1234.ts.net.")
    with pytest.raises(CockpitRefused, match="DNS name"):
        gate.read_self(CockpitSettings(PORT, frozenset(), frozenset(), helper))


# -- who is asking ------------------------------------------------------------------------------

def test_identify_reads_login_stable_id_and_device_from_whois(tmp_path):
    helper = write_tailscale(tmp_path, whois=whois_document("owner@example.com", "nPHONE", "phone"),
                             marker=tmp_path / "calls")
    found = gate.identify(PEER_ADDRESS, CockpitSettings(PORT, frozenset(), frozenset(), helper))
    assert found == gate.Identity("owner@example.com", "nPHONE", "phone")
    assert (tmp_path / "calls").read_text().strip() == f"whois --json {PEER_ADDRESS}"


@pytest.mark.parametrize("body", ["", "not json", "[]", "{}", '{"Node": {}, "UserProfile": {}}',
                                  json.dumps(whois_document("bad login", "nX")),
                                  json.dumps(whois_document("owner@example.com", "n X")),
                                  json.dumps(whois_document("", "nX"))])
def test_identify_fails_closed_on_unusable_whois(tmp_path, body):
    helper = write_tailscale(tmp_path, whois_body=body or " ")
    assert gate.identify(PEER_ADDRESS, CockpitSettings(PORT, frozenset(), frozenset(), helper)) is None


def test_identify_fails_closed_on_nonzero_exit(tmp_path):
    failing = tmp_path / "tailscale"
    failing.write_text("#!/bin/sh\necho '{}'\nexit 1\n")
    failing.chmod(0o700)
    assert gate.identify(PEER_ADDRESS, CockpitSettings(PORT, frozenset(), frozenset(), failing)) is None


@pytest.mark.parametrize("peer", ["127.0.0.1", "192.168.1.5", "8.8.8.8", "::1", "garbage", "-h", "", "100.64.0.9; id"])
def test_identify_never_runs_the_helper_for_a_non_overlay_peer(tmp_path, peer):
    helper = write_tailscale(tmp_path, whois=whois_document("owner@example.com", "nMAC"), marker=tmp_path / "calls")
    assert gate.identify(peer, CockpitSettings(PORT, frozenset(), frozenset(), helper)) is None
    assert not (tmp_path / "calls").exists()


def test_whois_failure_and_hang_fail_closed(tmp_path, monkeypatch):
    monkeypatch.setattr(gate, "TAILSCALE_TIMEOUT_SECONDS", 2.0)
    marker = tmp_path / "started"
    hanging = write_hanging_tailscale(tmp_path, marker)
    started = time.monotonic()
    found = gate.identify(PEER_ADDRESS, CockpitSettings(PORT, frozenset(), frozenset(), hanging))
    assert found is None and time.monotonic() - started < 5
    pid = int(marker.read_text().split()[0])
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    with pytest.raises(gate.Denied, match="unidentified"):
        gate.check(_request(), _settings(tmp_path), AUTHORITIES, lambda _ip: None, csrf_secret=SECRET, now=NOW)


def _fake_run(document, counter=None, delay=0.0):
    def run(argv, **kwargs):
        if counter is not None:
            counter.enter()
        try:
            time.sleep(delay)
            return subprocess.CompletedProcess(argv, 0, stdout=json.dumps(document), stderr="")
        finally:
            if counter is not None:
                counter.leave()
    return run


class _Counter:
    def __init__(self):
        self.lock = threading.Lock()
        self.active = 0
        self.peak = 0
        self.calls = 0

    def enter(self):
        with self.lock:
            self.active += 1
            self.calls += 1
            self.peak = max(self.peak, self.active)

    def leave(self):
        with self.lock:
            self.active -= 1


def test_whois_results_are_cached_for_sixty_seconds(tmp_path):
    counter, clock = _Counter(), [100.0]
    lookup = gate.CachedIdentifier(_settings(tmp_path), _fake_run(whois_document("owner@example.com", "nMAC"), counter),
                                   clock=lambda: clock[0])
    assert gate.WHOIS_CACHE_SECONDS == 60
    assert lookup(PEER_ADDRESS) == gate.Identity("owner@example.com", "nMAC", "phone")
    clock[0] += 59
    lookup(PEER_ADDRESS)
    assert counter.calls == 1
    lookup("100.64.0.10")
    assert counter.calls == 2
    clock[0] += 2
    lookup(PEER_ADDRESS)
    assert counter.calls == 3


def test_failures_are_not_cached(tmp_path):
    counter = _Counter()

    def run(argv, **kwargs):
        counter.enter()
        counter.leave()
        raise subprocess.TimeoutExpired(argv, 3)
    lookup = gate.CachedIdentifier(_settings(tmp_path), run)
    assert lookup(PEER_ADDRESS) is None and lookup(PEER_ADDRESS) is None
    assert counter.calls == 2


def test_helper_processes_never_exceed_the_concurrency_limit(tmp_path):
    assert gate.WHOIS_CONCURRENCY == 4
    counter = _Counter()
    lookup = gate.CachedIdentifier(_settings(tmp_path), _fake_run(whois_document("owner@example.com", "nMAC"), counter, 0.1))
    results = []
    threads = [threading.Thread(target=lambda n=n: results.append(lookup(f"100.64.1.{n}"))) for n in range(12)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(10)
    assert counter.peak <= 4 and counter.calls == 12
    assert all(item is not None for item in results)


def test_saturated_helper_slots_fail_closed_after_the_timeout(tmp_path):
    release = threading.Event()
    occupied = threading.Semaphore(0)
    document = json.dumps(whois_document("owner@example.com", "nMAC"))
    elapsed = []

    def blocked(argv, **kwargs):
        if argv[-1] != "100.64.2.99":
            occupied.release()
            assert release.wait(10)
        return subprocess.CompletedProcess(argv, 0, stdout=document, stderr="")

    lookup = gate.CachedIdentifier(_settings(tmp_path), blocked)
    slots = lookup._slots

    def acquire(*, timeout):
        if slots.acquire(blocking=False):
            return True
        elapsed.append(timeout)
        return False

    lookup._slots = SimpleNamespace(acquire=acquire, release=slots.release)
    with ThreadPoolExecutor(max_workers=gate.WHOIS_CONCURRENCY) as workers:
        holders = [workers.submit(lookup, f"100.64.2.{n}") for n in range(gate.WHOIS_CONCURRENCY)]
        try:
            for _ in holders:
                assert occupied.acquire(timeout=10)
            assert lookup("100.64.2.99") is None
            assert elapsed == [gate.TAILSCALE_TIMEOUT_SECONDS]
        finally:
            release.set()
        assert all(holder.result(timeout=10) is not None for holder in holders)


# -- allow lists --------------------------------------------------------------------------------

def test_allowed_login_on_allowed_node_passes(tmp_path):
    assert _check(tmp_path, _request()) == OWNER


def test_same_login_other_node_denied(tmp_path):
    stranger = gate.Identity("owner@example.com", "nSTOLEN", "mac")
    _denied(tmp_path, _request(), "node", identity=stranger)


def test_other_login_on_allowed_node_denied(tmp_path):
    _denied(tmp_path, _request(), "login", identity=gate.Identity("friend@example.com", "nMAC", "mac"))


def test_renamed_allowed_device_still_allowed(tmp_path):
    renamed = gate.Identity("owner@example.com", "nMAC", "totally-new-name")
    assert _check(tmp_path, _request(), identity=renamed) == renamed


def test_empty_lists_deny_everyone(tmp_path):
    _denied(tmp_path, _request(), "login", logins=())
    _denied(tmp_path, _request(), "node", nodes=())
    _denied(tmp_path, _request(), "login", logins=(), nodes=())


def test_unidentified_peer_denied(tmp_path):
    _denied(tmp_path, _request(), "unidentified", identity=None)


# -- Host ---------------------------------------------------------------------------------------

@pytest.mark.parametrize("host", [
    f"{OWN_ADDRESS}:{PORT + 1}", f"{OWN_DNS_NAME}:{PORT + 1}", OWN_ADDRESS, OWN_DNS_NAME,
    f"user@{OWN_DNS_NAME}:{PORT}", f"evil.{OWN_DNS_NAME}:{PORT}", f"{OWN_DNS_NAME}.evil:{PORT}",
    f"{OWN_DNS_NAME}.:{PORT}", f"{OWN_DNS_NAME.upper()}:{PORT}", f" {OWN_DNS_NAME}:{PORT} x", f"127.0.0.1:{PORT}",
    f"{OWN_DNS_NAME}:{PORT}/", "", f"{OWN_ADDRESS}:{PORT},evil.example:{PORT}",
])
def test_host_authority_cases_denied(tmp_path, host):
    _denied(tmp_path, _request(host=host), "Host")


def test_host_authority_cases_allowed(tmp_path):
    assert _check(tmp_path, _request(host=HOST_IP)) == OWNER
    assert _check(tmp_path, _request(host=HOST_NAME)) == OWNER


def test_missing_or_duplicated_host_denied(tmp_path):
    _denied(tmp_path, _request(host=None), "Host")
    _denied(tmp_path, _request(extra=(("host", HOST_IP),)), "Host")
    _denied(tmp_path, _request(extra=(("HOST", "evil.example"),)), "Host")


# -- Origin and CSRF ----------------------------------------------------------------------------

def test_valid_post_passes(tmp_path):
    assert _check(tmp_path, _post()) == OWNER
    assert _check(tmp_path, _post(host=HOST_NAME, origin=f"http://{HOST_NAME}")) == OWNER


@pytest.mark.parametrize("origin", [
    None, "null", "", f"https://{HOST_IP}", f"ftp://{HOST_IP}", f"//{HOST_IP}", HOST_IP, f"http://{HOST_NAME}",
    f"http://{HOST_IP}/", f"http://{HOST_IP}/x", f"http://evil.example:{PORT}", f"http://u@{HOST_IP}", f"HTTP://{HOST_IP}",
])
def test_origin_cases_denied(tmp_path, origin):
    _denied(tmp_path, _post(origin=origin), "Origin")


def test_ip_host_with_dns_origin_denied(tmp_path):
    _denied(tmp_path, _post(host=HOST_IP, origin=f"http://{HOST_NAME}"), "Origin")


def test_duplicated_origin_denied(tmp_path):
    _denied(tmp_path, _post(extra=(("Origin", f"http://{HOST_IP}"),)), "Origin")


def test_get_does_not_need_origin_or_csrf(tmp_path):
    assert _check(tmp_path, _request("GET")) == OWNER


def test_post_without_or_with_bad_csrf_denied(tmp_path):
    _denied(tmp_path, _post(csrf=""), "CSRF")
    _denied(tmp_path, _post(csrf="garbage"), "CSRF")
    _denied(tmp_path, _post(csrf=gate.csrf_token(b"other-secret" * 3, OWNER, NOW)), "CSRF")


def test_csrf_bound_to_node(tmp_path):
    phone = gate.Identity("owner@example.com", "nPHONE", "phone")
    token = gate.csrf_token(SECRET, phone, NOW)
    _denied(tmp_path, _post(csrf=token), "CSRF")
    assert _check(tmp_path, _post(csrf=token), identity=phone) == phone


def test_csrf_bound_to_login(tmp_path):
    other = gate.Identity("friend@example.com", "nMAC", "mac")
    _denied(tmp_path, _post(csrf=gate.csrf_token(SECRET, other, NOW)), "CSRF")


def test_csrf_expires_after_thirty_minutes(tmp_path):
    assert gate.CSRF_TTL_SECONDS == 1800
    token = gate.csrf_token(SECRET, OWNER, NOW - 1800)
    assert gate.csrf_valid(SECRET, token, OWNER, NOW)
    assert not gate.csrf_valid(SECRET, gate.csrf_token(SECRET, OWNER, NOW - 1801), OWNER, NOW)
    assert not gate.csrf_valid(SECRET, gate.csrf_token(SECRET, OWNER, NOW + 5), OWNER, NOW)


@pytest.mark.parametrize("token", ["", ".", "x.y", "-1.abc", "12.", ".abc", "1e3.abc", "99999999999999999999.aa", "1.2.3"])
def test_csrf_garbage_is_invalid_not_an_error(token):
    assert not gate.csrf_valid(SECRET, token, OWNER, NOW)


@pytest.mark.parametrize("token", ["1000000.\u00e9", "1000000.\u0661", "\u0661.abc"])
def test_csrf_non_ascii_token_is_invalid_not_an_error(token):
    assert gate.csrf_valid(SECRET, token, OWNER, NOW) is False
