"""The repository's local Git config, as the engine's own Git calls consume it: pinned at task start, refused when
it names a program.

Engine Git calls ignore the system config and, except the network ones, the person's global config, but they still
read the repository's local config: the common folder's `config` and the `config.worktree` files of the main and
linked worktrees. A filter, merge or diff driver, an SSH command, a proxy, a pack program or a credential helper
there would run with the engine's rights during a checkout, a merge or a push. So a task does not start while the
local config names one (`executable_entries`), and its digest is recorded at the start; `gitutil` refuses every
engine Git call once it changed. The keys the person's own `sakura install` writes (the worktree config switch and the hook path, which engine
calls override anyway) are left out of the digest. The engine writes no key of this config.

Trust boundary: this config lives outside every workspace, the cage denies writes to it and its digest is checked
for every engine Git call of the task, so it is the person's own configuration and trusted. The check of the keys
that name a program is a courtesy guard against surprises, not the barrier.

No role reads this config: every workspace is a worktree of the task's own repository (`flow/task_repository.py`),
whose config the engine writes, and no cage is granted the person's Git folder. A secret the person keeps here (a
password for `sendemail`, a token in a remote URL) stays with the engine, which takes from it only what delivery needs
(`transport_entries`). A name heuristic for secrets was the control before; it could never tell every secret apart,
and it is gone.

The digest is cached on the files' identity (inode, size, mtime and ctime, which no process can set back), so a call
that finds the files untouched spawns no Git.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import threading
from pathlib import Path

from sakura import git_program

PARSE_TIMEOUT_SECONDS = 30
WORKTREE_CONFIG = "config.worktree"
# Written by the person's own `sakura install`, never by the engine; engine Git calls set `core.hooksPath=/dev/null`.
ENGINE_OWNED_KEYS = frozenset({"extensions.worktreeconfig", "core.hookspath"})
EXECUTABLE_KEYS = tuple(re.compile(pattern) for pattern in (
    r"filter\..+\.(smudge|clean|process)", r"merge\..+\.driver", r"diff\..+\.(command|textconv)", r"diff\.external",
    r"core\.(sshcommand|gitproxy|askpass)", r"remote\..+\.(receivepack|uploadpack)", r"include\.path",
    r"includeif\..+\.path", r"protocol\.ext\.allow", r"gpg\.(.+\.)?program", r"gpg\.ssh\.defaultkeycommand",
    r"core\.(editor|alternaterefscommand)", r"sequence\.editor"))
# A subsection shown as written: a plain name such as a remote's. Anything else (a URL, a path) is never printed.
PLAIN_SUBSECTION = re.compile(r"[A-Za-z0-9_-]{1,40}")
REDACTED_SUBSECTION = "<redacted>"
# The sections of the local config delivery takes from the person's repository: signing in to and reaching the remote.
TRANSPORT_SECTIONS = ("credential", "http")
BOOLEAN_WORDS = frozenset({"", "true", "false", "yes", "no", "on", "off", "1", "0"})
CREDENTIAL_HELPER = re.compile(r"credential\.(.+\.)?helper")
_cache: dict[str, tuple[tuple, str]] = {}
_cache_lock = threading.Lock()


class GitConfigError(RuntimeError):
    """The repository's local Git config cannot be read."""


def files(common_dir: Path) -> list[Path]:
    """The local config files Git reads for the repository's worktrees, existing or not."""
    linked = sorted((common_dir / "worktrees").glob(f"*/{WORKTREE_CONFIG}")) if (common_dir / "worktrees").is_dir() else []
    return [common_dir / "config", common_dir / WORKTREE_CONFIG, *linked]


def digest(common_dir: Path) -> str:
    """The digest of every local config entry the engine's Git consumes (engine-owned keys left out)."""
    key = os.path.realpath(common_dir)
    signature = tuple(_identity(path) for path in files(common_dir))
    with _cache_lock:
        cached = _cache.get(key)
        if cached and cached[0] == signature:
            return cached[1]
    entries = [[_label(common_dir, path), name, value] for path in files(common_dir) for name, value in _entries(path)
               if name not in ENGINE_OWNED_KEYS]
    value = hashlib.sha256(json.dumps(entries).encode()).hexdigest()
    with _cache_lock:
        _cache[key] = (signature, value)
    return value


def executable_entries(common_dir: Path, repo: Path) -> list[str]:
    """`key (file)` for every local config entry that names a program the engine's Git would run."""
    found = []
    for path in files(common_dir):
        for name, value in _entries(path):
            if _is_executable(name, value, (repo, common_dir)):
                found.append(f"{_redacted(name)} ({_label(common_dir, path)})")
    return found


def transport_entries(common_dir: Path) -> list[tuple[str, str]]:
    """The `credential.*` and `http.*` entries of the repository's local config (`config`, then the main worktree's
    `config.worktree`), names as Git prints them: what the engine's push from its own store needs from the person's
    repository to reach and sign in to the remote. Read and used by the engine only, outside every cage."""
    return [(name, value) for path in (common_dir / "config", common_dir / WORKTREE_CONFIG)
            for name, value in _pairs(path) if name.split(".", 1)[0].lower() in TRANSPORT_SECTIONS]


def _redacted(name: str) -> str:
    """A key name as it may be shown: the section, a plain subsection (`remote.origin.url`) and the last key part.

    A subsection can be a URL (`url.<base>.insteadof`, `credential.<url>.helper`) with userinfo, a query or a
    fragment in it, so any subsection that is not a plain name is replaced.
    """
    section, _, rest = name.partition(".")
    subsection, _, key = rest.rpartition(".")
    if not subsection:
        return name
    shown = subsection if PLAIN_SUBSECTION.fullmatch(subsection) else REDACTED_SUBSECTION
    return f"{section}.{shown}.{key}"


def _is_executable(name: str, value: str, repository_folders: tuple[Path, ...]) -> bool:
    if any(pattern.fullmatch(name) for pattern in EXECUTABLE_KEYS):
        return not (name == "protocol.ext.allow" and value.strip().lower() == "never")
    if name == "core.fsmonitor":
        return value.strip().lower() not in BOOLEAN_WORDS
    if CREDENTIAL_HELPER.fullmatch(name):
        return _helper_in_repository(value, repository_folders)
    return False


def _helper_in_repository(value: str, repository_folders: tuple[Path, ...]) -> bool:
    """A shell snippet, a relative path (resolved in the workspace) or a path inside the repository's folders."""
    helper = value.strip()
    if helper.startswith("!"):
        return True
    program = helper.split()[0] if helper.split() else ""
    if "/" not in program:
        return False
    if not os.path.isabs(program):
        return True
    real = os.path.realpath(program)
    return any(real == str(folder) or real.startswith(str(folder) + os.sep)
               for folder in (os.path.realpath(folder) for folder in repository_folders))


def _entries(path: Path) -> list[tuple[str, str]]:
    return [(name.lower(), value) for name, value in _pairs(path)]


def _pairs(path: Path) -> list[tuple[str, str]]:
    if not path.is_file():
        return []
    base = {"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "HOME": os.environ.get("HOME", "")}
    try:
        done = git_program.run(path.parent, ("config", "--file", str(path), "--list", "--null"), base, text=False,
                               timeout=PARSE_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise GitConfigError(f"reading {path} timed out") from error
    except git_program.GitProgramError as error:
        raise GitConfigError(str(error)) from error
    if done.returncode:
        raise GitConfigError(f"cannot read {path}: {done.stderr.decode(errors='replace').strip()}")
    pairs = []
    for item in done.stdout.decode(errors="replace").split("\0"):
        if item:
            name, _, value = item.partition("\n")
            pairs.append((name, value))
    return pairs


def _identity(path: Path) -> tuple:
    try:
        info = os.stat(path)
    except FileNotFoundError:
        return (str(path),)
    return (str(path), info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _label(common_dir: Path, path: Path) -> str:
    return str(path.relative_to(common_dir))
