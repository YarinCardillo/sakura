"""The repositories flow commands have run in, so the cockpit knows where to look."""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
from collections.abc import Iterator
from pathlib import Path

from sakura import engine_home, git_program
from sakura.flow import gates

REGISTRY_NAME = "repos.json"
# Where the registry lived before it followed the engine home: adopted once by the default engine home.
LEGACY_REGISTRY_FILE = Path(".local/state/sakura/flow/repos.json")
LOCK_SUFFIX = ".lock"
REPOS_KEY = "repos"
COMMON_DIRS_KEY = "common_dirs"


class RegistryError(RuntimeError):
    """Raised when the registry file cannot be read or written."""


def default_path() -> Path:
    """The registry file in the engine home, so a fresh `SAKURA_ENGINE_HOME` has a registry of its own.

    Raises RegistryError when `SAKURA_ENGINE_HOME` names a folder the engine must not use.
    """
    try:
        return engine_home.root() / REGISTRY_NAME
    except engine_home.EngineHomeError as error:
        raise RegistryError(str(error)) from error
    except OSError as error:
        # Inside an agent cage the engine home cannot even be looked at.
        raise RegistryError(f"cannot reach the engine home: {error}") from error


def remember(repo: Path, path: Path | None = None) -> None:
    """Add `repo` (resolved) to the registry under an exclusive lock; a known repository changes nothing.

    Raises RegistryError when the file is unreadable (it is left as it is) or cannot be written.
    """
    target = path or _default_target()
    resolved = repo.resolve()
    try:
        canonical, common = _identity(resolved)
        with _locked(target):
            known, common_dirs = _read(target)
            repos = list(dict.fromkeys(str(canonical) if entry == str(resolved) else entry for entry in known))
            if str(canonical) not in repos:
                repos.append(str(canonical))
            updated = {**common_dirs, str(canonical): str(common)} if common is not None else common_dirs
            if repos != known or updated != common_dirs:
                gates.write_atomically(target, _encode(repos, updated))
    except (OSError, engine_home.EngineHomeError) as error:
        raise RegistryError(f"cannot write {target}: {error}") from error


def _identity(repo: Path) -> tuple[Path, Path | None]:
    try:
        common = git_program.common_dir_of(repo)
    except git_program.GitProgramError:
        return repo, None
    common = engine_home.project_common_dir(common)
    return engine_home.main_worktree(common), common


def repos(path: Path | None = None, *, include_missing: bool = False) -> list[Path]:
    """Return registered repositories in order. Include missing folders when requested."""
    entries, _ = _read(path or _default_target())
    found = (Path(entry) for entry in entries)
    return [entry for entry in found if include_missing or entry.is_dir()]


def saved_common_dir(repo: Path) -> Path | None:
    """Return the Git identity saved for a canonical project, even if it is missing."""
    _, common_dirs = _read(default_path())
    return Path(found) if (found := common_dirs.get(str(repo))) else None


def _default_target() -> Path:
    """The registry file of the engine home; the default engine home adopts the legacy file first."""
    target = default_path()
    if not os.environ.get(engine_home.ENV_VARIABLE):
        _adopt_legacy(target)
    return target


def _adopt_legacy(target: Path) -> None:
    """Copy the entries of the legacy file into `target` once, including missing paths.

    Nothing happens when `target` exists or there is no legacy file. A legacy file that cannot be read is ignored:
    the registry starts empty and fills as flow commands run.
    """
    legacy = Path.home() / LEGACY_REGISTRY_FILE
    if target.exists() or not legacy.is_file():
        return
    try:
        entries, common_dirs = _read(legacy)
        with _locked(target):
            if not target.exists():
                gates.write_atomically(target, _encode(entries, common_dirs))
    except RegistryError:
        return
    except OSError as error:
        raise RegistryError(f"cannot write {target}: {error}") from error


@contextlib.contextmanager
def _locked(target: Path) -> Iterator[None]:
    target.parent.mkdir(mode=engine_home.DIRECTORY_MODE, parents=True, exist_ok=True)
    with target.with_name(target.name + LOCK_SUFFIX).open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def _read(target: Path) -> tuple[list[str], dict[str, str]]:
    try:
        document = json.loads(target.read_text())
        entries = document[REPOS_KEY]
        common_dirs = document.get(COMMON_DIRS_KEY, {})
    except FileNotFoundError:
        return [], {}
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise RegistryError(f"cannot read {target}: {error}") from error
    if not isinstance(entries, list):
        raise RegistryError(f"cannot read {target}: {REPOS_KEY} is not a list")
    if not isinstance(common_dirs, dict):
        raise RegistryError(f"cannot read {target}: {COMMON_DIRS_KEY} is not an object")
    return [entry for entry in entries if isinstance(entry, str)], {
        repo: common for repo, common in common_dirs.items() if isinstance(common, str) and Path(common).is_absolute()
    }


def _encode(entries: list[str], common_dirs: dict[str, str]) -> bytes:
    return json.dumps({REPOS_KEY: entries, COMMON_DIRS_KEY: common_dirs}, indent=2).encode()
