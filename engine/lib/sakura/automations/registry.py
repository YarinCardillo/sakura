"""Automations: engine verbs the person lets run on a schedule.

A definition is `<engine home>/automations/<name>.toml` (mode 0600): `name`, `description`, `schedule` (`daily HH:MM` or
`every N minutes`, N from 5 to 1440), `verb` and `enabled`. The verb is one of `ALLOWED_VERBS`, the whole list of what
an automation can run: each names a fixed engine command (`VERB_ARGV`), run as an argument list, never through a shell.
The check is made when a definition is proposed and again at every run, so a file edited to another verb runs nothing.

The master proposes (`propose`), which writes a pending definition under `pending/` and nothing else: only the person's
accept (`accept`) installs the LaunchAgent. A refusal always carries one fixed sentence, never the text that was refused.
The keeper and the phone page are listed as always on (their own switch is the `B` control); the morning digest is the
digest's own LaunchAgent under the name `morning-digest`.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import tomllib
from collections.abc import Callable
from dataclasses import asdict, dataclass, replace
from datetime import datetime, timezone
from pathlib import Path

from sakura import engine_home, keeper, text_limits
from sakura.automations import launchd
from sakura.digest import config as digest_config, launchd as digest_launchd
from sakura.flow import gates, process
from sakura.flow.surface import KIT_BIN

FOLDER, PENDING_FOLDER, STATE_FILE = "automations", "pending", "state.json"
FILE_MODE, FOLDER_MODE = 0o600, 0o700
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
MORNING_DIGEST = "morning-digest"
ALWAYS_ON = (keeper.KEEPER_NAME, keeper.PHONE_PAGE_NAME)
VERB_ARGV = {"digest run --scheduled": ("digest", "run", "--scheduled"), "flow wake": ("flow", "wake")}
ALLOWED_VERBS = tuple(VERB_ARGV)
VERB_REFUSAL = "an automation runs only an engine verb from the allow-list: " + ", ".join(ALLOWED_VERBS)
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
DAILY = re.compile(r"daily ([01]\d|2[0-3]):([0-5]\d)")
EVERY = re.compile(r"every (\d{1,4}) minutes")
MIN_MINUTES, MAX_MINUTES = 5, 1440
MAX_DESCRIPTION, MAX_PENDING = 200, 20
RUN_TIMEOUT_SECONDS = 900
KEYS = frozenset({"name", "description", "schedule", "verb", "enabled", "installed", "proposed_at", "author"})
ROW_FIELDS = ("name", "description", "schedule_text", "enabled", "installed", "last_run_at", "last_result")


class AutomationRefused(ValueError):
    """Raised, with one fixed sentence, when a definition or a request does not apply."""


@dataclass(frozen=True)
class Automation:
    """One automation. `installed` says its LaunchAgent was loaded by the person's accept (or enable)."""

    name: str
    description: str
    schedule: str
    verb: str
    enabled: bool = False
    installed: bool = False
    proposed_at: str = ""
    author: str = ""

    @property
    def digest(self) -> str:
        """What the person is shown (name, description, schedule, verb): an answer names it, so a changed proposal is another."""
        return text_limits.content_digest({"name": self.name, "description": self.description,
                                           "schedule": self.schedule, "verb": self.verb})


RunVerb = Callable[[tuple[str, ...]], int]


def folder() -> Path:
    """The folder of the definitions in the engine home; not created."""
    return engine_home.root() / FOLDER


def validate(name: str, description: str, schedule: str, verb: str) -> Automation:
    """The automation these fields make, or AutomationRefused: the verb must be on the allow-list, the schedule one of the
    two forms, the name a new lowercase name."""
    if verb not in VERB_ARGV:
        raise AutomationRefused(VERB_REFUSAL)
    if not NAME.fullmatch(name) or name == MORNING_DIGEST or name in ALWAYS_ON:
        raise AutomationRefused("the name is lowercase letters, digits and hyphens, at most 32, and not one already used")
    if not description.strip() or len(description) > MAX_DESCRIPTION or "\n" in description:
        raise AutomationRefused(f"the description is one line of at most {MAX_DESCRIPTION} characters")
    if not is_schedule(schedule):
        raise AutomationRefused(f"the schedule is `daily HH:MM` or `every N minutes` with N from {MIN_MINUTES} to {MAX_MINUTES}")
    return Automation(name, description.strip(), schedule, verb)


def is_schedule(schedule: str) -> bool:
    """Whether `schedule` is `daily HH:MM` or `every N minutes`, N within bounds."""
    every = EVERY.fullmatch(schedule)
    return bool(DAILY.fullmatch(schedule) or (every and MIN_MINUTES <= int(every.group(1)) <= MAX_MINUTES))


# Proposals

def propose(name: str, description: str, schedule: str, verb: str, author: str, now: datetime) -> Automation:
    """Write a pending definition for the person to accept; nothing is installed. Returns it."""
    found = replace(validate(name, description, schedule, verb), proposed_at=_format(now), author=author)
    if len(pending()) >= MAX_PENDING and found.name not in [each.name for each in pending()]:
        raise AutomationRefused(f"{MAX_PENDING} proposals are already waiting for the person")
    _write(folder() / PENDING_FOLDER / f"{found.name}.toml", found)
    return found


def pending() -> list[Automation]:
    """The proposals the person has not decided, oldest first."""
    from sakura.scheduled import migrate, store

    if _digest_migrated():
        return []
    try:
        retired = migrate.retired_names()
    except store.Refused as error:
        raise AutomationRefused(str(error)) from error
    return sorted((item for item in _read_all(folder() / PENDING_FOLDER) if item.name not in retired),
                  key=lambda found: (found.proposed_at, found.name))


def accept(name: str, digest: str, agent: digest_launchd.Agent, now: datetime) -> Automation:
    """Install the agent of the pending proposal `name` the person was shown (`digest`) and keep it as a definition.

    Raises AutomationRefused (`no longer waiting`) when there is no such proposal or it changed since."""
    found = _waiting(name, digest)
    kept = replace(found, enabled=True, installed=True)
    launchd.install(kept, agent)
    _write(folder() / f"{name}.toml", kept)
    (folder() / PENDING_FOLDER / f"{name}.toml").unlink(missing_ok=True)
    return kept


def reject(name: str, digest: str) -> None:
    """Drop the pending proposal `name` the person was shown. Raises AutomationRefused as `accept` does."""
    _waiting(name, digest)
    (folder() / PENDING_FOLDER / f"{name}.toml").unlink(missing_ok=True)


def _waiting(name: str, digest: str) -> Automation:
    found = next((each for each in pending() if each.name == name and each.digest == digest), None)
    if found is None:
        raise AutomationRefused("no longer waiting: the proposal was decided or changed since it was shown; nothing was done")
    return found


# Definitions

def load() -> list[Automation]:
    """The definitions, by name."""
    return sorted(_read_all(folder()), key=lambda found: found.name)


def enable(name: str, agent: digest_launchd.Agent) -> None:
    """Install the agent of `name` and mark it enabled."""
    found = _definition(name)
    launchd.install(found, agent)
    _write(folder() / f"{name}.toml", replace(found, enabled=True, installed=True))


def disable(name: str, agent: digest_launchd.Agent) -> None:
    """Unload the agent of `name` and keep the definition, switched off."""
    found = _definition(name)
    launchd.uninstall(name, agent)
    _write(folder() / f"{name}.toml", replace(found, enabled=False, installed=False))


def remove(name: str, agent: digest_launchd.Agent) -> None:
    """Unload the agent of `name` and delete its definition."""
    _definition(name)
    launchd.uninstall(name, agent)
    (folder() / f"{name}.toml").unlink(missing_ok=True)


def note_morning_digest(settings: digest_config.DigestConfig, *, is_installed: bool) -> None:
    """Keep the digest's own LaunchAgent as the automation `morning-digest` (`digest install`, `uninstall`, a new time)."""
    _write(folder() / f"{MORNING_DIGEST}.toml", Automation(
        MORNING_DIGEST, "Write the morning digest", f"daily {settings.at}", "digest run --scheduled",
        is_installed, is_installed))


def _definition(name: str) -> Automation:
    found = next((each for each in load() if each.name == name), None)
    if found is None:
        raise AutomationRefused(f"no automation {name!r}")
    if found.verb == "digest run --scheduled" and _digest_migrated():
        raise AutomationRefused("scheduled_migration_complete")
    return found


def _digest_migrated() -> bool:
    from sakura.scheduled import migrate, store

    try:
        return migrate.owns_digest()
    except store.Refused as error:
        raise AutomationRefused(str(error)) from error


# Rows

def rows() -> list[dict]:
    """The automations as the app reads them: the definitions with their last run, then the always-on services. Reads
    files of the engine home only: it starts nothing and never looks at the agents folder."""
    ran = _read_state()
    is_migrated = _digest_migrated()
    found = [{"name": each.name, "description": each.description, "schedule_text": each.schedule,
              "enabled": each.enabled, "installed": each.installed, "last_run_at": ran.get(each.name, {}).get("at"),
              "last_result": ran.get(each.name, {}).get("result")} for each in load() if not (is_migrated and each.verb == "digest run --scheduled")]
    return found + _always_on_rows()


def _always_on_rows() -> list[dict]:
    try:
        held = keeper.status(datetime.now(timezone.utc))
    except OSError:  # the keeper's own row says so (`engine.keeper.problem`); here the services show as off
        held = {"on": False, "phone_page": False}
    services = ((keeper.KEEPER_NAME, "Resume lanes after a restart", held["on"]),
                (keeper.PHONE_PAGE_NAME, "Serve the phone page", held["phone_page"]))
    return [{"name": name, "description": text, "schedule_text": "always on", "enabled": is_on, "installed": is_on,
             "last_run_at": None, "last_result": None} for name, text, is_on in services]


# Running

def run(name: str, now: datetime, *, run_verb: RunVerb | None = None) -> int:
    """Run the automation `name` once, as its LaunchAgent does, and record how it went. Returns the verb's exit code.

    A disabled automation does nothing. The verb is checked against the allow-list again: a definition edited to another
    verb raises AutomationRefused and runs nothing."""
    found = _definition(name)
    if found.verb not in VERB_ARGV:
        raise AutomationRefused(VERB_REFUSAL)
    if not found.enabled:
        return 0
    code = (run_verb or _run_engine_verb)(VERB_ARGV[found.verb])
    _record_run(name, now, "ok" if code == 0 else f"failed with exit code {code}")
    return code


def _run_engine_verb(argv: tuple[str, ...]) -> int:
    done = subprocess.run([str(KIT_BIN), *argv], stdin=subprocess.DEVNULL, capture_output=True, check=False,
                          env=process.child_environment(os.environ), timeout=RUN_TIMEOUT_SECONDS)
    return done.returncode


def _record_run(name: str, now: datetime, result: str) -> None:
    held = _read_state()
    held[name] = {"at": _format(now), "result": result}
    _write_text(folder() / STATE_FILE, json.dumps(held, sort_keys=True))


def _read_state() -> dict:
    try:
        found = json.loads((folder() / STATE_FILE).read_text())
    except (OSError, ValueError):
        return {}
    return found if isinstance(found, dict) else {}


# Files

def _read_all(directory: Path) -> list[Automation]:
    found = []
    for path in sorted(directory.glob("*.toml")) if directory.is_dir() else []:
        try:
            data = tomllib.loads(path.read_text())
            found.append(Automation(**{key: value for key, value in data.items() if key in KEYS}))
        except (OSError, ValueError, TypeError):
            continue  # a file that does not read is not an automation; the person sees it missing from the list
    return found


def _write(path: Path, found: Automation) -> None:
    lines = [f"{key} = {json.dumps(value)}" if not isinstance(value, bool) else f"{key} = {str(value).lower()}"
             for key, value in asdict(found).items() if value != ""]
    _write_text(path, "\n".join(lines) + "\n")


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(mode=FOLDER_MODE, parents=True, exist_ok=True)
    gates.write_atomically(path, text.encode())
    path.chmod(FILE_MODE)


def _format(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).strftime(TIME_FORMAT)
