"""Grants: which engine tools and host MCP servers each class of agent may use.

Two host files in the engine home, never read from a repository:
- `mcp-servers.toml` names the MCP servers the person has installed: a table per server with `command` (an absolute
  path to a file no role can write, see below), optional `arguments` and optional `environment` (variable names
  only, never values).
- `grants.toml` has a table per class (`master`, `planner`, `plan_reviewer`, `builder`, `code_reviewer`,
  `evaluator`, `reporter`) with optional `tools` (engine tools, replacing the class default) and `mcp` (server names).

A server runs as the person, outside the sandbox, with the person's credentials: granting one is a deliberate
widening, so no class gets a server unless the file lists it. For the same reason a server's files are never ones a role could have planted: its command is the resolved absolute
path of an existing file, its path-like arguments are absolute, and none of them lies in a folder a role can write
(`flow/cage.writable_roots`). Claude Code does not start the server itself: the MCP config names the engine's
launcher (`sakura grants exec`, `grants_exec.py`), which checks the entry again, runs in a private empty folder with a
fixed PATH and only then starts the command.
"""

from __future__ import annotations

import os
import re
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path

GRANTS_FILE = "grants.toml"
SERVERS_FILE = "mcp-servers.toml"
CLASSES = ("master", "planner", "plan_reviewer", "builder", "code_reviewer", "evaluator", "reporter")
ENGINE_SERVER = "sakura"
MASTER_CLASS = "master"
SCHEDULED_CLASS = "scheduled"
# The digest summarizer (`sakura/digest/summarize.py`) is a role with no grants at all: it is not one of CLASSES (a host
# cannot widen it), and its tools are the two the engine passes on its command line.
DIGEST_CLASS = "digest"
# The master's tools, served by `sakura master serve`.
MASTER_TOOLS = ("projects", "lanes", "lane_open", "lane_say", "lane_stop", "waits", "who_does_what", "board_read",
                "board_post", "knowledge_propose", "scheduled_propose", "permission", "tracks", "track_new",
                "track_update", "track_decision_add", "track_link_lane")
# The engine tools a Claude role may be granted, served by the cage's prompt server (`flow/cage_server.py`).
ROLE_TOOLS = ("board_read", "board_post", "knowledge_propose", "ask_person")
# What every Claude role gets with no host file; each task that adds a tool adds it here.
DEFAULT_ROLE_TOOLS: tuple[str, ...] = ("board_read", "board_post", "ask_person", "knowledge_propose")
SERVER_NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,31}")
VARIABLE_NAME = re.compile(r"[A-Z_][A-Z0-9_]{0,63}")
SERVER_KEYS = frozenset({"command", "arguments", "environment"})
CLASS_KEYS = frozenset({"tools", "mcp", "connectors"})
LAUNCHER = Path(__file__).resolve().parents[2] / "bin" / "sakura"
# An npm package name (`@scope/name`, optionally `@version`) names a registry package, not a path.
PACKAGE_NAME = re.compile(r"@[a-z0-9~][a-z0-9._~-]*/[a-z0-9~][a-z0-9._~-]*(@[A-Za-z0-9._~^<>=-]+)?")


class GrantsError(ValueError):
    """Raised when a grants file is unreadable or names something that does not exist."""


@dataclass(frozen=True)
class ClassGrant:
    """What one class may use: engine tools by name and host MCP servers by name."""

    tools: tuple[str, ...] = ()
    mcp: tuple[str, ...] = ()
    connectors: dict[str, tuple[str, ...]] = field(default_factory=dict)


@dataclass(frozen=True)
class McpServer:
    """One installed MCP server: how Claude Code starts it, and the variables it takes from its own environment."""

    command: str
    arguments: tuple[str, ...] = ()
    environment: tuple[str, ...] = ()

    def config(self, class_name: str, name: str) -> dict:
        """The server's entry in an MCP config: the engine's launcher, never the server's own command, so nothing a
        role could write is looked up or started by Claude Code. Variable values are references Claude Code
        expands, never copies; the launcher passes on only these."""
        entry: dict = {"command": str(LAUNCHER), "args": ["grants", "exec", class_name, name]}
        if self.environment:
            entry["env"] = {name: "${" + name + "}" for name in self.environment}
        return entry


@dataclass(frozen=True)
class Grants:
    """The grants of every class, and the servers they may name."""

    classes: dict[str, ClassGrant] = field(default_factory=dict)
    servers: dict[str, McpServer] = field(default_factory=dict)

    def for_class(self, name: str) -> ClassGrant:
        """The grant of class `name` (the digest role's is always empty); raises GrantsError for another name."""
        if name == DIGEST_CLASS:
            return ClassGrant()
        if name not in CLASSES:
            raise GrantsError(f"unknown class {name!r}; the classes are {', '.join(CLASSES)}")
        return self.classes[name]

    def mcp_servers(self, name: str) -> dict[str, dict]:
        """The MCP config entries of the host servers class `name` is granted, by server name."""
        return {server: self.servers[server].config(name, server) for server in self.for_class(name).mcp}


def load(home: Path, writable: Sequence[Path]) -> Grants:
    """Read `grants.toml` and `mcp-servers.toml` from the engine home `home`; a missing file means the defaults.

    `writable` is every folder a role can write (`flow/cage.writable_roots`, plus the caller's working folder): a
    server's command or path argument inside any of them, or reached through a link into one, is refused.
    Raises GrantsError.
    """
    forbidden = tuple(Path(os.path.realpath(folder)) for folder in writable)
    servers = _servers(_read(home / SERVERS_FILE), forbidden)
    given = _read(home / GRANTS_FILE)
    if DIGEST_CLASS in given:
        raise GrantsError(f"{GRANTS_FILE}: the {DIGEST_CLASS} role takes no grants; remove [{DIGEST_CLASS}]")
    unknown = sorted(set(given) - set(CLASSES))
    if unknown:
        raise GrantsError(f"{GRANTS_FILE}: unknown class {unknown[0]!r}; the classes are {', '.join(CLASSES)}")
    return Grants({name: _class_grant(name, given.get(name, {}), servers) for name in CLASSES}, servers)


def _read(path: Path) -> dict:
    try:
        return tomllib.loads(path.read_text())
    except FileNotFoundError:
        return {}
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise GrantsError(f"cannot read {path}: {error}") from error


def _servers(data: dict, forbidden: Sequence[Path]) -> dict[str, McpServer]:
    return {name: _server(name, table, forbidden) for name, table in data.items()}


def _command(name: str, command: object, forbidden: Sequence[Path]) -> str:
    """`command` resolved to the real file it names, which must be absolute, a regular file and not writable by a role."""
    if not isinstance(command, str) or not command.isprintable() or not os.path.isabs(command):
        raise GrantsError(f"{SERVERS_FILE}: [{name}] command must be an absolute path such as /opt/homebrew/bin/npx, "
                          f"not a bare name or a relative path; got {command!r}")
    resolved = Path(os.path.realpath(command))
    if not resolved.is_file():
        raise GrantsError(f"{SERVERS_FILE}: [{name}] command {command!r} is not an existing regular file")
    _require_outside(name, "command", command, forbidden)
    return str(resolved)


def _arguments(name: str, arguments: object, forbidden: Sequence[Path]) -> tuple[str, ...]:
    """`arguments` as they are, where an argument that looks like a path (it has a slash or starts with a dot) must be
    absolute and outside the folders a role can write; an npm package name is not a path."""
    if not _is_strings(arguments):
        raise GrantsError(f"{SERVERS_FILE}: [{name}] arguments must be a list of strings")
    for argument in arguments:
        if not ("/" in argument or argument.startswith(".")) or PACKAGE_NAME.fullmatch(argument):
            continue
        if not os.path.isabs(argument):
            raise GrantsError(f"{SERVERS_FILE}: [{name}] argument {argument!r} looks like a relative path, which the "
                              f"server would find in a folder a role can write; give an absolute path")
        _require_outside(name, "argument", argument, forbidden)
    return tuple(arguments)


def _require_outside(name: str, what: str, value: str, forbidden: Sequence[Path]) -> None:
    for given in (Path(os.path.abspath(value)), Path(os.path.realpath(value))):
        for root in forbidden:
            if given == root or root in given.parents:
                raise GrantsError(f"{SERVERS_FILE}: [{name}] {what} {value!r} is inside {root}, a folder a role "
                                  f"can write; install the server where only you can write")


def _server(name: str, table: object, forbidden: Sequence[Path]) -> McpServer:
    if not SERVER_NAME.fullmatch(name) or name == ENGINE_SERVER:
        raise GrantsError(f"{SERVERS_FILE}: server name {name!r} must match {SERVER_NAME.pattern} and not be "
                          f"{ENGINE_SERVER!r}")
    if not isinstance(table, dict) or set(table) - SERVER_KEYS:
        raise GrantsError(f"{SERVERS_FILE}: [{name}] has unknown keys; use {', '.join(sorted(SERVER_KEYS))}")
    command = _command(name, table.get("command"), forbidden)
    arguments = _arguments(name, table.get("arguments", []), forbidden)
    variables = table.get("environment", [])
    if not _is_strings(variables) or not all(VARIABLE_NAME.fullmatch(item) for item in variables):
        raise GrantsError(f"{SERVERS_FILE}: [{name}] environment must list variable names such as GITHUB_TOKEN")
    return McpServer(command, arguments, tuple(variables))


def _class_grant(name: str, table: object, servers: dict[str, McpServer]) -> ClassGrant:
    if not isinstance(table, dict) or set(table) - CLASS_KEYS:
        raise GrantsError(f"{GRANTS_FILE}: [{name}] has unknown keys; use {', '.join(sorted(CLASS_KEYS))}")
    tools, mcp = table.get("tools", _default_tools(name)), table.get("mcp", [])
    if not _is_strings(tools) or not _is_strings(mcp):
        raise GrantsError(f"{GRANTS_FILE}: [{name}] tools and mcp must each be a list of names")
    if name == MASTER_CLASS:
        tools = ["scheduled_propose" if tool == "automation_propose" else tool for tool in tools]
    allowed = _default_tools(name)
    for tool in tools:
        if tool not in allowed:
            raise GrantsError(f"{GRANTS_FILE}: [{name}] unknown tool {tool!r}; its tools are {', '.join(allowed)}")
    for server in mcp:
        if server not in servers:
            raise GrantsError(f"{GRANTS_FILE}: [{name}] names server {server!r}, which {SERVERS_FILE} does not list")
    return ClassGrant(tuple(dict.fromkeys(tools)), tuple(dict.fromkeys(mcp)), _connectors(table.get("connectors", {})))


def current_connectors(home: Path, class_name: str) -> dict[str, tuple[str, ...]]:
    """Read the class's current connector grants without resolving unrelated MCP programs."""
    if class_name not in CLASSES:
        return {}
    table = _read(home / GRANTS_FILE).get(class_name, {})
    if not isinstance(table, dict) or set(table) - CLASS_KEYS:
        raise GrantsError(f"{GRANTS_FILE}: invalid connector grant")
    return _connectors(table.get("connectors", {}))


def _connectors(table: object) -> dict[str, tuple[str, ...]]:
    from sakura.connectors import contract, kinds

    allowed = {kinds.READ, *kinds.WRITE_OPERATIONS.values()}
    if not isinstance(table, dict) or any(
        not contract.ID.fullmatch(key) or not _is_strings(ops) or not set(ops) <= allowed for key, ops in table.items()
    ):
        raise GrantsError(f"{GRANTS_FILE}: connectors must map instance IDs to operation names")
    return {key: tuple(dict.fromkeys(ops)) for key, ops in table.items()}


def _default_tools(name: str) -> tuple[str, ...]:
    if name == MASTER_CLASS:
        return MASTER_TOOLS
    return (*DEFAULT_ROLE_TOOLS, "run_row") if name == "evaluator" else DEFAULT_ROLE_TOOLS


def _is_strings(value: object) -> bool:
    return isinstance(value, (list, tuple)) and all(isinstance(item, str) for item in value)
