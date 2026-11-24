"""The project side of the cockpit's actions: a registered repository as a target (`["project", repo]`).

The only action that takes one is the start of a lane (`L`). A project is addressed by the path the engine's own
registry holds, and a request names it only through the snapshot, so a path a client invented is never a target.
Adding a project (`P`) is the one way a path a client names reaches the registry, and it gets there only as a folder
of a Git repository the engine can read (`add`).
"""

from __future__ import annotations

import json
import os
import stat
import sys
from dataclasses import dataclass
from pathlib import Path

from sakura import beneath, engine_home, git_program, programs
from sakura.flow import exits, gitutil, registry
from sakura.flow.render import strip_controls

KIND_PROJECT = "project"
GIT_DIR_NAME = ".git"
GITDIR_PREFIX = "gitdir:"
COMMONDIR_FILE = "commondir"
HEAD_FILE = "HEAD"
GIT_FOLDER_ENTRIES = ("objects", "refs")


@dataclass(frozen=True)
class ProjectTarget:
    """The target of a project action. `key` is `("project", repo)`."""

    repo: Path
    folder_missing: bool = False

    kind = KIND_PROJECT

    @property
    def key(self) -> tuple:
        return (KIND_PROJECT, str(self.repo))


def target_of(repo: Path, *, folder_missing: bool = False) -> ProjectTarget:
    """The target a registered repository of the snapshot stands for."""
    return ProjectTarget(repo, folder_missing=folder_missing)


# What reading a folder as a repository can raise; each becomes one fixed sentence.
_FOLDER_ERRORS = (git_program.GitProgramError, gitutil.GitError, engine_home.EngineHomeError, OSError)
NOT_ABSOLUTE = "the path is not absolute; choose the full path of a folder"
NOT_A_FOLDER = "the path is not a folder"
NOT_A_GIT_FOLDER = "the folder is not a Git repository; choose a folder of one"
WHERE_AN_AGENT_WRITES = "the folder lies where an agent can write; choose another folder"
CANNOT_USE = "the folder cannot be used as a project"
CANNOT_SAVE = "the project could not be saved"


def add(folder: str) -> tuple[int, str]:
    """Register the repository holding `folder`, as `sakura flow register` does, and say which one.

    Returns the exit code and the output: `exits.OK` with the project as JSON (`name`, `folder`) or `exits.USAGE`
    with one fixed sentence that says why the folder was not added. A sentence never repeats the folder or an error's
    own text, which can name host paths; that text goes to stderr only. Adding a known project changes nothing.

    The folder is found from the files on disk, never by running Git (whose trust is already set to the repositories
    registered so far), and its Git folders are resolved as Git does: the `.git` folder or the `gitdir:` of a `.git`
    file, then the `commondir` of that folder when it has one. The work tree, the Git folder and the common folder,
    symlinks resolved, may lie in no place an agent can write (`programs.agent_writable_roots`); then the folders
    must have the structure of a Git folder, read from the files alone (no Git process runs); only then is the
    repository trusted and registered.

    The folder is resolved once and every step uses that path. A folder swapped between the checks and the
    registration, by someone able to write outside every place an agent can, is not defended against: that is the
    person's own account or a session running as it.
    """
    if not os.path.isabs(folder):
        return exits.USAGE, NOT_ABSOLUTE
    chosen = Path(os.path.realpath(folder))  # resolved once: every step below gets this path, never the name given
    if not chosen.is_dir():
        return exits.USAGE, NOT_A_FOLDER
    try:
        found = _resolve(chosen)
        if found is None:
            return exits.USAGE, NOT_A_GIT_FOLDER
        if _is_where_an_agent_writes(found.work_tree, found.git_dir, found.common):
            return exits.USAGE, WHERE_AN_AGENT_WRITES
        _require_the_engine_reads_it_alike(chosen, found)
        if not _has_git_folder_structure(found):
            return exits.USAGE, NOT_A_GIT_FOLDER
        repo = found.common.parent
        gitutil.trust_repository(repo)
        registry.remember(repo)
    except git_program.GitProgramError as error:
        _unsaid(error, NOT_A_GIT_FOLDER)
        return exits.USAGE, NOT_A_GIT_FOLDER
    except registry.RegistryError as error:
        return exits.FAILED, _unsaid(error, CANNOT_SAVE)
    except _FOLDER_ERRORS as error:
        return exits.USAGE, _unsaid(error, CANNOT_USE)
    return exits.OK, json.dumps({"name": repo.name, "folder": str(repo)})


@dataclass(frozen=True)
class _Found:
    """The real folders of the repository a candidate belongs to."""

    work_tree: Path
    git_dir: Path
    common: Path


def _resolve(path: Path) -> _Found | None:
    """The work tree, Git folder and common folder of the repository holding `path`, as Git resolves them; None when
    `path` is in no repository with a `.git` main folder. Raises GitProgramError when the files cannot be read."""
    entries = git_program.controlled_folders(path)
    if not entries:
        return None
    work_tree = Path(entries[0])
    git_dir = _git_dir_of(work_tree)
    common = _common_of(git_dir)
    if common.name != GIT_DIR_NAME:
        return None
    if os.path.lexists(common / COMMONDIR_FILE):
        raise git_program.GitProgramError(f"{common} names another common folder; the engine follows one pointer only")
    return _Found(work_tree, git_dir, common)


def _require_the_engine_reads_it_alike(path: Path, found: _Found) -> None:
    """Raise GitProgramError unless the engine's own reading of the folder (`git_program.common_dir_of`) names the
    common folder Git does: they would otherwise work on different repositories."""
    if found.common != Path(os.path.realpath(git_program.common_dir_of(path))):
        raise git_program.GitProgramError(f"{found.git_dir} names {found.common} as its common folder, which the "
                                          "engine does not read")


def _has_git_folder_structure(found: _Found) -> bool:
    """Whether the Git folder holds `HEAD` and the common folder `objects` and `refs`, as Git's own check of a Git
    folder wants: read from the files, so no Git process and no repository configuration is involved."""
    return (os.path.lexists(found.git_dir / HEAD_FILE)
            and all((found.common / entry).is_dir() for entry in GIT_FOLDER_ENTRIES))


def _git_dir_of(work_tree: Path) -> Path:
    """The Git folder of `work_tree`: its `.git` folder, or the folder a `.git` file names with `gitdir:`."""
    marker = work_tree / GIT_DIR_NAME
    if stat.S_ISDIR(os.lstat(marker).st_mode):
        return Path(os.path.realpath(marker))
    text = beneath.read_bytes(work_tree, GIT_DIR_NAME).decode(errors="replace").strip()
    if not text.startswith(GITDIR_PREFIX):
        raise git_program.GitProgramError(f"{marker} is not a gitdir file")
    return Path(os.path.realpath(work_tree / text.removeprefix(GITDIR_PREFIX).strip()))


def _common_of(git_dir: Path) -> Path:
    """The common folder of the Git folder `git_dir`: the one its `commondir` file names, else itself."""
    try:
        named = beneath.read_bytes(git_dir, COMMONDIR_FILE).decode(errors="replace").strip()
    except FileNotFoundError:
        return git_dir
    return Path(os.path.realpath(git_dir / named))


def _is_where_an_agent_writes(*places: Path) -> bool:
    """Whether any of `places` is, or lies under, a folder an agent can write; a list of those that cannot be made is a yes."""
    try:
        roots = programs.agent_writable_roots()
    except Exception as error:  # noqa: BLE001  (a list that cannot be made is a refusal, never a shorter list)
        _unsaid(error, "")
        return True
    # A path below the place stands for the place itself: `holding_folder` looks at the parents of what it is given.
    return any(git_program.holding_folder(str(Path(os.path.realpath(place)) / "_"), roots) for place in places)


def _unsaid(error: Exception, sentence: str) -> str:
    """`sentence`, after the error's own text went to stderr, where only the machine's owner reads it."""
    print(f"cockpit: add project: {strip_controls(str(error))}", file=sys.stderr)
    return sentence
