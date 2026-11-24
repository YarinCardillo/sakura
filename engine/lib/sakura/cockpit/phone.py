"""The phone page, set up from the app: what the Mac offers, which devices of the same login may open it, and which port.

`cockpit.toml` (`settings.py`) holds who may use the cockpit; nobody wrote it from the app. This module reads the setup from the
file and from `tailscale status --json` (read only: it never changes Tailscale), lists the overlay's devices of the Mac's own login,
and changes the file: allow a device, remove one, set the port. A change is written whole to a file of mode 0600 and moved into
place, keeps every key the engine does not know, and is refused for a device that is not on the overlay or belongs to another login.
A running serve takes the new file in without a restart (`server.py`).
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import tomllib
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path

from sakura.cockpit import settings
from sakura.cockpit.settings import CockpitRefused
from sakura.flow import registry

CLI_CANDIDATES = ("/Applications/Tailscale.app/Contents/MacOS/Tailscale", "/opt/homebrew/bin/tailscale",
                  "/usr/local/bin/tailscale", "/usr/bin/tailscale")
TIMEOUT_SECONDS = 10
FILE_MODE = 0o600
RUNNING = "Running"
Run = Callable[..., subprocess.CompletedProcess]


class PhoneSetupRefused(ValueError):
    """The change cannot be made; the message says why, in words for the person."""


def read(*, path: Path | None = None, candidates: Sequence[str] | None = None, run: Run = subprocess.run) -> dict:
    """The phone page setup as it stands: Tailscale, this Mac, the port and link, the devices that may open the page and the
    overlay devices of the same login. Reads `tailscale status --json` only; nothing is changed."""
    source = _path(path)
    data = _file(source)
    cli = find_tailscale(candidates)
    net = _overlay(cli, run) if cli else None
    allowed = [str(item) for item in data.get("allow_nodes", [])]
    port = data.get("port", settings.DEFAULT_PORT)
    mac = _mac(net)
    devices = _devices(net, set(allowed)) if net else []
    allowed_devices = _allowed_devices(allowed, devices, mac)
    state = _state(cli, net, allowed_devices)
    return {"state": state, "words": _words(state, link=_link(mac, port), devices=devices, allowed=allowed_devices),
            "enabled": source.exists(), "port": port,
            "tailscale": {"installed": cli is not None, "logged_in": net is not None, "path": str(cli) if cli else ""},
            "mac": mac, "link": _link(mac, port), "allowed_logins": [str(item) for item in data.get("allow_logins", [])],
            "allowed_devices": allowed_devices, "devices": devices}


def allow_device(node_id: str, *, path: Path | None = None, candidates: Sequence[str] | None = None,
                 run: Run = subprocess.run) -> dict:
    """Let the device `node_id` open the phone page. The device must be on the overlay and of the Mac's own login; with no file yet,
    the Mac's login is allowed with it. Raises PhoneSetupRefused, writing nothing."""
    source, data, net, cli = _context(path, candidates, run)
    device = next((item for item in _devices(net, set()) if item["id"] == node_id), None)
    if device is None:
        if any(node_id == peer["id"] for peer in net["all"]):
            raise PhoneSetupRefused("that device belongs to another login: only your own devices can be allowed")
        raise PhoneSetupRefused("that device is not on your overlay")
    login = net["self"]["login"]
    changed = _with(data, "allow_logins", login, cli)
    changed = _with(changed, "allow_nodes", node_id, cli)
    _write(source, changed)
    return read(path=source, candidates=candidates, run=run)


def remove_device(node_id: str, *, path: Path | None = None, candidates: Sequence[str] | None = None,
                  run: Run = subprocess.run) -> dict:
    """Stop `node_id` from opening the phone page. Raises PhoneSetupRefused for a device that is not allowed."""
    source = _path(path)
    data = _file(source)
    nodes = [str(item) for item in data.get("allow_nodes", [])]
    if node_id not in nodes:
        raise PhoneSetupRefused("that device is not allowed")
    _write(source, {**data, "allow_nodes": [item for item in nodes if item != node_id]})
    return read(path=source, candidates=candidates, run=run)


def set_port(port: object, *, path: Path | None = None, candidates: Sequence[str] | None = None,
             run: Run = subprocess.run) -> dict:
    """Keep `port` as the phone page's port. Raises PhoneSetupRefused for anything that is not a port number."""
    number = _port(port)
    source, data, _, cli = _context(path, candidates, run)
    _write(source, _with({**data, "port": number}, "allow_logins", None, cli))
    return read(path=source, candidates=candidates, run=run)


def find_tailscale(candidates: Sequence[str] | None = None) -> Path | None:
    """The Tailscale CLI the cockpit may run, the first of `candidates` that is trusted (`settings.trusted_tailscale`)."""
    for candidate in candidates or CLI_CANDIDATES:
        try:
            return settings.trusted_tailscale(candidate, _repos())
        except CockpitRefused:
            continue
    return None


# -- Tailscale

def _overlay(cli: Path, run: Run) -> dict | None:
    """What `status --json` says, or None when Tailscale is not running or signed in."""
    try:
        done = run([str(cli), "status", "--json"], capture_output=True, text=True, timeout=TIMEOUT_SECONDS,
                   stdin=subprocess.DEVNULL, check=False)
        status = json.loads(done.stdout)
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return None
    mine = status.get("Self")
    if status.get("BackendState") != RUNNING or not isinstance(mine, dict):
        return None
    users = {str(uid): user.get("LoginName", "") for uid, user in (status.get("User") or {}).items()}
    return {"self": _node(mine, users), "all": [_node(peer, users) for peer in (status.get("Peer") or {}).values()]}


def _node(raw: dict, users: dict[str, str]) -> dict:
    name = str(raw.get("DNSName") or "").rstrip(".")
    addresses = [item for item in raw.get("TailscaleIPs") or [] if "." in str(item)]
    return {"id": str(raw.get("ID", "")), "name": str(raw.get("HostName") or name), "dns": name, "os": str(raw.get("OS", "")),
            "online": bool(raw.get("Online")), "address": addresses[0] if addresses else "",
            "login": users.get(str(raw.get("UserID")), "")}


def _mac(net: dict | None) -> dict | None:
    if net is None:
        return None
    mine = net["self"]
    return {"id": mine["id"], "name": mine["dns"], "address": mine["address"], "login": mine["login"]}


def _devices(net: dict, allowed: set[str]) -> list[dict]:
    """The devices of the Mac's own login (the Mac first), each with whether it may open the page."""
    mine = net["self"]
    nodes = [mine, *(peer for peer in net["all"] if peer["login"] == mine["login"])]
    return [{"id": node["id"], "name": node["name"], "os": node["os"], "online": node["online"],
             "allowed": node["id"] in allowed, "is_this_mac": node is mine} for node in nodes]


def _allowed_devices(allowed: Iterable[str], devices: list[dict], mac: dict | None) -> list[dict]:
    named = {device["id"]: device for device in devices}
    login = mac["login"] if mac else ""
    return [{"id": node, "name": named[node]["name"] if node in named else "", "login": login if node in named else "",
             "known": node in named} for node in allowed]


def _state(cli: Path | None, net: dict | None, allowed: list[dict]) -> str:
    """`not_installed`, `logged_out`, `no_device` (nobody who is on the overlay may open the page yet) or `ready`."""
    if cli is None:
        return "not_installed"
    if net is None:
        return "logged_out"
    return "ready" if any(device["known"] for device in allowed) else "no_device"


def _words(state: str, *, link: str, devices: list[dict], allowed: list[dict]) -> str:
    """What the person reads for `state`: what is so and what to do, with no engine word in it."""
    if state == "not_installed":
        return "Tailscale is not installed on this Mac. Install it from tailscale.com and sign in, then come back here."
    if state == "logged_out":
        return "Tailscale is installed but you are not signed in. Open Tailscale and sign in, then come back here."
    if state == "no_device" and any(not device["is_this_mac"] for device in devices):
        return ("No device may open the phone page yet. Pick your phone in the list; it must be signed in to Tailscale with "
                "this account.")
    if state == "no_device":
        return ("No device may open the phone page yet, and none of your other devices is on your overlay. Install Tailscale on "
                "your phone and sign in with this account: it will appear in the list.")
    names = [device["name"] for device in allowed if device["known"]]
    return f"The phone page is ready: open {link} on {_joined(names)}."


def _joined(names: list[str]) -> str:
    return names[0] if len(names) == 1 else f"{', '.join(names[:-1])} or {names[-1]}"


def _link(mac: dict | None, port: object) -> str:
    host = (mac["name"] or mac["address"]) if mac else ""
    return f"http://{host}:{port}/" if host else ""


# -- the file

def _path(path: Path | None) -> Path:
    return Path(path) if path is not None else Path(settings.DEFAULT_PATH).expanduser()


def _repos() -> list[Path]:
    return registry.repos()


def _file(source: Path) -> dict:
    if not source.exists():
        return {}
    try:
        return tomllib.loads(source.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PhoneSetupRefused(f"{source} cannot be read: {error}") from error


def _context(path: Path | None, candidates: Sequence[str], run: Run) -> tuple[Path, dict, dict, Path]:
    """The file, its content, the overlay and the CLI; Raises PhoneSetupRefused when Tailscale is not there or not signed in."""
    source = _path(path)
    data = _file(source)
    cli = find_tailscale(candidates)
    if cli is None:
        raise PhoneSetupRefused("Tailscale is not installed on this Mac")
    net = _overlay(cli, run)
    if net is None:
        raise PhoneSetupRefused("Tailscale is not signed in on this Mac")
    return source, data, net, cli


def _with(data: dict, key: str, value: str | None, cli: Path) -> dict:
    """`data` with `value` in the list `key`, and the settings the file needs to load (`tailscale`, `port`)."""
    changed = dict(data)
    changed.setdefault("port", settings.DEFAULT_PORT)
    changed.setdefault("tailscale", str(cli))
    changed.setdefault("allow_logins", [])
    changed.setdefault("allow_nodes", [])
    if value is not None and value not in changed[key]:
        changed[key] = [*changed[key], value]
    return changed


def _port(value: object) -> int:
    text = str(value).strip()
    if isinstance(value, bool) or not text.isdigit() or not 1 <= int(text) <= settings.MAX_PORT:
        raise PhoneSetupRefused(f"the port must be a whole number from 1 to {settings.MAX_PORT}; got {value!r}")
    return int(text)


def _write(source: Path, data: dict) -> None:
    """Write `data` as the whole of `source`, mode 0600, through a file of its own that is then moved into place."""
    source.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{source.name}.", dir=source.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(_dump(data))
        os.chmod(name, FILE_MODE)
        os.replace(name, source)
    except BaseException:
        Path(name).unlink(missing_ok=True)
        raise


def _dump(data: dict) -> str:
    """TOML text of scalars, lists of scalars and tables of those; anything else the file might hold is refused, not lost."""
    lines = [f"{key} = {_value(value)}" for key, value in data.items() if not isinstance(value, dict)]
    for key, table in ((key, value) for key, value in data.items() if isinstance(value, dict)):
        lines += ["", f"[{key}]", *(f"{name} = {_value(value)}" for name, value in table.items())]
    return "\n".join(lines) + "\n"


def _value(value: object) -> str:
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_value(item) for item in value) + "]"
    if isinstance(value, (bool, int, float, str)):
        return json.dumps(value)
    raise PhoneSetupRefused(f"cockpit.toml holds a value of a kind this page cannot keep: {value!r}")
