"""The pieces of a Codex run's configuration that are not paths: TOML text, the network table and the MCP servers.

`flow/cage.py` decides what a run may read, write and reach, from the same lists as the Claude cage; this module only
spells those decisions in Codex's `-c key=value` form, so the two stay apart and neither imports the other.
"""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence

WORK_PROFILE = "sakura-work"
REVIEW_PROFILE = "sakura-review"
NETWORK_PROXY_FLAG = "features.network_proxy=true"


def toml_string(text: str) -> str:
    """Quote `text` as a TOML basic string: backslash, quote and control characters are escaped."""
    escaped = "".join(f"\\u{ord(char):04x}" if char in '\\"' or ord(char) < 0x20 or ord(char) == 0x7F else char
                      for char in text)
    return f'"{escaped}"'


def filesystem_table(entries: Mapping[str, str]) -> str:
    """The inline TOML table of a permission profile's filesystem entries (path to `read`, `write` or `none`)."""
    return "{" + ", ".join(f"{json.dumps(path)} = {json.dumps(access)}" for path, access in entries.items()) + "}"


def network_table(domains: Sequence[str], local_binding: bool) -> str:
    """The network of a profile that may reach the net: only `domains` (Codex's proxy refuses every other host, which
    needs `features.network_proxy`), and the machine's own ports only when `local_binding`."""
    allowed = ", ".join(f"{toml_string(domain)} = \"allow\"" for domain in domains)
    return f"{{enabled=true, allow_local_binding={str(local_binding).lower()}, domains={{{allowed}}}}}"


def mcp_pairs(servers: Mapping[str, Mapping]) -> list[str]:
    """One `-c mcp_servers.<name>=...` pair per server of an MCP config (`command`, `args`, `env`), so Codex starts the
    engine's tool server and the granted host servers the way Claude Code does from its `--mcp-config` file."""
    pairs: list[str] = []
    for name, entry in servers.items():
        parts = [f"command={toml_string(str(entry['command']))}",
                 f"args=[{', '.join(toml_string(str(arg)) for arg in entry.get('args', []))}]"]
        if entry.get("env"):
            parts.append("env={" + ", ".join(f"{key}={toml_string(str(value))}" for key, value in entry["env"].items()) + "}")
        pairs += ["-c", f"mcp_servers.{name}={{{', '.join(parts)}}}"]
    return pairs


def environment_pair(values: Mapping[str, str]) -> list[str]:
    """`-c shell_environment_policy.set={NAME="value", ...}`: variables Codex puts in the environment of its commands over
    what they inherit."""
    table = ", ".join(f"{name}={toml_string(value)}" for name, value in values.items())
    return ["-c", f"shell_environment_policy.set={{{table}}}"]
