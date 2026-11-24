"""The launchd agent that runs the scheduled digest.

A LaunchAgent with a calendar time and `RunAtLoad`: launchd runs a calendar job it missed while the Mac slept once, at
wake, and never wakes the Mac for it; `RunAtLoad` covers a Mac that was off at that time. The command it starts,
`sakura digest run --scheduled`, does nothing before the configured time and nothing twice in one day, so the extra
starts are harmless. The agent has no wake key and no keep-alive.

Installing it is a persistent change to the person's Mac, so only `sakura digest install` does it, on their word.
`launchctl` is injected: the tests fake it, and nothing here runs it on its own.
"""

from __future__ import annotations

import os
import plistlib
import pwd
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from sakura import beneath, engine_home
from sakura.digest import config
from sakura.flow.surface import KIT_BIN

LABEL = "local.sakura.digest"
LAUNCHCTL = "/bin/launchctl"
AGENTS_RELATIVE = Path("Library") / "LaunchAgents"
LOG_FILE = "launchd.log"
PLIST_MODE = 0o644
AGENT_PATH = ("{home}/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin")

Launchctl = Callable[[list[str]], int]


@dataclass(frozen=True)
class Agent:
    """Where and how the agent is installed; the defaults are the real `launchctl` and the account's own folder."""

    launchctl: Launchctl | None = None
    folder: Path | None = None
    home: Path | None = None


class LaunchdError(RuntimeError):
    """Raised, with a one-line reason, when the agent cannot be installed."""


def account_home() -> Path:
    """The account's home from the password database; `$HOME` is not the agent's to choose."""
    return Path(pwd.getpwuid(os.getuid()).pw_dir)


def plist(settings: config.DigestConfig, home: Path, sakura_bin: Path = KIT_BIN) -> dict:
    """The agent's property list: the calendar time, `RunAtLoad`, a fixed PATH and a log in the engine home."""
    hours, minutes = settings.at.split(":")
    log = str(config.digest_folder() / LOG_FILE)
    return {"Label": LABEL, "ProgramArguments": [str(sakura_bin), "digest", "run", "--scheduled"],
            "StartCalendarInterval": {"Hour": int(hours), "Minute": int(minutes)}, "RunAtLoad": True,
            "EnvironmentVariables": agent_environment(home), "StandardOutPath": log, "StandardErrorPath": log}


def install(settings: config.DigestConfig, *, launchctl: Launchctl | None = None, agents_dir: Path | None = None,
            home: Path | None = None) -> Path:
    """Write the agent's file and (re)load it; running it again replaces the old agent. Returns the file's path."""
    home = home or account_home()
    config.digest_folder().mkdir(mode=config.FOLDER_MODE, parents=True, exist_ok=True)
    return install_agent(LABEL, plist(settings, home), launchctl=launchctl, agents_dir=agents_dir, home=home)


def install_agent(label: str, content: dict, *, launchctl: Launchctl | None = None, agents_dir: Path | None = None,
                  home: Path | None = None) -> Path:
    """Write the agent `label` with the property list `content` into the agents folder and (re)load it. Returns the file.

    Raises LaunchdError when the file cannot be written or launchctl refuses it (the file stays in place then).
    """
    run_launchctl = launchctl or default_launchctl
    folder = agents_dir or (home or account_home()) / AGENTS_RELATIVE
    try:
        folder.mkdir(parents=True, exist_ok=True)
        beneath.write_bytes(folder, f"{label}.plist", plistlib.dumps(content), PLIST_MODE)
    except OSError as error:
        raise LaunchdError(f"cannot write the agent file in {folder}: {error.strerror or error}") from error
    run_launchctl(["bootout", _target(label)])  # an agent that was not loaded is not an error
    if run_launchctl(["bootstrap", _domain(), str(folder / f"{label}.plist")]) != 0:
        raise LaunchdError("launchctl could not load the agent; the file is in place, try again or log out and in")
    return folder / f"{label}.plist"


def is_installed(*, agents_dir: Path | None = None, home: Path | None = None) -> bool:
    """Whether the digest's agent file is in the agents folder."""
    return ((agents_dir or (home or account_home()) / AGENTS_RELATIVE) / f"{LABEL}.plist").is_file()


def uninstall(*, launchctl: Launchctl | None = None, agents_dir: Path | None = None, home: Path | None = None) -> bool:
    """Unload the agent and remove its file; True when there was something to remove, and no error when not."""
    return uninstall_agent(LABEL, launchctl=launchctl, agents_dir=agents_dir, home=home)


def uninstall_agent(label: str, *, launchctl: Launchctl | None = None, agents_dir: Path | None = None,
                    home: Path | None = None) -> bool:
    """Unload the agent `label` and remove its file; True when there was a file, and no error when not."""
    run_launchctl = launchctl or default_launchctl
    path = (agents_dir or (home or account_home()) / AGENTS_RELATIVE) / f"{label}.plist"
    run_launchctl(["bootout", _target(label)])
    existed = path.is_file() or path.is_symlink()
    try:
        path.unlink(missing_ok=True)
    except OSError as error:
        raise LaunchdError(f"cannot remove {path}: {error.strerror or error}") from error
    return existed


def agent_environment(home: Path) -> dict:
    """The environment every agent starts with: the account's home, a fixed PATH, and the engine home override if any."""
    override = os.environ.get(engine_home.ENV_VARIABLE)
    return {"HOME": str(home), "PATH": ":".join(AGENT_PATH).format(home=home),
            **({engine_home.ENV_VARIABLE: override} if override else {})}


def _domain() -> str:
    return f"gui/{os.getuid()}"


def _target(label: str) -> str:
    return f"{_domain()}/{label}"


def default_launchctl(arguments: list[str]) -> int:
    return subprocess.run([LAUNCHCTL, *arguments], capture_output=True, check=False, stdin=subprocess.DEVNULL).returncode


def remove_confirmed(label: str, agent: Agent) -> None:
    """Retire a legacy label only after launchd confirms it is unloaded."""
    home = agent.home or account_home()
    folder = agent.folder or home / AGENTS_RELATIVE
    control = agent.launchctl or default_launchctl
    target = _target(label)
    control(["bootout", target])
    if control(["print", target]) == 0:
        raise LaunchdError("the legacy agent is still loaded")
    (folder / f"{label}.plist").unlink(missing_ok=True)
