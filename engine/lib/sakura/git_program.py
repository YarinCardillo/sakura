"""The programs every engine Git call runs: one Git and one ssh, resolved once per process, never ones a repository
controls, and a fixed search path for whatever Git starts itself.

`PATH` is the person's, and it can name a folder a repository controls (a workspace's virtual environment, `.`, any
relative entry): a `git` found there would run with the engine's rights, and so would any program Git looks up on
that PATH (ssh, a credential helper). So the engine resolves `git` and `ssh` once, to absolute real paths, refuses
one when the PATH entry it came from is relative or empty, and refuses it for every call on a repository whose
folders hold it: the main worktree, the Git folder and every linked worktree, found from the files on disk, without
running Git, and compared by device and inode, so a path spelled in another case on a case-insensitive volume is
still caught. Every engine Git call then runs with `PATH` fixed to the pinned Git's own folder and the system
folders (`SAFE_PATH`), and with `GIT_SSH_COMMAND` naming the pinned ssh.

Limit: a credential helper the person's Git config names by a bare program name must live in one of those folders
(`gh auth setup-git` writes an absolute path, which works).
"""

from __future__ import annotations

import os
import shlex
import stat
import subprocess
from pathlib import Path
from typing import Mapping

from sakura import beneath

GIT = "git"
SSH = "ssh"
SAFE_PATH = ("/usr/bin", "/bin", "/usr/sbin", "/sbin")
# An agent can plant `refs/replace/<sha>` (Git then shows another commit's tree under that sha) or `info/grafts` (it
# rewrites parents). Replacement objects and the graft file are never honoured, and `info/grafts` is refused outright.
NO_REPLACE = {"GIT_NO_REPLACE_OBJECTS": "1", "GIT_GRAFT_FILE": "/dev/null"}
# Variables that point Git at another object database: never passed to an engine Git call.
OBJECT_DIRECTORY_VARIABLES = ("GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_OBJECT_DIRECTORY")
GRAFTS_FILE = os.path.join("info", "grafts")
# Hooks, the filesystem monitor, signing and signature-checking programs and the ext transport are code a config can
# name: off for the engine. Every transport is allowed only where the engine asks for it itself (`user`).
HARDENED_CONFIG = ("-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false", "-c", "commit.gpgsign=false",
                   "-c", "tag.gpgsign=false", "-c", "gpg.program=false", "-c", "gpg.ssh.program=false",
                   "-c", "gpg.x509.program=false", "-c", "log.showSignature=false", "-c", "merge.verifySignatures=false",
                   "-c", "core.attributesFile=/dev/null", "-c", "protocol.allow=user", "-c", "protocol.ext.allow=never")
WORKTREES_DIR = "worktrees"
GITDIR_FILE = "gitdir"
_resolved: dict[str, str] = {}


class GitProgramError(RuntimeError):
    """No usable program: none on PATH, or the one PATH names lies where a repository can replace it."""


def path(name: str = GIT) -> str:
    """The absolute real path of program `name`, resolved from PATH on first use and kept for the process.

    Raises GitProgramError when PATH holds no such program or resolution lands in a relative or empty entry.
    """
    if name not in _resolved:
        _resolved[name] = _resolve(name, os.environ.get("PATH", ""))
    return _resolved[name]


def for_repository(location: Path, name: str = GIT) -> str:
    """The pinned program `name`, refused (GitProgramError) when it lies inside the repository holding `location`."""
    program = path(name)
    folder = holding_folder(program, controlled_folders(location))
    if folder:
        raise GitProgramError(f"the {name} on PATH ({program}) lies in {folder}, a folder the repository controls; "
                              f"put a host {name} (such as /usr/bin/{name}) before it on PATH")
    return program


def environment(location: Path, base: Mapping[str, str]) -> dict[str, str]:
    """`base` for a Git call in `location`: `PATH` fixed to the pinned Git's folder and the system folders, and
    `GIT_SSH_COMMAND` naming the pinned ssh, and replacement objects switched off (left out when there is no ssh on PATH, so Git searches that fixed PATH).
    No variable of `base` that names another object database (`OBJECT_DIRECTORY_VARIABLES`) is kept.

    Raises GitProgramError as `for_repository` does, for Git and for ssh.
    """
    folders = [os.path.dirname(for_repository(location)), *SAFE_PATH]
    kept = {key: value for key, value in base.items() if key not in OBJECT_DIRECTORY_VARIABLES}
    env = {**kept, "PATH": os.pathsep.join(dict.fromkeys(folders)), **NO_REPLACE}
    try:
        env["GIT_SSH_COMMAND"] = shlex.quote(for_repository(location, SSH))
    except GitProgramError as error:
        if str(error) != _missing(SSH):
            raise
        env.pop("GIT_SSH_COMMAND", None)
    return env


def command(location: Path, args: tuple[str, ...], base: Mapping[str, str],
            hardened: bool = True) -> tuple[list[str], dict[str, str]]:
    """The argv and environment of one Git call in `location`: the only place the engine builds one.

    The pinned Git (refused inside the repository), `base` scrubbed to the fixed `PATH`, the pinned ssh, no
    replacement objects and no graft file, the hardened config (`hardened=False` leaves out the `-c` pairs, for a call
    that must read the effective config as it is) and `-C location`. Fails closed: GitProgramError when the
    repository's common folder cannot be resolved from the files on disk, or `info/grafts` exists there.
    """
    program = for_repository(location)
    require_unrewritten_history(common_dir_of(location))
    argv = [program, *(HARDENED_CONFIG if hardened else ()), "-C", str(location), *args]
    return argv, environment(location, base)


def run(location: Path, args: tuple[str, ...], base: Mapping[str, str], *, text: bool = True,
        timeout: float | None = None, hardened: bool = True) -> subprocess.CompletedProcess:
    """`command`, run to the end with no stdin; the result whatever its exit code."""
    argv, env = command(location, args, base, hardened)
    return subprocess.run(argv, capture_output=True, text=text, check=False, timeout=timeout, env=env,
                          stdin=subprocess.DEVNULL)


def init_bare_command(folder: Path, object_format: str, base: Mapping[str, str]) -> tuple[list[str], dict[str, str]]:
    """The argv and environment of `git init --bare` making the engine's own repository `folder`: the pinned Git, the
    hardened config, no template (no sample hooks, no exclude file) and the source repository's object format."""
    argv = [path(), *HARDENED_CONFIG, "init", "--quiet", "--bare", "--template=", f"--object-format={object_format}",
            str(folder)]
    return argv, environment(folder, base)


def common_dir_of(location: Path) -> Path:
    """The common Git folder of the repository holding `location`, read from the files on disk (never by running Git).

    A folder that is itself a bare repository (the engine's own store, `flow/sealed.py`) is its own common folder.
    Raises GitProgramError when `location` is in no repository or its `.git` cannot be read.
    """
    real = Path(os.path.realpath(location))
    if _is_bare_repository(real):
        return real
    worktree = _enclosing_worktree(real)
    if worktree is None:
        raise GitProgramError(f"{location} is in no Git repository the engine can resolve")
    return _common_dir(worktree / ".git")


def require_unrewritten_history(common: Path) -> None:
    """Raise GitProgramError while `info/grafts` exists in `common`: no setting turns the graft file off for every
    Git command, and it rewrites commit parents."""
    if os.path.lexists(common / GRAFTS_FILE):
        raise GitProgramError(f"{common / GRAFTS_FILE} exists: it rewrites history for Git and the engine runs no Git "
                              "command on a repository with it; remove the file")


def controlled_folders(location: Path) -> list[str]:
    """The real paths of the repository holding `location`: its worktree, Git folder, main and linked worktrees.

    Fails closed: a `.git` or a worktree pointer that is there but cannot be read as a regular file raises
    GitProgramError, since the folders it would name stay unknown.
    """
    worktree = _enclosing_worktree(Path(os.path.realpath(location)))
    if worktree is None:
        return []
    common = _common_dir(worktree / ".git")
    main = common.parent if common.name == ".git" else common
    linked = []
    entries = common / WORKTREES_DIR
    for entry in sorted(entries.iterdir()) if entries.is_dir() else []:
        try:
            pointer = beneath.read_bytes(entry, GITDIR_FILE).decode(errors="replace").strip()
        except FileNotFoundError:
            continue
        except OSError as error:
            raise GitProgramError(f"{entry / GITDIR_FILE} cannot be read as a regular file ({error}); the engine "
                                  "cannot tell which worktree it names") from None
        linked.append(os.path.realpath(os.path.dirname(pointer)))
    return list(dict.fromkeys([str(worktree), str(common), str(main), *linked]))


def holding_folder(program: str, folders: list[str]) -> str:
    """The folder among `folders` that holds `program`, compared by device and inode of each of its parents."""
    identities = {}
    for folder in folders:
        try:
            info = os.stat(folder)
        except OSError:
            continue
        identities[(info.st_dev, info.st_ino)] = folder
    for parent in Path(program).parents:
        try:
            info = os.stat(parent)
        except OSError:
            continue
        if (info.st_dev, info.st_ino) in identities:
            return identities[(info.st_dev, info.st_ino)]
    return ""


def _resolve(name: str, search_path: str) -> str:
    """The first copy of `name` on `search_path` that passes the trust check (`programs.untrusted`) against the places an agent
    can write that are known without running Git (`programs.fixed_roots`: the full list reads the registry and the
    repositories' configuration, which runs Git itself, so it is never asked of the program that runs it). Raises
    GitProgramError."""
    from sakura import programs

    refused = []
    for entry in search_path.split(os.pathsep):
        candidate = os.path.join(entry or ".", name)
        if not (os.path.isfile(candidate) and os.access(candidate, os.X_OK)):
            continue
        if not entry or not os.path.isabs(entry):
            raise GitProgramError(f"{name} resolves through the relative PATH entry {entry or '(empty)'!r}, which "
                                  "names whatever folder the engine runs in; remove it from PATH")
        reason = programs.untrusted(candidate, None, programs.fixed_roots())
        if not reason:
            return os.path.realpath(candidate)
        refused.append(f"{candidate} ({reason})")
    if refused:
        raise GitProgramError(f"no {name} on PATH may be run outside the cage: {'; '.join(refused)}")
    raise GitProgramError(_missing(name))


def _missing(name: str) -> str:
    return f"no {name} on PATH"


def _is_bare_repository(folder: Path) -> bool:
    """Whether `folder` itself is a bare repository: Git's own marks (HEAD, objects, refs) and no `.git` in it."""
    return (not os.path.lexists(folder / ".git") and (folder / "HEAD").is_file() and (folder / "objects").is_dir()
            and (folder / "refs").is_dir())


def worktree_of(location: Path) -> Path | None:
    """The real path of the worktree holding `location` (the nearest folder with a `.git` entry), read from the files
    on disk, never by running Git; None outside every worktree."""
    return _enclosing_worktree(Path(os.path.realpath(location)))


def _enclosing_worktree(start: Path) -> Path | None:
    for folder in (start, *start.parents):
        if os.path.lexists(folder / ".git"):
            return folder
    return None


def _common_dir(marker: Path) -> Path:
    """The common Git folder behind a `.git` folder or `.git` file; GitProgramError when it cannot be read."""
    if stat.S_ISDIR(os.lstat(marker).st_mode):
        return Path(os.path.realpath(marker))
    try:
        text = beneath.read_bytes(marker.parent, marker.name).decode(errors="replace").strip()
    except OSError as error:
        raise GitProgramError(f"{marker} cannot be read as a regular file ({error}); the engine cannot tell which "
                              "repository it belongs to") from None
    if not text.startswith("gitdir:"):
        raise GitProgramError(f"{marker} cannot be read as a gitdir file; the engine cannot tell which repository "
                              "it belongs to")
    gitdir = Path(os.path.realpath(marker.parent / text.removeprefix("gitdir:").strip()))
    return gitdir.parent.parent if gitdir.parent.name == WORKTREES_DIR else gitdir
