"""Small helpers shared by the workspace, stack and contract code: git calls and log tails.

Every Git call the engine makes in a workspace goes through `run` (and so `git`). A workspace and its `.git` are
writable by the agents that work in it, so the engine never lets Git find its own folder or read its own config
there. Once a repository is registered with `trust_repository` (a task store does it), a call:

* uses the Git folder the engine resolves from that repository (`GIT_DIR`, `GIT_COMMON_DIR`, `GIT_WORK_TREE`) and
  refuses a workspace whose `.git` does not point at its own entry under the trusted repository's `worktrees/`
  (or, for the main checkout, is not the trusted Git folder);
* refuses to run once the repository's local Git config differs from the digest pinned when the task started
  (`pin_config`, see `flow/git_config.py`).

Every call, trusted repository or not, starts from an allow-listed environment, ignores the system Git config and,
except the network subcommands (the person's credential helper lives there), the global one: the person's commit
identity is passed as environment variables, so no driver of the global config runs on the workspace's content.
Hooks, the filesystem monitor and signing programs are switched off for every call. Every call runs the one Git
the engine pinned (`sakura.git_program`), refused when it lies in a folder of the repository it works on, with `PATH`
fixed to that Git's folder and the system folders and `GIT_SSH_COMMAND` naming the pinned ssh.
"""

from __future__ import annotations

import os
import stat
import subprocess
from pathlib import Path

from sakura import beneath, git_program
from sakura.flow import git_config

GIT_TIMEOUT_SECONDS = 300
LOG_TAIL_LINES = 20
HARDENED_CONFIG = git_program.HARDENED_CONFIG
# A URL Git would hand to a program: never recorded or pushed to.
COMMAND_TRANSPORT_PREFIX = "ext::"
# GIT_INDEX_FILE: Git sets it for a hook (the temporary index of `commit -a` or `commit <paths>`), and the kit's own
# hook checks must read that index; the engine process itself never has one.
ENVIRONMENT_VARIABLES = ("PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "TMPDIR", "USER", "LOGNAME",
                         "SSH_AUTH_SOCK", "GIT_INDEX_FILE")
# They talk to a remote with the person's credential helper, which lives in the global config.
NETWORK_SUBCOMMANDS = ("ls-remote", "fetch", "push", "pull", "clone")
IDENTITY_SUBCOMMANDS = ("commit", "merge", "tag", "cherry-pick", "revert", "am", "rebase")
IDENTITY_VARIABLES = (("user.name", ("GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME")),
                      ("user.email", ("GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL")))
_trusted_repositories: list[Path] = []
# The engine's own bare repositories (`flow/sealed.py`), by real path: their checkouts are verified like a workspace.
_trusted_stores: list[str] = []
# The digest of each trusted repository's local Git config when its task started, by real common folder.
_pinned_configs: dict[str, str] = {}


class GitError(RuntimeError):
    """Raised when a git command fails or times out; the message says which and why."""


class GitTimeout(GitError):
    """Raised when a git command exceeded GIT_TIMEOUT_SECONDS."""


def trust_repository(repo: Path) -> None:
    """Tell the helper which repository the engine works on: its workspaces' Git folders are then verified.

    The pinned Git is resolved here, at the engine's start on the repository; raises GitError when it lies in one
    of the repository's folders.
    """
    program_for(repo)
    if repo not in _trusted_repositories:
        _trusted_repositories.append(repo)


def trust_store(store: Path) -> None:
    """Accept `store`, a bare repository the engine made and only the engine writes (`flow/sealed.py`): a call in it
    names it as the Git folder, and a checkout made from it is verified against it like a workspace."""
    real = os.path.realpath(store)
    if real not in _trusted_stores:
        _trusted_stores.append(real)


def init_store(store: Path, object_format: str) -> None:
    """Create the engine's bare repository `store` (an empty folder) with no template and trust it as a store;
    raises GitError on failure."""
    init_bare(store, object_format)
    trust_store(store)


def init_bare(folder: Path, object_format: str) -> None:
    """Create a bare repository in the empty folder `folder` with no template, no system or global config and
    `object_format`; raises GitError on failure."""
    isolated = {**_allowed_environment(), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}
    try:
        argv, env = git_program.init_bare_command(folder, object_format, isolated)
    except git_program.GitProgramError as error:
        raise GitError(str(error)) from error
    done = subprocess.run(argv, capture_output=True, text=True, check=False, timeout=GIT_TIMEOUT_SECONDS, env=env,
                          stdin=subprocess.DEVNULL)
    if done.returncode:
        raise GitError(f"git init --bare {folder} failed: {(done.stderr or done.stdout).strip()}")


def pin_config(repo: Path, digest: str) -> None:
    """Refuse every later Git call on `repo` once its local Git config no longer has `digest` (the task's start)."""
    _pinned_configs[_common(repo)] = digest


def is_command_transport(url: str) -> bool:
    """Whether `url` names the ext transport, which runs a program instead of reaching a remote."""
    return url.strip().lower().startswith(COMMAND_TRANSPORT_PREFIX)


def config_environment(settings: list[tuple[str, str]]) -> dict[str, str]:
    """`settings` (key, value pairs) as `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>` and `GIT_CONFIG_VALUE_<n>`: config
    a Git call takes from its environment, never written to a file; nothing for no settings."""
    if not settings:
        return {}
    env = {"GIT_CONFIG_COUNT": str(len(settings))}
    for index, (key, value) in enumerate(settings):
        env.update({f"GIT_CONFIG_KEY_{index}": key, f"GIT_CONFIG_VALUE_{index}": value})
    return env


def git(path: Path, *args: str) -> str:
    """Run `git -C path args` and return stdout; raises GitError on failure or timeout."""
    done = run(path, *args)
    if done.returncode:
        raise GitError(f"git {' '.join(args)} failed: {(done.stderr or done.stdout).strip()}")
    return done.stdout


def command(path: Path, *args: str, hardened: bool = True, person_config: bool = False,
            local_transfer: bool = False) -> tuple[list[str], dict[str, str]]:
    """The argv and environment of `git args` in `path`, hardened (see the module text), for a caller that supervises
    the process itself. Raises GitError when the workspace's Git folder is not the trusted one, its local config
    changed, `info/grafts` exists or the repository's common folder cannot be resolved.

    `hardened=False` leaves out the `-c` pairs and `person_config=True` lets Git read the system and global config:
    both only for a call that inspects the effective config (the commit guard) and runs no repository content.
    `local_transfer=True`: a fetch from a folder of this machine, which reads no global config.
    """
    program_for(path)
    env = _environment(path, args, person_config, local_transfer)
    try:
        argv, full_env = git_program.command(path, args, env, hardened)
        common = git_program.common_dir_of(path)
    except git_program.GitProgramError as error:
        raise GitError(str(error)) from error
    _require_pinned_config(str(common))
    return argv, full_env


def verified_common_dir(path: Path) -> Path:
    """The real common Git folder of the workspace at `path`, verified as the trusted repository's own (its `.git`
    names its own entry of that repository, see `_resolved_folders`). Raises GitError for anything else: no trusted
    repository, a `.git` that names another repository or one of the engine's stores."""
    if not _trusted_repositories:
        raise GitError(f"no repository is trusted, so the Git folder of {path} cannot be verified")
    folders = _resolved_folders(path)
    common = folders.get("GIT_COMMON_DIR", folders["GIT_DIR"])
    if common not in _trusted_commons():
        raise GitError(f"the Git folder of {path} is not the task repository's own")
    return Path(common)


def _trusted_commons() -> set[str]:
    """The real common folders of the trusted repositories that still exist (a task's repository goes at cleanup)."""
    return {_common(repo) for repo in _trusted_repositories if os.path.lexists(repo)}


def _common(repo: Path) -> str:
    """The real path of `repo`'s common Git folder, read from the files on disk."""
    try:
        return os.path.realpath(git_program.common_dir_of(repo))
    except git_program.GitProgramError as error:
        raise GitError(str(error)) from error


def program_for(path: Path) -> str:
    """The pinned Git for a call in `path`; raises GitError when it lies in a folder of that repository."""
    try:
        return git_program.for_repository(path)
    except git_program.GitProgramError as error:
        raise GitError(str(error)) from error


def program_environment(path: Path, base: dict[str, str] | None = None) -> dict[str, str]:
    """`base` (the process environment by default) for a Git call in `path`: the fixed PATH and the pinned ssh.

    For the few calls that read the person's own Git config (identity, remote URL); raises GitError as `program_for`.
    """
    if base is None:
        from sakura.flow import process  # process reaches gitutil through config and the cage: import it late

        base = process.child_environment(os.environ)
    try:
        return git_program.environment(path, base)
    except git_program.GitProgramError as error:
        raise GitError(str(error)) from error


def run(path: Path, *args: str, text: bool = True, hardened: bool = True, person_config: bool = False,
        local_transfer: bool = False) -> subprocess.CompletedProcess:
    """Run Git in `path` the hardened way; returns the result whatever its exit code (`text=False`: bytes).

    Raises GitError when `command` does, GitTimeout on a timeout.
    """
    argv, env = command(path, *args, hardened=hardened, person_config=person_config, local_transfer=local_transfer)
    try:
        return subprocess.run(argv, capture_output=True, text=text, check=False, timeout=GIT_TIMEOUT_SECONDS, env=env,
                              stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired as error:
        raise GitTimeout(f"git {' '.join(args)} timed out after {GIT_TIMEOUT_SECONDS} seconds") from error


def _allowed_environment() -> dict[str, str]:
    env = {key: os.environ[key] for key in ENVIRONMENT_VARIABLES if key in os.environ}
    env.update(GIT_TERMINAL_PROMPT="0", GIT_ATTR_NOSYSTEM="1")
    return env


def _environment(path: Path, args: tuple[str, ...], person_config: bool = False,
                 local_transfer: bool = False) -> dict[str, str]:
    """The allow-listed environment of one call, with the engine-resolved Git folder once a repository is trusted.

    `local_transfer`: a fetch between two folders of this machine needs no credential helper, so it reads no global
    config either (no `fetch.fsck.*` or URL rewrite of the person's can loosen or redirect it)."""
    env = _allowed_environment()
    if not person_config:
        env["GIT_CONFIG_NOSYSTEM"] = "1"
    if _trusted_repositories:
        env.update(_resolved_folders(path))
    subcommand = _subcommand(args)
    if (subcommand not in NETWORK_SUBCOMMANDS or local_transfer) and not person_config:
        env["GIT_CONFIG_GLOBAL"] = "/dev/null"
    if subcommand in IDENTITY_SUBCOMMANDS:
        env.update(_identity(_trusted_repositories[0] if _trusted_repositories else path))
    return env


def _require_pinned_config(git_folder: str) -> None:
    """Raise GitError when the local Git config of the repository that owns `git_folder` changed since its pin."""
    if not git_folder:
        return
    common = os.path.realpath(git_folder)
    pinned = _pinned_configs.get(common)
    if pinned is None:
        return
    try:
        current = git_config.digest(Path(common))
    except git_config.GitConfigError as error:
        raise GitError(f"cannot read the repository's local Git config: {error}") from error
    if current != pinned:
        raise GitError("the repository's local Git config changed since the task started; the engine runs no Git "
                       f"command on it until it is restored (files: {', '.join(map(str, git_config.files(Path(common))))})")


def _subcommand(args: tuple[str, ...]) -> str:
    """The Git subcommand in `args`, after any leading `-c name=value` pairs."""
    rest = list(args)
    while rest and rest[0] == "-c":
        rest = rest[2:]
    return rest[0] if rest else ""


def _resolved_folders(path: Path) -> dict[str, str]:
    """`GIT_DIR`, `GIT_COMMON_DIR` and `GIT_WORK_TREE` of the workspace at `path`, verified against a trusted repository.

    A linked worktree's `.git` is a regular file naming `<trusted Git folder>/worktrees/<name>`, and that entry's
    `gitdir` file names this very `.git` file back. The main checkout's `.git` is the trusted Git folder itself.
    Anything else (a symlink, another repository, a folder an agent made, a foreign entry) raises GitError.
    An engine store (`trust_store`) and a trusted bare repository (a task's own, `flow/task_repository.py`) are their
    own Git folder, and a checkout made from either points at an entry of it.
    """
    commons = _trusted_commons() | set(_trusted_stores)
    roots = {os.path.realpath(repo) for repo in _trusted_repositories}
    root = os.path.realpath(path)
    if root in _trusted_stores or (root in roots and root in commons):
        return {"GIT_DIR": root}
    marker = Path(root) / ".git"
    try:
        info = marker.lstat()
    except OSError:
        raise GitError(f"{path} has no .git of its own; the engine does not let Git look for one") from None
    if not _named_exactly(Path(root), marker.name):
        raise GitError(f"the Git marker of {path} is not named exactly .git; the engine accepts only the one Git made")
    work_tree = {"GIT_WORK_TREE": root}
    if stat.S_ISDIR(info.st_mode):
        folder = os.path.realpath(marker)
        # The only checkout whose Git folder sits inside it is a registered repository's own main checkout.
        if root not in roots:
            raise GitError(f"the Git folder of {path} is inside the workspace, where an agent can write it")
        if folder not in commons:
            raise GitError(f"the .git folder of {path} is not a registered repository's Git folder")
        return {"GIT_DIR": folder, **work_tree}
    if not stat.S_ISREG(info.st_mode):
        raise GitError(f"the .git of {path} is neither a file nor a folder the engine accepts")
    gitdir = _linked_gitdir(marker)
    common = str(Path(gitdir).parent.parent)
    if _inside(gitdir, root) or _inside(common, root):
        raise GitError(f"the Git folder named by the .git file of {path} is inside the workspace, where an agent can write it")
    if Path(gitdir).parent.name != "worktrees" or common not in commons or not _points_back(Path(gitdir), marker):
        raise GitError(f"the .git file of {path} does not point at its own entry of a registered repository")
    return {"GIT_DIR": gitdir, "GIT_COMMON_DIR": common, **work_tree}


def _inside(folder: str, root: str) -> bool:
    return folder == root or folder.startswith(root + os.sep)


def _linked_gitdir(marker: Path) -> str:
    """The entry a linked worktree's `.git` file names, refused unless it is an absolute path with no link on the way
    (Git writes the real path; a link would let a folder an agent controls stand in for part of it)."""
    try:
        text = beneath.read_bytes(marker.parent, marker.name).decode(errors="replace").strip()
    except OSError as error:
        raise GitError(f"{marker} cannot be read as a regular file ({error.strerror or error})") from None
    target = text.removeprefix("gitdir:").strip() if text.startswith("gitdir:") else ""
    if not target:
        raise GitError(f"{marker} is not a gitdir file")
    real = os.path.realpath(marker.parent / target)
    if not os.path.isabs(target) or os.path.normpath(target) != real:
        raise GitError(f"{marker} names its Git folder through a link or a relative path; the engine accepts only "
                       "the real path Git writes")
    return real


def _named_exactly(folder: Path, name: str) -> bool:
    """Whether `folder` holds an entry named exactly `name`: on a case-insensitive volume a lookup also finds a case
    variant, which no Git and no engine step made."""
    try:
        return name in os.listdir(folder)
    except OSError:
        return False


def _points_back(gitdir: Path, marker: Path) -> bool:
    try:
        return os.path.realpath(beneath.read_bytes(gitdir, "gitdir").decode(errors="replace").strip()) == \
            os.path.realpath(marker)
    except OSError:
        return False


def person_identity(repo: Path) -> dict[str, str]:
    """The person's commit identity, `user.name` and `user.email`, from `repo`'s effective config (local, then
    global); a key the config does not set is left out."""
    found: dict[str, str] = {}
    for key, _variables in IDENTITY_VARIABLES:
        done = run(repo, "config", key, person_config=True)
        if done.returncode == 0 and done.stdout.strip():
            found[key] = done.stdout.strip()
    return found


def _identity(repo: Path) -> dict[str, str]:
    """The host's commit identity from `repo`'s config (local, then global), as environment variables."""
    values = person_identity(repo)
    return {variable: values[key] for key, variables in IDENTITY_VARIABLES if key in values for variable in variables}


def branch_exists(path: Path, branch: str) -> bool:
    """Return whether the local branch exists."""
    try:
        git(path, "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}")
    except GitTimeout:
        raise
    except GitError:
        return False
    return True


def tail(log: Path) -> str:
    """Return the last LOG_TAIL_LINES lines of `log`, or an empty string when it does not exist."""
    if not log.exists():
        return ""
    return "\n".join(log.read_text(errors="replace").splitlines()[-LOG_TAIL_LINES:])
