"""`sakura grants exec <class> <server>`: the engine's launcher for a granted MCP server.

Claude Code starts a server with the role's working folder (a workspace) and environment, and a bare command or a
relative argument would find a file the role planted there. So the MCP config of a run names this launcher instead of
the server. It reads the host grants again, checks that the class is still granted the server and that the entry
still passes the checks of `grants.load` (its command and path arguments outside every folder a role can write,
and outside the folder it was started in), moves to a private empty folder of the engine home, fixes PATH, drops the
tmux variables and only then starts the command.

The server's environment is built, not filtered: trusted defaults (the person's home, user name and locale), a fixed PATH,
the variables the host file grants by name, and private cache, config and tmp folders under the engine home. A role's
cage points `npm_config_cache` and the other caches at its own tmp folder, which later runs of the task reuse, so
anything inherited from the role's environment (those caches, `PYTHONPATH`, `NODE_OPTIONS`, a preload) could make the
server, which runs outside the sandbox, execute code the role wrote.

It protects against a role (or a repository) planting a file the server would run or read through the working
folder, the PATH or a relative argument. It does not protect against another process of the same user outside the
cage, which can edit the host files or the server's own folder.
"""

from __future__ import annotations

import os
import sys
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home, git_program, grants
from sakura.flow import cage

FOLDER = "grants-cwd"
STATE_FOLDER = "grants-state"
# What the server may inherit from the environment it was started in: who the person is and how text is written.
TRUSTED_VARIABLES = ("HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TERM")
# The cache, configuration and tmp variables of the common toolchains, each pointed at a private folder of the engine home.
PRIVATE_VARIABLES = {"npm_config_cache": "npm", "PIP_CACHE_DIR": "pip", "UV_CACHE_DIR": "uv", "XDG_CACHE_HOME": "xdg-cache",
                     "XDG_CONFIG_HOME": "xdg-config", "TMPDIR": "tmp"}
FOLDER_MODE = 0o700
TMUX_PREFIX = "TMUX"
USAGE = "usage: sakura grants exec <class> <server>"


@dataclass(frozen=True)
class Launch:
    """What the launcher starts: the program and its arguments, the folder it starts in and its environment."""

    argv: list[str]
    cwd: Path
    environment: dict[str, str]


def main(argv: list[str]) -> int:
    """`sakura grants exec <class> <server>`: replace this process by the granted server; 2 for a refusal."""
    if len(argv) != 2:
        print(USAGE, file=sys.stderr)
        return 2
    try:
        launch = prepare(engine_home.root(), argv[0], argv[1], cwd=Path.cwd(), environment=os.environ)
        os.chdir(launch.cwd)
    except (grants.GrantsError, OSError, engine_home.EngineHomeError) as error:
        print(f"sakura grants exec: {error}", file=sys.stderr)
        return 2
    os.execve(launch.argv[0], launch.argv, launch.environment)
    return 1  # not reached: execve replaces the process or raises


def prepare(home: Path, class_name: str, server: str, cwd: Path, environment: Mapping[str, str]) -> Launch:
    """The launch of `server` for class `class_name`, checked against the host files as they are now.

    `cwd` is the folder the launcher was started in (a role's workspace): nothing may be taken from it. Raises
    GrantsError when the class is not granted the server, the entry no longer passes, or the private folder fails.
    """
    loaded = grants.load(home, (*cage.writable_roots(), cwd))
    if server not in loaded.for_class(class_name).mcp:
        raise grants.GrantsError(f"{class_name} is not granted the server {server!r}")
    entry = loaded.servers[server]
    return Launch([entry.command, *entry.arguments], _private_folder(home),
                  _environment(entry, os.path.dirname(entry.command), environment, home))


def _environment(entry: grants.McpServer, command_folder: str, environment: Mapping[str, str],
                 home: Path) -> dict[str, str]:
    kept = {name: environment[name] for name in (*TRUSTED_VARIABLES, *entry.environment) if name in environment}
    kept["PATH"] = os.pathsep.join([*git_program.SAFE_PATH, command_folder])
    state = _private_folder(home, STATE_FOLDER)
    for variable, name in PRIVATE_VARIABLES.items():
        kept[variable] = str(_private_folder(state, name))
    return kept


def _private_folder(home: Path, name: str = FOLDER) -> Path:
    """A fixed folder of the server under `home` (the engine home, which no role can write), mode 0700; the default is
    the empty folder it starts in."""
    folder = home / name
    try:
        folder.mkdir(mode=FOLDER_MODE, exist_ok=True)
        folder.chmod(FOLDER_MODE)
    except OSError as error:
        raise grants.GrantsError(f"cannot make the server's working folder {folder}: {error.strerror}") from error
    return folder
