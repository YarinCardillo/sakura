"""Helpers for cockpit tests: fake tailscale scripts and overlay constants. Nothing here runs the real binary."""

from __future__ import annotations

import json
import stat
from pathlib import Path

OWN_ADDRESS = "100.64.0.1"
OWN_DNS_NAME = "mac.tail1234.ts.net"
OWN_NODE_ID = "nHOST"
PEER_ADDRESS = "100.64.0.9"


def whois_document(login: str, node_id: str, device: str = "phone") -> dict:
    """Return the part of `tailscale whois --json` the gate reads."""
    return {"Node": {"StableID": node_id, "ComputedName": device, "Name": f"{device}.tail1234.ts.net."},
            "UserProfile": {"LoginName": login}}


def write_tailscale(directory: Path, *, address: str = OWN_ADDRESS, dns_name: str = OWN_DNS_NAME + ".",
                    node_id: object = OWN_NODE_ID, whois: dict | None = None, whois_body: str = "",
                    marker: Path | None = None) -> Path:
    """Write an executable fake tailscale and return its path.

    `ip -4` prints `address`, `status --json` prints the Self DNS name and node id (`node_id` None leaves the
    id out), `whois --json <peer>` prints `whois` (or the raw `whois_body` when given). Every call appends its arguments to `marker` when set.
    """
    script = directory / "tailscale"
    log = f'echo "$@" >> "{marker}"\n' if marker else ""
    document = json.dumps({"Self": {"DNSName": dns_name, **({} if node_id is None else {"ID": node_id})}})
    whois_text = whois_body or json.dumps(whois or {})
    script.write_text(
        "#!/bin/sh\n" + log
        + f'case "$1" in\n  ip) echo "{address}";;\n'
        + f"  status) cat <<'JSON'\n{document}\nJSON\n;;\n"
        + f"  whois) cat <<'JSON'\n{whois_text}\nJSON\n;;\n  *) exit 2;;\nesac\n")
    script.chmod(stat.S_IRWXU)
    return script


def write_hanging_tailscale(directory: Path, marker: Path) -> Path:
    """Write a fake tailscale whose `whois` records its start and then sleeps far past the timeout."""
    script = directory / "tailscale"
    script.write_text(f'#!/bin/sh\necho "$$ $@" >> "{marker}"\nexec sleep 30\n')
    script.chmod(stat.S_IRWXU)
    return script


def settings_text(tailscale: Path, *, logins=("owner@example.com",), nodes=("nMAC", "nPHONE"), port: int = 8787) -> str:
    """Return a cockpit.toml body."""
    return (f"port = {port}\nallow_logins = {json.dumps(list(logins))}\n"
            f"allow_nodes = {json.dumps(list(nodes))}\ntailscale = {json.dumps(str(tailscale))}\n")
