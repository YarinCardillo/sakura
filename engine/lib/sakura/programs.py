"""Every program the engine runs outside a cage, resolved once and never one a repository controls.

`PATH` is the person's, and it can name a folder a repository controls: a workspace's virtual environment, a
relative entry, `.`. A `gh` planted there would run with the engine's rights, uncaged, with the engine's environment,
when the engine looks up a pull request. So the engine resolves each tool it starts itself (`gh`, `tmux`,
`caffeinate`, `ps`, `security`, `codex`, `claude`) from a `PATH` (the system's own folders for the system's tools) stripped of relative and empty entries and of every
folder of the repository it works in (its worktrees and Git folders, read from the files on disk), to an absolute
real path, once per process and repository. When nothing is left the engine refuses, naming the cause. Git and ssh
have their own pinning in `git_program`, which this module follows.

`tmux` takes the host's choice first too (`SAKURA_TMUX`, set by the app to the copy it carries), then `PATH`.

The agent CLIs (`claude`, `codex`) do not depend on the caller's `PATH` (an app started from Finder has a minimal one): the
host's choice comes first (`<engine home>/programs.toml`, or `CLAUDE_CODE_EXECUTABLE` for claude), then the same `PATH`
search, then the usual install places of the person's own account. A copy found in an install place, or named by the host,
must be a regular executable the person (or root) owns that no one else can write, outside every repository folder; which
of the three found it is recorded (`resolution`, and the run's ledger line).

One trust check, `untrusted`, decides whether a file may run outside the cage, whichever way it was found (`PATH`, an install
place, the host's file or variable, the master's adapter and the node that runs it): the canonical target, owned by the
person or root, with no group- or world-writable file or ancestor, and under no folder an agent can write (every task
workspace and tmp folder, the engine home, every registered repository, the repository the engine works in).

Each child also gets an environment made for its purpose: an explicit allow-list, with `PATH` fixed to the tool's own
folder and the system folders. Nothing else of the engine's environment reaches it.
"""

from __future__ import annotations

import grp
import hashlib
import os
import pwd
import re
import stat
import subprocess
import sys
import tempfile
import threading
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home, git_program

IDENTITY_VARIABLES = ("HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR")
PROXY_VARIABLES = tuple(name for base in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY")
                        for name in (base, base.lower()))
# What each kind of child may inherit from the engine; tokens only where the tool needs one to do its job.
PURPOSE_VARIABLES = {
    "gh": (*IDENTITY_VARIABLES, "TERM", "GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN",
           "GH_HOST", "GH_CONFIG_DIR", "XDG_CONFIG_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", *PROXY_VARIABLES),
    "tmux": (*IDENTITY_VARIABLES, "TERM", "TMUX", "TMUX_PANE", "TMUX_TMPDIR", engine_home.ENV_VARIABLE),
    "system": IDENTITY_VARIABLES,
}
FIXED_VARIABLES = {"gh": {"GH_PROMPT_DISABLED": "1", "NO_COLOR": "1"}}
# Tools of the system itself: looked up in its own folders only, never on the person's PATH.
SYSTEM_TOOLS = frozenset({"ps", "caffeinate", "security", "sh", "lsof"})
# The agent CLIs and where a copy is looked for besides `PATH`: the person's own install places, in this order.
AGENT_TOOLS = ("claude", "codex")
# What an agent adapter runs on (the master's ACP adapter is a Node script): found the same way.
RUNTIME_TOOLS = ("node",)
FALLBACK_TOOLS = (*AGENT_TOOLS, *RUNTIME_TOOLS, "goose", "claude-agent-acp", "codex-acp")
INSTALL_FOLDERS = ("~/.local/bin", "/opt/homebrew/bin", "/usr/local/bin")
HOST_FILE = "programs.toml"
# `SAKURA_TMUX` is set by the Sakura app to the tmux it carries, so lanes run on a Mac without Homebrew.
HOST_VARIABLES = {"claude": "CLAUDE_CODE_EXECUTABLE", "tmux": "SAKURA_TMUX"}
SOURCE_HOST, SOURCE_PATH, SOURCE_INSTALL = "host configuration", "PATH", "install place"
OTHERS_CAN_WRITE = stat.S_IWGRP | stat.S_IWOTH
# The system's own temp folders are agent-writable roots too; the test suite switches this off because its fake tools live in
# the test's own tmp folder (and tests of the rule switch it back on).
FORBID_SYSTEM_TMP = True
_resolved: dict[tuple[str, str], tuple[str, str]] = {}
_lock = threading.Lock()


class ProgramError(RuntimeError):
    """A program cannot be pinned: it is not on PATH, or every copy lies where a repository can replace it."""


class ProgramMissing(ProgramError):
    """No copy of the program is on PATH at all."""


@dataclass(frozen=True)
class QualifiedProgram:
    """An executable matched to operator install metadata and a specific file identity."""

    path: str
    version: str
    identity: tuple[int, ...]

    def recheck(self, controlled: list[str]) -> None:
        """Refuse a replaced or newly untrusted executable before spawning."""
        if _identity(self.path, controlled) != self.identity:
            raise ProgramError("program_replaced")


def qualify(selected: str, record: object, controlled: list[str]) -> QualifiedProgram:
    """Match the executable to its declared installation without running it."""
    if not isinstance(record, dict) or record.keys() != {"path", "version", "sha256"}:
        raise ProgramError("qualification_required")
    if (record["path"] != selected or not isinstance(record["version"], str)
            or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]+)?", record["version"])
            or not isinstance(record["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", record["sha256"])):
        raise ProgramError("incompatible_install")
    identity = _identity(selected, controlled)
    if _digest(selected, identity) != record["sha256"] or _identity(selected, controlled) != identity:
        raise ProgramError("incompatible_install")
    return QualifiedProgram(selected, record["version"], identity)


def _identity(candidate: str, controlled: list[str]) -> tuple[int, ...]:
    if (not os.path.isabs(candidate) or os.path.realpath(candidate) != candidate
            or not _is_executable(candidate) or untrusted(candidate, controlled)):
        raise ProgramError("program_untrusted")
    try:
        return _fingerprint(os.stat(candidate, follow_symlinks=False))
    except OSError:
        raise ProgramError("program_untrusted") from None


def _fingerprint(info: os.stat_result) -> tuple[int, ...]:
    return info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns


def _digest(candidate: str, identity: tuple[int, ...]) -> str:
    try:
        descriptor = os.open(candidate, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            if _fingerprint(os.fstat(source.fileno())) != identity:
                raise ProgramError("program_replaced")
            return hashlib.file_digest(source, "sha256").hexdigest()
    except OSError:
        raise ProgramError("program_untrusted") from None


def acp_install(harness: str) -> dict:
    """Read operator ACP install metadata from a trusted engine configuration file."""
    try:
        path = engine_home.root() / HOST_FILE
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or _writable_by_others(str(path), info):
            raise ProgramError("program_untrusted")
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            if _fingerprint(os.fstat(source.fileno())) != _fingerprint(info):
                raise ProgramError("program_replaced")
            entries = tomllib.load(source).get("acp", {})
        record = entries.get(harness) if isinstance(entries, dict) else None
    except FileNotFoundError:
        return {}
    except (OSError, ValueError, engine_home.EngineHomeError):
        raise ProgramError("qualification_required") from None
    return record if isinstance(record, dict) else {}


def path(name: str, location: Path | None = None) -> str:
    """The absolute real path of `name`, as `resolution` finds it."""
    return resolution(name, location)[0]


def resolution(name: str, location: Path | None = None) -> tuple[str, str]:
    """The absolute real path of `name` and what found it (`SOURCE_HOST`, `SOURCE_PATH` or `SOURCE_INSTALL`).

    A tool is the first copy on `PATH` that is not in a relative or empty entry and not inside the repository holding
    `location` (the working directory by default); an agent CLI is also looked for by the host's choice first and in the
    install places last. Resolved once per process and repository.

    Raises ProgramError when no copy is left, and says whether none exists or only unsafe ones.
    """
    folders = _controlled(location)
    key = (name, "\0".join(folders))
    with _lock:
        if key not in _resolved:
            _resolved[key] = _find(name, folders)
        return _resolved[key]


def _find(name: str, folders: list[str]) -> tuple[str, str]:
    if (name in FALLBACK_TOOLS or name in HOST_VARIABLES) and (chosen := _host_choice(name, folders)):
        return chosen, SOURCE_HOST
    search = os.pathsep.join(git_program.SAFE_PATH) if name in SYSTEM_TOOLS else os.environ.get("PATH", "")
    # A system tool comes from the system's own folders: listing every agent-writable root (which reads the flow
    # configuration of every repository) would add nothing there, so only the roots known without it apply.
    roots = fixed_roots() if name in SYSTEM_TOOLS else None
    try:
        return _resolve(name, search, folders, roots), SOURCE_PATH
    except ProgramError as error:
        if name not in FALLBACK_TOOLS:
            raise
        if installed := _installed(name, folders):
            return installed, SOURCE_INSTALL
        raise type(error)(f"{error}; and no usable {name} in {', '.join(install_folders())}") from None


def install_folders() -> list[str]:
    """The install places of the person's own account, as absolute paths."""
    home = pwd.getpwuid(os.getuid()).pw_dir
    return [os.path.join(home, folder[2:]) if folder.startswith("~/") else folder for folder in INSTALL_FOLDERS]


def _installed(name: str, controlled: list[str]) -> str:
    for folder in install_folders():
        if real := _own_executable(os.path.join(folder, name), controlled):
            return real
    return ""


def _host_choice(name: str, controlled: list[str]) -> str:
    """The copy the host named for `name` (environment variable, else `<engine home>/programs.toml`), or an empty string
    when it named none. A named copy that is not usable is an error, never a silent fall back to another."""
    variable = HOST_VARIABLES.get(name)
    named, where = (os.environ.get(variable, ""), variable) if variable else ("", "")
    if not named:
        named, where = _host_file_entry(name), f"{HOST_FILE}"
    if not named:
        return ""
    if not os.path.isabs(named) or not (real := _own_executable(named, controlled)):
        raise ProgramError(f"{where} names {named!r} for {name}, which is not an absolute path to an executable of yours "
                           "that no one else can write, outside every repository")
    return real


def host_entry(key: str) -> object:
    """What `<engine home>/programs.toml` says for `key` (a value of any type), or None when the file or the key is
    absent or the file cannot be read."""
    try:
        return tomllib.loads((engine_home.root() / HOST_FILE).read_text()).get(key)
    except (OSError, ValueError, engine_home.EngineHomeError):
        return None


def _host_file_entry(name: str) -> str:
    entry = host_entry(name)
    return entry if isinstance(entry, str) else ""


def fixed_roots() -> list[str]:
    """The agent-writable folders known without running anything: the system tmp folders (unless `FORBID_SYSTEM_TMP` is off)
    and the engine home. Git's own trust check uses these (`forbidden_roots` needs Git to read repository configuration)."""
    from sakura.flow import cage

    roots = [] if not FORBID_SYSTEM_TMP else [*cage.SYSTEM_TMP_ROOTS, "/private/var/folders", tempfile.gettempdir()]
    try:
        roots.append(str(engine_home.root()))
    except (OSError, engine_home.EngineHomeError):
        pass  # inside a cage the engine home may not be readable: the cage is the boundary there
    return roots


def agent_writable_roots() -> list[str]:
    """Every folder an agent (a role, a workspace, a survivor) can write: the cages' writable roots (the engine home and its
    task folders, the workspaces folders, the system tmp folders unless `FORBID_SYSTEM_TMP` is off). Raises OSError,
    EngineHomeError, RegistryError or anything the configuration raises when that cannot be told: the caller refuses then."""
    from sakura.flow import cage

    roots = [str(path) for path in cage.writable_roots()]
    if not FORBID_SYSTEM_TMP:
        system = {os.path.realpath(root) for root in (*cage.SYSTEM_TMP_ROOTS, "/private/var/folders", tempfile.gettempdir())}
        roots = [root for root in roots if os.path.realpath(root) not in system]
    return roots


def forbidden_roots() -> list[str]:
    """Every place the engine runs nothing from: `agent_writable_roots` and every registered repository. Nothing the engine
    runs outside a cage may lie under one. Raises what `agent_writable_roots` raises: the caller refuses then (`untrusted`)."""
    from sakura.flow import registry

    return agent_writable_roots() + [os.path.realpath(repo) for repo in registry.repos()]


def untrusted(candidate: str, controlled: list[str] | None = None, roots: list[str] | None = None) -> str:
    """Why `candidate` may not run outside the cage, or an empty string when it may.

    The canonical target (links resolved) must be a regular file owned by this user or root; neither it nor any folder above
    it may be writable by group or others, and every folder above it must belong to this user or root (see
    `_writable_by_others`); and it may lie under none of `controlled` (the repository the engine works in) nor `roots`
    (`forbidden_roots`, every place an agent can write, by default). Folders are compared by device and inode, never by
    spelling: the volume ignores case. When the roots cannot be listed the answer is a refusal."""
    real = os.path.realpath(candidate)
    try:
        info = os.stat(real)
    except OSError:
        return f"{real} does not exist"
    if not stat.S_ISREG(info.st_mode):
        return f"{real} is not a regular file"
    if reason := _writable_by_others(real, info):
        return reason
    try:
        folders = [*(controlled or []), *(forbidden_roots() if roots is None else roots)]
    except Exception as error:  # noqa: BLE001  (any failure to list the roots is a refusal, never a shorter list)
        return f"{real} is refused because the places an agent can write cannot be listed: {error}"
    if folder := git_program.holding_folder(real, folders):
        return f"{real} lies under {folder}, where a repository or an agent can write"
    return ""


def _writable_by_others(real: str, info: os.stat_result) -> str:
    """The first problem in the ownership and mode of `real` and of every folder above it, or an empty string.

    Each of them must belong to this user or to root: the owner of a folder can replace its entries whatever its mode says.
    Others may not write any of them, except a sticky-bit folder (such as `/tmp`) whose entry below it belongs to this user,
    since there only the entry's owner can replace it. Group write is allowed when nobody but this user and root is in that
    group (`_group_is_private`): the usual Homebrew layout is writable by `admin`, which only the person belongs to."""
    me = os.getuid()
    walked = [(real, info)]
    parent = os.path.dirname(real)
    while True:
        try:
            walked.append((parent, os.stat(parent)))
        except OSError:
            return f"{parent} cannot be read"
        if parent == os.path.dirname(parent):
            break
        parent = os.path.dirname(parent)
    for index, (path, found) in enumerate(walked):
        if found.st_uid not in (me, 0):
            return f"{path} belongs to another account"
        if found.st_mode & stat.S_IWGRP and not found.st_mode & stat.S_IWOTH and _group_is_private(found.st_gid):
            continue
        if not found.st_mode & OTHERS_CAN_WRITE:
            continue
        below = walked[index - 1][1] if index else None
        if not (below is not None and found.st_mode & stat.S_ISVTX and below.st_uid == me):
            return f"{path} can be written by its group or by others"
    return ""


# The only group whose write permission is trusted: macOS `admin`, where Homebrew installs. Every other group, the implicit
# ones (everyone, staff, localaccounts) included, is refused.
ADMIN_GROUP_ID, ADMIN_GROUP_NAME = 80, "admin"
DIRECTORY_TOOL = "/usr/bin/dscl"
DIRECTORY_TIMEOUT_SECONDS = 10


def _group_is_private(gid: int) -> bool:
    """Whether group `gid` is `admin` (80) holding nobody but this user and root: the directory service shows no nested group,
    every listed member name resolves to an account, and no account but those two is a member or has it as primary group.
    An unknown group, an unresolved name or any failed lookup is not private."""
    try:
        name = grp.getgrgid(gid).gr_name
    except (KeyError, OSError):
        return False
    if gid != ADMIN_GROUP_ID or name != ADMIN_GROUP_NAME or _has_nested_groups(name):
        return False
    return _group_members(gid) <= {pwd.getpwuid(os.getuid()).pw_name, "root"}


def _group_members(gid: int) -> set[str]:
    """The accounts in group `gid`: those the group lists and those whose primary group it is. A group that cannot be read, or
    a listed name that is no account, adds a member nobody can name (`?`), so the group is never private."""
    try:
        listed = set(grp.getgrgid(gid).gr_mem)
        members = {*listed, *(account.pw_name for account in pwd.getpwall() if account.pw_gid == gid)}
        for name in listed:
            pwd.getpwnam(name)
    except (KeyError, OSError):
        return {"?"}
    return members


def _has_nested_groups(name: str) -> bool:
    """Whether the directory service says group `name` contains other groups (macOS `NestedGroups`); a failed lookup counts as
    yes. Other systems have no nested groups."""
    if sys.platform != "darwin":
        return False
    try:
        done = subprocess.run([DIRECTORY_TOOL, ".", "-read", f"/Groups/{name}", "NestedGroups"], capture_output=True,
                              text=True, check=False, timeout=DIRECTORY_TIMEOUT_SECONDS, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        return True
    return "No such key" not in done.stdout + done.stderr  # anything else (a value, an error) refuses


def trusted(candidate: str, controlled: list[str] | None = None) -> str:
    """The canonical path of `candidate` when it passes `untrusted`, else an empty string."""
    return "" if untrusted(candidate, controlled) else os.path.realpath(candidate)


def _own_executable(candidate: str, controlled: list[str]) -> str:
    """The canonical path of `candidate` when it is an executable that passes the trust check, else an empty string."""
    return trusted(candidate, controlled) if _is_executable(candidate) else ""


def environment(purpose: str, program: str, base: Mapping[str, str] | None = None) -> dict[str, str]:
    """The environment of a child started for `purpose` (a key of `PURPOSE_VARIABLES`) that runs `program`: the
    allow-listed variables of `base` (the process environment by default) and a fixed `PATH`."""
    source = os.environ if base is None else base
    env = {key: source[key] for key in PURPOSE_VARIABLES[purpose] if key in source}
    env.update(FIXED_VARIABLES.get(purpose, {}))
    folders = [os.path.dirname(program), *git_program.SAFE_PATH]
    env["PATH"] = os.pathsep.join(dict.fromkeys(folders))
    return env


def controlled_folders(location: Path | None = None) -> list[str]:
    """The folders of the repository the engine works in (`location`, the working directory by default). Raises ProgramError
    when they cannot be told."""
    return _controlled(location)


def _controlled(location: Path | None) -> list[str]:
    """The folders of the repository holding `location` (the working directory by default: the engine runs in one)."""
    try:
        return git_program.controlled_folders(location or Path.cwd())
    except git_program.GitProgramError as error:
        raise ProgramError(str(error)) from error


def _resolve(name: str, search_path: str, controlled: list[str], roots: list[str] | None = None) -> str:
    unsafe = []
    for entry in search_path.split(os.pathsep):
        candidate = os.path.join(entry or ".", name)
        if not _is_executable(candidate):
            continue
        if not entry or not os.path.isabs(entry):
            unsafe.append(f"{candidate} ({entry or '(empty)'})")
        elif reason := untrusted(candidate, controlled, roots):
            unsafe.append(f"{candidate} ({reason})")
        else:
            return os.path.realpath(candidate)
    if unsafe:
        raise ProgramError(f"no {name} on PATH may be run outside the cage: {'; '.join(unsafe)}. A program has to be owned by "
                           f"you, writable by no one else (nor any folder above it) and outside every place an agent can "
                           f"write; put a host {name} on PATH ahead of them, or remove those entries")
    raise ProgramMissing(f"no {name} on PATH")


def _is_executable(candidate: str) -> bool:
    try:
        info = os.stat(candidate)
    except OSError:
        return False
    return stat.S_ISREG(info.st_mode) and os.access(candidate, os.X_OK)
