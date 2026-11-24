"""The engine's host folder: task state and config live here, never inside a repository.

A workspace and its `.git` can be written by the agents that work in it, so nothing the engine trusts
(contract, ledger, verdicts, config) is kept there. The folder is
`<account home>/Library/Application Support/Sakura/engine`, the account home read from the password
database, not from `$HOME`. Each repository gets a project folder named after its main worktree plus a
short hash of that worktree's absolute path, shared by all its linked worktrees.

`SAKURA_ENGINE_HOME` moves task state and config (`root`) to another folder, which must already exist,
belong to the user and have mode 0700, and the records that are not trust decisions (`state_dir`: the cockpit's
decided asks and action log, the lane events). It never moves what hooks, brakes and stamp checks trust
(`root_for_trust`), nor what signs or verifies (the cockpit's proof secret): an agent can set a variable on its
own command, a `git commit` that runs our hooks included, so those ignore the environment entirely.
"""

from __future__ import annotations

import contextlib
import hashlib
import os
import pwd
import stat
from pathlib import Path

from sakura import beneath, repository

ENV_VARIABLE = "SAKURA_ENGINE_HOME"
# The file at the top of a task's own repository (`flow/task_repository.py`) in which the engine names the common Git
# folder of the person's repository it was made from; the cage denies every write to it.
TASK_REPOSITORY_RECORD = "sakura-project"
ENGINE_PLACEHOLDER = "{engine}"
TRUST_NOTICE_MARKER = ".trust-records-noted"
DEFAULT_RELATIVE = Path("Library") / "Application Support" / "Sakura" / "engine"
CONFIG_NAME = "config.toml"
STATE_FOLDER_NAME = "state"
HOME_STATE_RELATIVE = Path(".local") / "state" / "sakura"
REMEMBERED_CHECKS_NAME = "remembered-checks.toml"
FLOW_DIR_NAME = "flow"
KIT_DIR_NAME = "kit"
HASH_LENGTH = 8
DIRECTORY_MODE = 0o700


class EngineHomeError(RuntimeError):
    """Raised when `SAKURA_ENGINE_HOME` names a folder the engine must not use."""


def root(home: Path | None = None) -> Path:
    """The folder of task state and config: a valid `SAKURA_ENGINE_HOME`, else the default.

    `home` stands in for the account home (tests). Raises EngineHomeError for an override that does not
    exist, is a symlink, belongs to another user or is open to group or others.
    """
    named = os.environ.get(ENV_VARIABLE, "")
    if named:
        return _checked_override(Path(named))
    return home / DEFAULT_RELATIVE if home else root_for_trust()


def state_dir(home: Path | None = None) -> Path:
    """The folder of the host's records that decide nothing about trust: the cockpit's decided asks and action log, its
    access log, the lane events.

    Under the engine home when `SAKURA_ENGINE_HOME` names one, so a throwaway engine home keeps them; otherwise
    `~/.local/state/sakura` (`home` stands in for `$HOME`, tests). The trust records and the proof secret never come
    from here: see `root_for_trust` and `cockpit/proofstore.py`.
    """
    if os.environ.get(ENV_VARIABLE):
        return root() / STATE_FOLDER_NAME
    return (home or Path.home()) / HOME_STATE_RELATIVE


def with_engine_root(template: str) -> str:
    """`template` with `{engine}` replaced by the engine home (`root`); a template without it is returned as it is.

    The defaults of the places the engine writes to (workspaces, reports) name the engine home this way instead
    of a folder of the person's own.
    """
    return template.replace(ENGINE_PLACEHOLDER, str(root())) if ENGINE_PLACEHOLDER in template else template


def trust_records_notice() -> str:
    """One line saying where the trust records stay when `SAKURA_ENGINE_HOME` moved the rest; "" otherwise.

    Shown once per engine home: a marker file in it records that the person was told.
    """
    home, trust = root(), root_for_trust()
    marker = home / TRUST_NOTICE_MARKER
    if home == trust or marker.exists():
        return ""
    with contextlib.suppress(OSError):
        marker.write_text(f"{trust}\n")
    return (f"note: {ENV_VARIABLE} moves task state and config to {home}; trust records (test stamps, hook "
            f"fingerprints, Codex trust) stay in {trust}")


def root_for_trust() -> Path:
    """The folder of trust records (stamps, fingerprints, trusted hooks, Codex trust, role sessions).

    Ignores `SAKURA_ENGINE_HOME` and `$HOME`: hooks, brakes and stamp checks read it.
    """
    return account_root()


def account_root() -> Path:
    """The default engine folder under the account home of the current user, from the password database."""
    return Path(pwd.getpwuid(os.getuid()).pw_dir) / DEFAULT_RELATIVE


def _checked_override(path: Path) -> Path:
    try:
        info = path.lstat()
    except FileNotFoundError:
        raise EngineHomeError(f"{ENV_VARIABLE}={path} does not exist; create it with mode 0700") from None
    if stat.S_ISLNK(info.st_mode):
        raise EngineHomeError(f"{ENV_VARIABLE}={path} is a symlink; name the folder itself")
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != DIRECTORY_MODE:
        raise EngineHomeError(f"{ENV_VARIABLE}={path} must be a folder owned by you with mode 0700")
    return path


PROBE_NAME = ".operator"


def sandboxed() -> bool:
    """True when this process cannot write the engine's trust root: it runs inside an agent cage.

    The sandbox itself says so, not a variable an agent could unset: an operator command opens (or creates) a
    probe file there, which every cage the engine writes forbids.
    """
    root = root_for_trust()
    flags = os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW
    try:
        try:
            descriptor = os.open(root / PROBE_NAME, flags, 0o600)
        except FileNotFoundError:
            root.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
            descriptor = os.open(root / PROBE_NAME, flags, 0o600)
    except (PermissionError, FileExistsError):
        # FileExistsError: the folder is there but the sandbox hides it from this process.
        return True
    os.close(descriptor)
    return False


def config_file(home: Path | None = None) -> Path:
    """The personal engine config, shared by every project."""
    return root(home) / CONFIG_NAME


def project_id(repo: Path) -> str:
    """`<main worktree name>-<hash>`: stable for the repository and the same from any of its worktrees."""
    return project_id_for(repository.common_dir(repo))


def project_id_for(common_dir: Path) -> str:
    """The project id of the repository whose common Git directory is `common_dir`."""
    return project_id_for_path(main_worktree(common_dir))


def project_id_for_path(main: Path) -> str:
    """The project id from its main worktree or bare repository path, even if the folder is missing."""
    digest = hashlib.sha256(str(main).encode()).hexdigest()[:HASH_LENGTH]
    return f"{main.name}-{digest}"


def main_worktree(common_dir: Path) -> Path:
    """The main worktree of a common Git directory; the directory itself for a bare repository. A task's own
    repository (`flow/task_repository.py`) stands for the person's repository it was made from
    (`project_common_dir`), so a workspace of a task belongs to that project."""
    common_dir = project_common_dir(common_dir)
    return common_dir.parent if common_dir.name == ".git" else common_dir


def project_common_dir(common_dir: Path) -> Path:
    """`common_dir`, or for a task's own repository the common Git directory of the person's repository its record
    names (`TASK_REPOSITORY_RECORD`), exactly as the engine resolved it when it made the repository.

    Raises EngineHomeError for a record that is a link, not a regular file or not an absolute path.
    """
    try:
        text = beneath.read_bytes(common_dir, TASK_REPOSITORY_RECORD).decode().strip()
    except FileNotFoundError:
        return common_dir
    except (OSError, UnicodeDecodeError) as error:
        raise EngineHomeError(f"{common_dir / TASK_REPOSITORY_RECORD} cannot be read as the engine's record: "
                              f"{error}") from None
    if TASK_REPOSITORY_RECORD not in os.listdir(common_dir):
        # A case-insensitive volume finds a case variant too, which the engine never writes.
        raise EngineHomeError(f"the project record in {common_dir} is not named exactly {TASK_REPOSITORY_RECORD}")
    if not os.path.isabs(text):
        raise EngineHomeError(f"{common_dir / TASK_REPOSITORY_RECORD} does not name a Git folder")
    return Path(text)


def project_dir(repo: Path, home: Path | None = None) -> Path:
    """The folder of one repository's tasks and per-project config."""
    return root(home) / project_id(repo)


def flow_dir(repo: Path) -> Path:
    """The folder of one repository's flow tasks (`<project>/flow/<task>/`) and their calibration."""
    return project_dir(repo) / FLOW_DIR_NAME


def flow_dir_for(common_dir: Path) -> Path:
    """`flow_dir` from an already resolved common Git directory, so no git process is spawned."""
    return root() / project_id_for(common_dir) / FLOW_DIR_NAME


def kit_dir(repo: Path) -> Path:
    """The kit's trust records for one repository: test stamps, guard fingerprints, trusted hooks, sessions.

    Always under `root_for_trust`, whatever `SAKURA_ENGINE_HOME` says.
    """
    return root_for_trust() / project_id(repo) / KIT_DIR_NAME


def project_config_file(repo: Path, home: Path | None = None) -> Path:
    """The per-project engine config, a layer between the shared repository config and the local overlay."""
    return project_dir(repo, home) / CONFIG_NAME


def remembered_checks_file(repo: Path, home: Path | None = None) -> Path:
    """The contract commands the person chose to approve for good in one repository (`approve checks --remember`)."""
    return project_dir(repo, home) / REMEMBERED_CHECKS_NAME


def ensure_project_dir(repo: Path) -> Path:
    """Create the project folder, and the engine folder above it, private to the user; returns the project folder."""
    return ensure_private(project_dir(repo))


def ensure_private(project: Path) -> Path:
    """Create `project` and the engine folder that holds it with mode 0700; returns `project`."""
    for directory in (project.parent, project):
        directory.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
        directory.chmod(DIRECTORY_MODE)
    return project
