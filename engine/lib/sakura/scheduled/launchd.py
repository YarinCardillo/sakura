"""One on-demand LaunchAgent per engine home for scheduled work."""

import fcntl
import hashlib
import os
import plistlib
from pathlib import Path
from xml.parsers.expat import ExpatError

from sakura import beneath, engine_home
from sakura.digest import launchd
from sakura.flow.surface import KIT_BIN
from sakura.scheduled import store

LABEL_PREFIX = "local.sakura.scheduled"
LABEL_HASH_LENGTH = 16


def label_of(root: Path) -> str:
    """Return the dispatcher label for the canonical engine path."""
    digest = hashlib.sha256(os.fsencode(root.resolve())).hexdigest()
    return f"{LABEL_PREFIX}.{digest[:LABEL_HASH_LENGTH]}"


def plist(tasks: store.Store, root: Path, home: Path, sakura_bin: Path = KIT_BIN) -> dict:
    """One minute calendar pulses let launchd coalesce misses into a wake notification."""
    log = str(tasks.path.parent.resolve() / "launchd.log")
    environment = launchd.agent_environment(home)
    if engine_home.ENV_VARIABLE in environment:
        environment[engine_home.ENV_VARIABLE] = str(root)
    return {"Label": label_of(root), "ProgramArguments": [str(sakura_bin), "scheduled", "tick"],
            "StartCalendarInterval": [{"Minute": minute} for minute in range(60)], "RunAtLoad": True,
            "EnvironmentVariables": environment, "StandardOutPath": log, "StandardErrorPath": log}


def reconcile(tasks: store.Store, *, agent: launchd.Agent | None = None, force_enabled: bool = False,
              verify_loaded: bool = False) -> None:
    """Reconcile against task state, optionally checking launchd even if the plist exists."""
    agent = agent or launchd.Agent()
    root = engine_home.root().resolve()
    home = root if os.environ.get(engine_home.ENV_VARIABLE) else agent.home or launchd.account_home()
    folder = agent.folder or home / launchd.AGENTS_RELATIVE
    content = plist(tasks, root, home)
    path = folder / f"{content['Label']}.plist"
    if not tasks.path.parent.is_dir() and not path.exists():
        return
    lock = tasks.path.parent / "launchd.lock"
    descriptor = os.open(lock, os.O_CREAT | os.O_NOFOLLOW | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        _apply(tasks, agent, path, content, force_enabled=force_enabled, verify_loaded=force_enabled or verify_loaded)
    finally:
        os.close(descriptor)


def _apply(tasks, agent, path, content, *, force_enabled: bool, verify_loaded: bool) -> None:
    problem = tasks.path.parent / "launchd-problem"
    enabled = force_enabled or _has_work(tasks)
    try:
        installed = path.is_file() and not problem.exists()
        if enabled and installed and verify_loaded:
            installed = _confirmed(path, agent, content)
        if enabled and not installed:
            launchd.install_agent(path.stem, content, launchctl=agent.launchctl, agents_dir=path.parent)
        if enabled or not _cutover_in_progress(tasks):
            if not enabled and path.exists():
                _unload(path, agent)
            environment = content["EnvironmentVariables"]
            if engine_home.ENV_VARIABLE not in environment:
                _retire_legacy(path.with_name(f"{LABEL_PREFIX}.plist"), agent, environment["HOME"])
    except (launchd.LaunchdError, OSError) as error:
        problem.touch(mode=0o600)
        store.note_problem("launchd_reconciliation_failed", error)
        raise store.Refused("launchd_reconciliation_failed") from error
    problem.unlink(missing_ok=True)


def retire_idle(tasks: store.Store) -> None:
    """Unload after the last admitted turn finishes."""
    if tasks.path.exists() and not _has_work(tasks):
        reconcile(tasks)


def _has_work(tasks: store.Store) -> bool:
    if any(task.definition["enabled"] and not task.connector_pause for task in tasks.tasks()):
        return True
    if not tasks.path.exists():
        return False
    with tasks.connect() as db:
        return db.execute("SELECT 1 FROM runs WHERE state='running' LIMIT 1").fetchone() is not None


def _cutover_in_progress(tasks: store.Store) -> bool:
    if not tasks.path.exists():
        return False
    with tasks.connect() as db:
        return db.execute("SELECT 1 FROM digest_migration WHERE phase='staged' AND problem IS NULL").fetchone() is not None


def _unload(path: Path, agent: launchd.Agent) -> None:
    # A dispatcher booting itself out may exit before launchctl returns.
    path.unlink(missing_ok=True)
    control = agent.launchctl or launchd.default_launchctl
    control(["bootout", launchd._target(path.stem)])


def _retire_legacy(path: Path, agent: launchd.Agent, home: str) -> None:
    try:
        content = plistlib.loads(beneath.read_bytes(path.parent, path.name))
    except (FileNotFoundError, plistlib.InvalidFileException, ExpatError):
        return
    environment = content.get("EnvironmentVariables") if isinstance(content, dict) else None
    if (isinstance(environment, dict) and environment.get("HOME") == home
            and engine_home.ENV_VARIABLE not in environment and content.get("Label") == path.stem):
        _unload(path, agent)


def _confirmed(path: Path, agent: launchd.Agent, content: dict) -> bool:
    control = agent.launchctl or launchd.default_launchctl
    return control(["print", launchd._target(path.stem)]) == 0 and path.read_bytes() == plistlib.dumps(content)


def kick() -> None:
    """Ask launchd to start an already queued manual run before the next pulse."""
    try:
        label = label_of(engine_home.root())
        status = launchd.default_launchctl(["kickstart", launchd._target(label)])
        if status:
            raise launchd.LaunchdError("the run remains queued for the next dispatcher pulse")
    except (OSError, launchd.LaunchdError) as error:
        store.note_problem("dispatcher_kick_pending", error)
