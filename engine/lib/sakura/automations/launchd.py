"""The LaunchAgent of one automation: `local.sakura.auto.<name>`, which runs `sakura automations run <name>`.

A daily schedule is a calendar time, an interval schedule a `StartInterval`; neither wakes the Mac, and the agent has no
keep-alive. The morning digest keeps the digest's own agent (`local.sakura.digest`), which this module builds through the
digest's own property list. `launchctl` and the agents folder are injected through the digest's `Agent`: the tests fake them.
"""

from __future__ import annotations

from pathlib import Path

from sakura import engine_home
from sakura.digest import config as digest_config, launchd as digest_launchd
from sakura.flow.surface import KIT_BIN

LABEL_PREFIX = "local.sakura.auto."
MORNING_DIGEST = "morning-digest"
LOG_SUFFIX = ".log"
SECONDS_PER_MINUTE = 60


def label_of(name: str) -> str:
    """The LaunchAgent label of the automation `name`."""
    return digest_launchd.LABEL if name == MORNING_DIGEST else LABEL_PREFIX + name


def plist(automation, home: Path, sakura_bin: Path = KIT_BIN) -> dict:
    """The agent's property list for `automation`."""
    if automation.name == MORNING_DIGEST:
        return digest_launchd.plist(digest_config.load_or_default(), home, sakura_bin)
    log = str(engine_home.root() / "automations" / f"{automation.name}{LOG_SUFFIX}")
    content = {"Label": label_of(automation.name),
               "ProgramArguments": [str(sakura_bin), "automations", "run", automation.name],
               "EnvironmentVariables": digest_launchd.agent_environment(home), "StandardOutPath": log,
               "StandardErrorPath": log}
    words = automation.schedule.split()
    if words[0] == "daily":
        hours, minutes = words[1].split(":")
        return {**content, "StartCalendarInterval": {"Hour": int(hours), "Minute": int(minutes)}}
    return {**content, "StartInterval": int(words[1]) * SECONDS_PER_MINUTE}


def install(automation, agent: digest_launchd.Agent) -> Path:
    """Write the agent's file and (re)load it. Returns the file's path."""
    home = agent.home or digest_launchd.account_home()
    return digest_launchd.install_agent(label_of(automation.name), plist(automation, home), launchctl=agent.launchctl,
                                        agents_dir=agent.folder, home=home)


def uninstall(name: str, agent: digest_launchd.Agent) -> bool:
    """Unload the agent of `name` and remove its file; True when there was one."""
    return digest_launchd.uninstall_agent(label_of(name), launchctl=agent.launchctl, agents_dir=agent.folder,
                                          home=agent.home)
