"""The launchd agent that keeps the engine keeper (`sakura keeper serve`) running.

It starts at login (`RunAtLoad`) and is started again only when the keeper exits with a failure (`KeepAlive` on
unsuccessful exit): a keeper stopped on purpose stays stopped. Installing it is a persistent change to the person's
Mac, so only `sakura keeper install` does it, on their word; `launchctl` is injected and the tests fake it.
"""

from __future__ import annotations

from pathlib import Path

from sakura import engine_home
from sakura.digest import launchd
from sakura.flow.surface import KIT_BIN

LABEL = "local.sakura.keeper"
LOG_FILE = "keeper.log"


def plist(home: Path, sakura_bin: Path = KIT_BIN) -> dict:
    """The agent's property list: run at load, restart on failure only, a fixed PATH and a log in the engine home."""
    log = str(engine_home.root() / LOG_FILE)
    return {"Label": LABEL, "ProgramArguments": [str(sakura_bin), "keeper", "serve"], "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False}, "EnvironmentVariables": launchd.agent_environment(home),
            "StandardOutPath": log, "StandardErrorPath": log}


def install(*, launchctl: launchd.Launchctl | None = None, agents_dir: Path | None = None,
            home: Path | None = None) -> Path:
    """Write the keeper's agent file and (re)load it; running it again replaces the old agent. Returns the file."""
    home = home or launchd.account_home()
    return launchd.install_agent(LABEL, plist(home), launchctl=launchctl, agents_dir=agents_dir, home=home)


def uninstall(*, launchctl: launchd.Launchctl | None = None, agents_dir: Path | None = None,
              home: Path | None = None) -> bool:
    """Unload the agent and remove its file; lanes and their runs are left alone. True when there was a file."""
    return launchd.uninstall_agent(LABEL, launchctl=launchctl, agents_dir=agents_dir, home=home)
