"""Seatbelt rules for loopback connections of the engine's caged commands: the exact ports, and, only when the person
approved it for the project, the system's ephemeral port range.

A checks or exec-role profile denies all network and then names what it allows. By default that is the exact ports the
engine allocated for the application and the ones the host listed (`checks.loopback_ports`): each may be bound,
accepted on and connected to, nothing else on loopback. A test suite that binds a socket on a port the system picks (from
the ephemeral range) and connects to it needs that range open, and so does every other process of the same user that
listens inside it (a debugger, a dev server, a database client): that is the person's decision, given once per project
(`sakura flow approve <task> checks --local-sockets`, kept in the engine home as `checks.local_sockets`), and the
approval text says what it risks. Seatbelt takes no port ranges, so each port of the range is one rule (a profile with
them takes about five seconds to compile, paid by each caged command that has them).

Claude Code lanes are not covered by this module: measured with Claude Code 2.1.323 (acceptance/loopback_canary.py),
its sandbox has no per-port rule for direct loopback connections (`allowLocalBinding` off: no bind or connect at all;
on: every loopback port, a canary on a fixed port included), and an outer Seatbelt fence around the Claude process
makes Claude's own `sandbox_apply` fail (Operation not permitted, measured: any outer profile with a deny rule
refuses a nested one), so a lane cannot be fenced from outside. See the plan for the decision.
"""

from __future__ import annotations

import functools
import os
import re
import subprocess

SYSCTL = "/usr/sbin/sysctl"
RANGE_VARIABLE = "SAKURA_EPHEMERAL_PORT_RANGE"
MAX_PORT = 65535
DEFAULT_EPHEMERAL_RANGE = (49152, 65535)
CHUNK_SIZE = 128  # filters per rule: one rule with every port does not compile
OUTBOUND = "network-outbound"
# How a refused bind or connect reads in a command's output: Seatbelt answers with EPERM, and the language names the call.
_REFUSED = re.compile(r"operation not permitted|\bEPERM\b|PermissionError", re.IGNORECASE)
_SOCKET_CALL = re.compile(r"\b(?:bind|listen|connect|accept|socket)\b|127\.0\.0\.1|localhost|\[?::1\]?", re.IGNORECASE)


def ephemeral_range() -> tuple[int, int]:
    """The first and last port the system hands out for a socket bound to port 0 (the IANA range on macOS).

    `SAKURA_EPHEMERAL_PORT_RANGE=<first>-<last>` narrows it, which keeps the profile small for a test that opens no
    ephemeral socket; anything malformed is ignored.
    """
    override = os.environ.get(RANGE_VARIABLE, "")
    first, _, last = override.partition("-")
    if first.isdigit() and last.isdigit() and 0 < int(first) <= int(last) <= MAX_PORT:
        return int(first), int(last)
    return _system_range()


@functools.lru_cache(maxsize=1)
def _system_range() -> tuple[int, int]:
    values = []
    for name in ("net.inet.ip.portrange.first", "net.inet.ip.portrange.last"):
        done = subprocess.run([SYSCTL, "-n", name], capture_output=True, text=True, check=False,
                              stdin=subprocess.DEVNULL, timeout=10)
        values.append(int(done.stdout) if done.returncode == 0 and done.stdout.strip().isdigit() else 0)
    first, last = values
    return (first, last) if 0 < first <= last <= MAX_PORT else DEFAULT_EPHEMERAL_RANGE


def port_rules(ports: tuple[int, ...]) -> list[str]:
    """Seatbelt rules that let a process bind, accept on and connect to loopback on exactly `ports`, one rule per
    port (the caller denies the rest first). Each name covers IPv4 and IPv6."""
    rules = []
    for port in dict.fromkeys(ports):
        rules += [f'(allow network-bind (local ip "localhost:{port}"))',
                  f'(allow network-inbound (local ip "localhost:{port}"))',
                  f'(allow {OUTBOUND} (remote ip "localhost:{port}"))']
    return rules


def socket_rules() -> list[str]:
    """Seatbelt rules for a suite's own sockets: binding anywhere on loopback and connecting to every ephemeral port.

    Only for a project whose person approved it; every other loopback port (a database, a daemon: all below the
    ephemeral range) stays refused unless it is one of the exact ports."""
    first, last = ephemeral_range()
    return ['(allow network-bind (local ip "localhost:*"))', '(allow network-inbound (local ip "localhost:*"))',
            *_chunks(range(first, last + 1))]


def _chunks(ports: range) -> list[str]:
    rules = []
    for offset in range(0, len(ports), CHUNK_SIZE):
        filters = " ".join(f'(remote ip "localhost:{port}")' for port in ports[offset:offset + CHUNK_SIZE])
        rules.append(f"(allow {OUTBOUND} (require-any {filters}))")
    return rules


def is_socket_denial(output: str) -> bool:
    """Whether a command's output shows the cage refusing its own loopback bind or connect."""
    return bool(_REFUSED.search(output) and _SOCKET_CALL.search(output))


def approval_message(task_id: str, row_ids: list[str]) -> str:
    """The blocked line when check rows failed on a socket the cage refused: what happened, the exact command, its risk."""
    return (f"check {'row' if len(row_ids) == 1 else 'rows'} {', '.join(row_ids)} failed because the cage refused "
            f"the command's own loopback sockets (a bind or connect on a port the system picks). If the command is "
            f"meant to open its own sockets, run `sakura flow approve {task_id} checks --local-sockets`, then "
            f"`sakura flow resume {task_id}`. Risk: it opens the system's high loopback ports, where debuggers, dev "
            f"servers and other tasks' apps listen, to every command of this project, and is remembered for the project.")
