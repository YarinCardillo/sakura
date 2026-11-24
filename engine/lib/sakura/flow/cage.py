"""The cage: the native sandbox configuration the engine writes for every Claude Code and Codex run.

Agents run on the Mac inside the sandbox of their own CLI. Before each run the engine writes, into the task
folder (never into the workspace), a Claude Code settings file, an MCP config naming the engine's permission
prompt server and the policy that server answers from. Claude Code gets its sandbox on with no unsandboxed
retry, reads of secret folders denied, writes only in the workspace, the Git common folder of the task's own
repository (`flow/task_repository.py`; its hooks, config and project record excepted, and never the person's own Git
folder) and a run tmp folder, a network allow-list, and deny rules for the programs that reach outside
(`osascript`, `open`, `launchctl`, `tailscale`, `docker`, signals, `security`, `git push`). Whatever would ask a
person goes to the engine's prompt tool instead (`flow/cage_server.py`), which answers from this policy and
records every refusal for the person.

A cage policy is `default` (built in) or `<engine home>/cages/<name>.toml`, named by the preset's `cage` knob.
Its lists add to the defaults; nothing in a policy file removes a default denial.
"""

from __future__ import annotations

import contextlib
import contextvars
import hashlib
import json
import os
import pwd
import re
import shutil
import sys
import tempfile
import tomllib
from dataclasses import dataclass, field, replace
from pathlib import Path

from sakura import engine_home, git_program, grants, install, interpreter
from sakura.flow import codex_profile, config, denied_commands, gitutil, loopback, pinned_roots, presets, registry, task_repository

KIT_BIN = Path(__file__).resolve().parents[3] / "bin" / "sakura"
POLICIES_DIR = "cages"
DEFAULT_POLICY = "default"
CAGE_DIR = "cage"
# The task folder's run logs (`TaskStore.runs_dir`), where every caged command's output goes.
RUNS_DIR = "runs"
SERVER_NAME = "sakura"
PERMISSION_TOOL = f"mcp__{SERVER_NAME}__permission"
RUN_ROW_TOOL = f"mcp__{SERVER_NAME}__run_row"
TMP_ROOT = "sakura-cage"
# Where the engine makes the writable folder of each row run: denied to every other cage, so no agent reaches it.
ROWS_ROOT = "sakura-rows"
# The identity of the task's shared tmp folder, in the task's cage folder (no role writes there).
SHARED_TMP_RECORD = "shared-tmp.json"
# A row runs on read-only source: the tools write their bytecode, build output and test caches in its own folder.
BUILD_VARIABLES = {"PYTHONPYCACHEPREFIX": "pycache", "CARGO_TARGET_DIR": "cargo-target", "GOCACHE": "go-build"}
# Claude Code's own temp root (its command output files among them) is the run's tmp folder, not the per-user one
# under /private/tmp that every run shares and the cage denies: pytest stats its stdout there (measured: exit 120).
CLAUDE_TMP_VARIABLE = "CLAUDE_CODE_TMPDIR"
CLAUDE_TMP_NAME = "claude"
# Names the writable folder of a run on read-only source, for setup commands and tests that need one.
BUILD_DIR_VARIABLE = "SAKURA_FLOW_BUILD_DIR"
# pytest would otherwise write `.pytest_cache` into the source it tests.
READ_ONLY_SOURCE_ENV = {"PYTEST_ADDOPTS": "-p no:cacheprovider"}
# Agent account folders (every registered or detected Claude and Codex config folder) are added by `secret_paths`.
SECRET_HOME_PATHS = (".ssh", ".aws", ".gnupg", ".config/gh", ".claude", ".codex", "Library/Keychains")
SECRET_SYSTEM_PATHS = ("/Library/Keychains",)
# Reads outside the home denied like the home itself: other homes and /Users/Shared, mounted volumes, and the shared
# temp folders (another run's or another program's files); the run's own tmp folder and workspace are allowed back.
OUTSIDE_HOME_READ_DENIALS = ("/Users", "/Volumes", "/private/tmp", "/private/var/tmp", "/private/var/folders", "/cores",
                             # System folders a user can read that carry credentials: service configs and data of
                             # Homebrew (database passwords, tokens), the SSH host setup, the system database folder
                             # (Directory Services, system state) and the logs. The Keychains are secrets already.
                             "/opt/homebrew/etc", "/opt/homebrew/var", "/usr/local/etc", "/usr/local/var",
                             "/private/etc/ssh", "/private/var/db", "/private/var/log", "/Library/Logs",
                             "/Library/Preferences/SystemConfiguration")
# Every top-level folder of the volume but the system's own (`/System`, `/usr`, `/bin`, `/sbin`, `/dev`): Claude Code's
# sandbox has no "deny everything" (a denied `/` hides the workspace even when it is allowed back, measured on 2.1.323),
# so its settings deny each of these and allow back the trees below. Nothing new appears at the top without root rights.
TOP_LEVEL_READ_DENIALS = ("/Applications", "/Library", "/Users", "/Volumes", "/cores", "/home", "/opt", "/private")
# What every caged command may read outside the checkout: the system and its developer tools (measured with Python, Node,
# npm, Git, Cargo, Go, curl and a pytest suite; the reason of each entry is in the plan).
# The folders of the engine's own interpreter a caged command may read, and the only places under the home where a
# per-version interpreter (pyenv, uv) is accepted.
INTERPRETER_FOLDERS = ("bin", "include", f"lib/python{sys.version_info.major}.{sys.version_info.minor}")
HOME_INTERPRETER_PARENTS = (".pyenv/versions", ".local/share/uv/python")
HOMEBREW_TOOL_FOLDERS = ("bin", "sbin", "Cellar", "opt", "lib", "include", "share", "Frameworks")
ETC_READ_FILES = ("hosts", "protocols", "services", "shells", "passwd", "group")
SYSTEM_READ_TREES = ("/System", "/usr", "/bin", "/sbin", "/Library/Developer", "/Library/Frameworks", "/Library/Apple",
                     "/Applications/Xcode.app",
                     # `sh` and the developer-tool shims look up the selected shell and Xcode here (measured: `sh` fails
                     # without `/private/var/select`), and OpenSSL's and LibreSSL's tools read their config and bundles.
                     "/private/var/select", "/private/var/db/xcode_select_link", "/private/etc/ssl",
                     # Time zone, name resolution and the account database: the few files of `/private/etc` that tools read.
                     *(f"/private/etc/{name}" for name in ETC_READ_FILES),
                     *(f"/opt/homebrew/{name}" for name in HOMEBREW_TOOL_FOLDERS))
# The root folder itself (its listing, not what is under it): every system binary opens it at start (measured: `true` and
# `echo` abort without it).
SYSTEM_READ_FILES = ("/", "/private/var/run/resolv.conf",
                     # Xcode's license acceptance, which the developer-tool shims (`/usr/bin/git`) check at start.
                     "/Library/Preferences/com.apple.dt.Xcode.plist")
# The links the system's tools look at (the folders `/var`, `/etc` and `/tmp`, the time zone, the resolver file and the
# descriptor names): a link is read by its own name, which the real-path rules of the folders above do not cover.
# The time zone link, by its own name: a settings file lists paths as written and does not follow a link (measured: the
# zone is read as UTC without it).
SYSTEM_LINKS_READ_BY_NAME = ("/private/etc/localtime",)
SYSTEM_LINKS = ("/var", "/etc", "/tmp", "/dev/stdin", "/dev/stdout", "/dev/stderr", "/private/etc/localtime",
                "/private/etc/resolv.conf")
# What the system's own tools read inside those folders: the time zone (without it every tool falls back to UTC) and
# Homebrew's git system config and certificate bundles (git exits 128 without its config). Nothing secret.
SYSTEM_READ_BACK = ("/private/var/db/timezone",
                    *(f"{prefix}/etc/{name}" for prefix in ("/opt/homebrew", "/usr/local")
                      for name in ("gitconfig", "ca-certificates", "openssl@3/cert.pem", "openssl@3/openssl.cnf",
                                   "openssl@3/certs", "openssl@4/cert.pem", "openssl@4/openssl.cnf", "openssl@4/certs")))
# The data volume reaches every firmlinked folder (the home, /Users, /private, /Library) under a second name: each denied
# path is denied under that name too. Seatbelt resolves the alias itself (measured); Claude Code's rules match names.
DATA_VOLUME = "/System/Volumes/Data"
CODEX_PROFILE = codex_profile.REVIEW_PROFILE
# What Codex's own `:minimal` platform defaults read that a reviewer does not need (measured with 0.164.0, read in its
# Seatbelt source): the applications folder, all of `/private/etc`, the system preferences and `/private/var/db`. A plain
# deny entry loses to those defaults (measured: `ls /private/etc/ssh` worked despite one); patterns win: `<path>*` for the
# folder itself and `<path>/**` for everything below, and a narrower read entry beats them (what a reviewer's tools need
# comes back).
CODEX_PLATFORM_DENIALS = ("/Applications", "/private/etc", "/Library/Preferences", "/private/var/db")
CODEX_PLATFORM_DENIALS_WORK = ("/private/etc", "/private/var/db")
CODEX_WORK_KEEPS = {"/Applications": ("Xcode.app",), "/Library/Preferences": ("com.apple.dt.Xcode.plist",)}
# The terminal and serial devices the platform defaults let a profile write (measured: with `:minimal` alone a reviewer's
# command writes to another terminal's `/dev/ttys*`). A deny pattern covers them, a path of one device does not.
CODEX_DENIED_DEVICES = ("/dev/tty*", "/dev/pty*", "/dev/ptmx", "/dev/cu.*", "/dev/console")
# Codex's own binaries: its file helper must be able to start from inside the profile (the rest of `~/.codex` is secret).
CODEX_INSTALL_PATHS = (".codex/packages",)
# What a role reads of the engine's installation: the program, its library and the Git hook scripts the workspace links
# to. Never the installation root: in an ordinary clone it holds `.git/config` (review 17, finding 2).
ENGINE_RUNTIME_FOLDERS = ("bin", "lib", "git-hooks")
# Operator state outside the engine home: hook marks, digests, deferred pages, cockpit and lane config.
OPERATOR_HOME_PATHS = (".local/state/sakura", ".config/sakura")
# The files of a linked worktree's Git folder that say where its config and common folder are.
WORKTREE_GITDIR_FILES = ("config.worktree", "gitdir", "commondir")
# What in the common Git folder makes Git read another repository's objects: the alternates lists, a `commondir` and a
# `.git` file naming another Git folder. No agent writes them (an agent's own Git never needs them).
INDIRECTION_FILES = ("objects/info/alternates", "objects/info/http-alternates", "commondir", ".git")
# The system's shared tmp folders: Claude Code gives a lane's commands its own tmp folder under one of them
# (measured with 2.1.323: TMPDIR=/private/tmp/claude-<uid>, writable inside its sandbox), and the per-user one
# (`tempfile.gettempdir()`) holds the cages' own tmp folders.
SYSTEM_TMP_ROOTS = ("/private/tmp", "/private/var/tmp")
# The object folders an agent may write into but never remove, rename or replace with a link.
PINNED_FOLDERS = ("objects", "objects/info", "objects/pack")
# Claude Code pins every folder above a write-denied path against removal and replacement (measured, 2.1.323): the
# alternates entries pin `objects/info` and `objects`, and this name, which Git never writes, pins `objects/pack`.
PACK_FOLDER_PIN = "objects/pack/.sakura-pin"
SEATBELT = "/usr/bin/sandbox-exec"
# The only devices a caged command may write: the data sinks and sources every tool opens, and the descriptors it
# inherited (`/dev/fd/N`, which `/dev/stdout` and `/dev/stderr` point to). Every other device stays unwritable, the
# terminals (`/dev/ttys*`, `/dev/tty`: injected output and control sequences), pseudo-terminals, disks and serial ports.
WRITABLE_DEVICES = ("/dev/null", "/dev/zero", "/dev/random", "/dev/urandom")
WRITABLE_DEVICE_FOLDERS = ("/dev/fd",)
# Programs a caged command may not start, whatever the path it is copied to (Seatbelt checks the signed binary).
DENIED_EXECUTABLES = ("/usr/bin/osascript", "/usr/bin/open", "/bin/launchctl", "/usr/bin/security")
# Mach services that hold what no file rule covers: the pasteboard, the keychain (securityd), and LaunchServices,
# which starts apps (measured: `pbpaste`, `security find-generic-password` and `open -a` fail without them).
SENSITIVE_MACH_SERVICES = ('(global-name-prefix "com.apple.pasteboard")', '(global-name "com.apple.SecurityServer")',
                           '(global-name-prefix "com.apple.securityd")', '(global-name "com.apple.secd")',
                           '(global-name "com.apple.coreservices.launchservicesd")', '(global-name-prefix "com.apple.lsd.")')
# Another process's arguments and environment: process info is for the run's own tree only. The procargs sysctl
# needs its own rule (measured: without it `KERN_PROCARGS2` still reads another process's environment).
PROCESS_INFO_RULES = ("(deny process-info*)", "(allow process-info* (target self))",
                      "(allow process-info* (target children))", "(allow process-info* (target same-sandbox))",
                      '(deny sysctl-read (sysctl-name-prefix "kern.procargs"))')
ENV_FILES = "**/.env*"
# The programs and commands a role may not start (`flow/denied_commands.py` holds the lists, which a Codex role's hook
# checks by name too): a Bash rule only stops the program by name; the sandbox stays the barrier.
DENIED_PROGRAMS = denied_commands.PROGRAMS
DENIED_COMMANDS = denied_commands.COMMANDS
SUBAGENT_TOOLS = ("Agent", "Task")
# The digest summarizer reads one file and writes one file: every other tool is denied by name, whatever else allows it.
DIGEST_DENIED_TOOLS = ("Bash", "WebFetch", "WebSearch", "Glob", "Grep", "LS", "Edit", "MultiEdit", "NotebookEdit",
                       *SUBAGENT_TOOLS)
WRITE_TOOLS = ("Edit", "Write", "MultiEdit", "NotebookEdit")
GITHUB_READ = ("api.github.com", "raw.githubusercontent.com", "codeload.github.com", "objects.githubusercontent.com")
MODEL_API = ("api.anthropic.com",)
# How a loopback destination is spelled in Claude Code's allow-list, always with its port.
LOOPBACK_HOSTS = ("127.0.0.1", "[::1]", "localhost")
# The package registries a project uses, by the file that shows it.
REGISTRIES = {
    "package.json": ("registry.npmjs.org", "registry.yarnpkg.com"),
    "pyproject.toml": ("pypi.org", "files.pythonhosted.org"),
    "requirements.txt": ("pypi.org", "files.pythonhosted.org"),
    "setup.py": ("pypi.org", "files.pythonhosted.org"),
    "Cargo.toml": ("crates.io", "index.crates.io", "static.crates.io"),
    "go.mod": ("proxy.golang.org", "sum.golang.org"),
    "Gemfile": ("rubygems.org",),
}
CACHE_VARIABLES = {"npm_config_cache": "npm", "PIP_CACHE_DIR": "pip", "UV_CACHE_DIR": "uv", "XDG_CACHE_HOME": "xdg"}
# An agent never signs with the person's key (its folder is denied anyway): Git settings for every caged command.
UNSIGNED_GIT = (("commit.gpgsign", "false"), ("tag.gpgsign", "false"))
POLICY_KEYS = frozenset({"secret_paths", "allowed_domains", "read_paths", "local_network"})
# A key older policies carried: the stack is the person's own command now, no policy reaches it.
RETIRED_STACK_KEY = "stack"
# Reads in the home are denied by default; these toolchain folders are what a lane needs to read there. Of Cargo's
# home only what `cargo build` and `cargo test` read (measured with a crate built offline from the registry cache: the
# index, the downloaded crates and their sources are each needed) and its Git caches; never its credentials or config.
TOOLCHAIN_HOME_PATHS = (".cargo/bin", ".cargo/registry/index", ".cargo/registry/cache", ".cargo/registry/src",
                        ".cargo/git/db", ".cargo/git/checkouts", ".rustup", ".nvm", ".npm", ".pyenv", ".bun", ".deno",
                        ".local/bin", "Library/Python")
# The credential files of the toolchains and package managers, at their documented places in the home: secrets of
# every cage, so no toolchain grant or policy read path ever covers one.
CREDENTIAL_HOME_FILES = (".cargo/credentials", ".cargo/credentials.toml", ".cargo/config", ".cargo/config.toml",
                         ".npmrc", ".yarnrc.yml", ".bunfig.toml", ".config/pnpm/rc", "Library/Preferences/pnpm/rc",
                         ".pypirc", ".pip/pip.conf", ".config/pip/pip.conf", "Library/Application Support/pip/pip.conf",
                         ".config/uv/uv.toml", ".local/share/uv/credentials", ".netrc", ".git-credentials",
                         ".config/git/credentials", ".docker/config.json", ".gem/credentials",
                         ".gradle/gradle.properties", ".m2/settings.xml", ".m2/settings-security.xml")
# Inside the granted trees: the global npm config of each nvm Node, the pip config of each pyenv Python, and the
# credential dotfiles no toolchain keeps as source, wherever they appear in a granted tree.
CREDENTIAL_TOOLCHAIN_PATTERNS = (".nvm/versions/node/**/etc/npmrc", ".pyenv/versions/**/pip.conf")
CREDENTIAL_DOTFILES = (".npmrc", ".pypirc", ".netrc", ".git-credentials")
# The site-wide pip config of the engine's own interpreter, which may lie in a system tree every cage reads.
INTERPRETER_CREDENTIAL_FILES = ("pip.conf",)
GIT_IDENTITY = (("user.name", ("GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME")),
                ("user.email", ("GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL")))


# The canonical spelling of each writable root of the cage being generated, captured when the root was verified
# (`flow/pinned_roots.py`). `_real` answers from it and never resolves such a root again: a surviving child can point a
# link at a root's name for the moment a profile is generated and put the folder back before the final check
# (review 18, finding 1).
_CAPTURED: contextvars.ContextVar[dict[str, str] | None] = contextvars.ContextVar("cage_captured_roots", default=None)


class CageError(ValueError):
    """Raised when a cage policy is unknown or malformed."""


@dataclass(frozen=True)
class Policy:
    """What a cage adds to the defaults: secret paths, hosts, read paths and local network."""

    name: str
    secret_paths: tuple[str, ...] = ()
    allowed_domains: tuple[str, ...] = ()
    local_network: bool = False
    read_paths: tuple[str, ...] = ()


@dataclass(frozen=True)
class RunCage:
    """The files and environment of one caged run."""

    folder: Path
    settings: Path
    mcp_config: Path
    policy: Path
    requests: Path
    tmp: Path
    codex_args: tuple[str, ...]
    seatbelt: Path
    env: dict[str, str] = field(default_factory=dict)

    def wrap(self, argv: list[str]) -> list[str]:
        """`argv` run under the engine's Seatbelt profile for this run (exec roles and contract checks)."""
        return [SEATBELT, "-f", str(self.seatbelt), *argv]


@dataclass(frozen=True)
class RunSite:
    """Where one run works: its workspace, the Git common folder of the task's own repository (never the person's)
    and the engine folders it must not touch."""

    task_root: Path
    run: str
    role: str
    workspace: Path
    common_dir: Path
    engine_roots: tuple[Path, ...]
    # The person's repository the task works on; the master has none.
    repo: Path | None
    # The evaluator's `run_row` tool: the environment `sakura flow run-row` needs (task, stack, base URL, phase).
    row_env: dict[str, str] = field(default_factory=dict)
    # The loopback ports the run may connect to: the registered application port and the host's approved dependent ports.
    loopback_ports: tuple[int, ...] = ()
    # A row run: the workspace and the Git folder are read-only, and this engine-made folder is the only writable one.
    build_dir: Path | None = None
    # The person approved this project's commands to open their own loopback sockets (`checks.local_sockets`).
    local_sockets: bool = False
    # The files an exec role's host-configured command names (its program, its script): readable wherever they are.
    program_paths: tuple[str, ...] = ()
    # A run isolated from the task (the reporter, a code reviewer: review 11): a tmp folder of its own the engine made
    # for it, never the one the task's runs share, and these folders (the task's workspaces, repository, shared tmp
    # and every earlier isolated run's tmp folder) denied like secrets.
    tmp_dir: Path | None = None
    denied_reads: tuple[Path, ...] = ()
    # The knowledge folders by name (`sakura/knowledge.py`), already checked: every run may read them, none may write.
    knowledge: dict[str, str] = field(default_factory=dict)
    # The master (`master/spec.py`) has no task: its settings carry no task hooks (without a task they fail closed on
    # every call), its MCP config starts the master's own server, and the files the engine writes for it are read-only.
    hooks: bool = True
    server_command: tuple[str, ...] = ("cage", "serve")
    server_env: dict[str, str] = field(default_factory=dict)
    protected_writes: tuple[Path, ...] = ()
    # The one file a run may write, and only that one (the digest summarizer's `out.json`): a rule names it, and the run
    # does not auto-accept edits, so any other write is a prompt that the engine's prompt tool denies.
    output_file: Path | None = None
    # The folders the Read, Grep and Glob tools may reach besides the working folder, and nothing else (the master):
    # their Git folders stay denied. None leaves those tools to the sandbox and the prompt server, as for every lane.
    read_roots: tuple[Path, ...] | None = None
    connector_ids: tuple[str, ...] | None = None
    tool_names: tuple[str, ...] = ()
    worker_env: dict[str, str] = field(default_factory=dict)


def load_policy(name: str, root: Path | None = None) -> Policy:
    """Return cage policy `name`: `default`, or `<engine home>/cages/<name>.toml` over it."""
    if not presets.NAME.fullmatch(name or ""):
        raise CageError(f"cage policy name must match {presets.NAME.pattern}; got {name!r}")
    if name == DEFAULT_POLICY:
        return Policy(DEFAULT_POLICY)
    path = (root or engine_home.root()) / POLICIES_DIR / f"{name}.toml"
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError:
        raise CageError(f"unknown cage policy {name}: add it as {path}") from None
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise CageError(f"cannot read {path}: {error}") from error
    return _policy(path, name, data)


def prepare(site: RunSite, policy: Policy, subagents: int, home: Path | None = None) -> RunCage:
    """Write the cage of one run into `<task folder>/cage/<run>/`, replacing any earlier file of that run."""
    folder = site.task_root / CAGE_DIR / site.run
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    home = home or account_home()
    tmp = _verified_roots(site)
    token = _CAPTURED.set(_captured_roots(site, tmp))
    try:
        return _prepared_cage(site, policy, subagents, home, folder, tmp)
    finally:
        _CAPTURED.reset(token)


def _prepared_cage(site: RunSite, policy: Policy, subagents: int, home: Path, folder: Path, tmp: Path) -> RunCage:
    _require_narrow_reads(policy, home, secret_paths(policy, home, site.workspace), site.engine_roots)
    folders = {**CACHE_VARIABLES, **(BUILD_VARIABLES if site.build_dir else {})}
    cage = RunCage(folder, folder / "settings.json", folder / "mcp.json", folder / "policy.json",
                   folder / "requests.jsonl", tmp, (), folder / "sandbox.sb",
                   {"TMPDIR": str(tmp), CLAUDE_TMP_VARIABLE: str(tmp / CLAUDE_TMP_NAME), **_git_settings(site),
                    **git_program.NO_REPLACE,
                    **_git_identity(site.repo),
                    # The person's Git config is in the home, which caged commands cannot read.
                    "GIT_CONFIG_GLOBAL": "/dev/null", "XDG_CONFIG_HOME": str(tmp / "config"),
                    **{variable: str(tmp / name) for variable, name in folders.items()},
                    **({**READ_ONLY_SOURCE_ENV, BUILD_DIR_VARIABLE: str(site.build_dir)} if site.build_dir else {})})
    try:
        for variable in ("XDG_CONFIG_HOME", CLAUDE_TMP_VARIABLE, *folders):
            pinned_roots.make_folder(tmp, Path(cage.env[variable]).name)
    except pinned_roots.RootReplaced as error:
        raise CageError(str(error)) from None
    _private_folder(Path(cage.env[CLAUDE_TMP_VARIABLE]))
    granted, servers = _granted(site)
    mcp = {SERVER_NAME: {"command": str(KIT_BIN), "args": [*site.server_command, str(cage.policy)],
                         **({"env": site.server_env} if site.server_env else {})}, **servers}
    tools = {} if site.role == grants.DIGEST_CLASS else mcp  # the summarizer has no tool server
    cage = replace(cage, codex_args=(*codex_args(site, policy, cage, home), *codex_profile.mcp_pairs(tools)))
    served = server_policy(site, cage, granted)
    settings = claude_settings(site, policy, subagents, cage, home, granted)
    if found := served.get("connectors"):
        operations = sorted({key for entry in found.values() for key in entry["operations"]})
        settings["permissions"]["allow"].extend(f"mcp__{SERVER_NAME}__connector_{key}" for key in operations)
    profile = seatbelt_profile(site, policy, cage, home)
    # Every grant above resolved its root afresh: a root swapped while they were generated is refused before any file
    # is written (review 17, finding 1). The profile's closing denials are the structural answer to a swap after this.
    if _verified_roots(site) != cage.tmp:
        raise CageError(f"{cage.tmp} changed while its cage was generated; nothing is granted on it")
    cage.requests.unlink(missing_ok=True)
    _write_json(cage.settings, settings)
    _write_json(cage.policy, served)
    _write_text(cage.seatbelt, profile)
    _write_json(cage.mcp_config, {"mcpServers": mcp})
    return cage


def _captured_roots(site: RunSite, tmp: Path) -> dict[str, str]:
    """Each writable root (and the rows folder a build folder lies in) by the canonical spelling captured when it was
    pinned; `tmp` and the roots are verified by `_verified_roots` just before."""
    roots = {str(path): pinned_roots.canonical(path) for path in (site.workspace, site.common_dir, tmp)}
    if site.build_dir:
        roots[str(site.build_dir.parent)] = pinned_roots.spelling(site.build_dir.parent)
    return roots


def workspaces_folder() -> Path:
    """The folder that holds every lane's workspaces under the default `workspace.root`."""
    return task_repository.workspaces_folder(config.defaults().values)


def writable_roots() -> tuple[Path, ...]:
    """Every folder a role's cage can let a role write, each through `os.path.realpath`: the system and account temp
    folders (where the run's tmp folder is made), the engine home, and the workspaces folder of the default, the
    personal and every registered repository's `workspace.root`. A file a role could have planted lies in one of
    them, so nothing the engine runs outside a cage (a granted MCP server) may be taken from there."""
    named = [*SYSTEM_TMP_ROOTS, "/private/var/folders", tempfile.gettempdir(), engine_home.root(),
             *(task_repository.workspaces_folder(values) for values in _workspace_settings())]
    return tuple(dict.fromkeys(Path(os.path.realpath(folder)) for folder in named))


def _workspace_settings() -> list[dict]:
    """The effective settings of the defaults, the personal layer and each registered repository; a repository whose
    configuration cannot be read is skipped, as no lane can start in it."""
    found = [config.defaults().values, config.load(None).values]
    for repo in registry.repos():
        try:
            found.append(config.load(repo).values)
        except (config.FlowConfigError, task_repository.TaskRepositoryError, OSError, ValueError):
            continue
    return found


def _granted(site: RunSite) -> tuple[grants.ClassGrant, dict[str, dict]]:
    """The host grant of a role's class and the MCP config entries of its servers; a role that is no class (the
    checks) gets nothing. A server's command may not lie where this run can write."""
    if site.role == grants.SCHEDULED_CLASS:
        return grants.ClassGrant(site.tool_names), {}
    if site.role not in grants.CLASSES:
        return grants.ClassGrant(), {}
    try:
        loaded = grants.load(engine_home.root(), (site.workspace, site.common_dir.parent, *writable_roots()))
    except (grants.GrantsError, config.FlowConfigError, registry.RegistryError,
            task_repository.TaskRepositoryError) as error:
        raise CageError(str(error)) from None
    return loaded.for_class(site.role), loaded.mcp_servers(site.role)


def _verified_roots(site: RunSite) -> Path:
    """The run's writable tmp (or build) folder, once it and the workspace are checked to be the folders the engine
    made, reached through no link (`flow/pinned_roots.py`); raises CageError otherwise, before anything names them."""
    try:
        pinned_roots.verified(site.workspace)
        pinned_roots.verified(site.common_dir)
        if site.build_dir:
            return pinned_roots.verified(site.build_dir)
        if site.tmp_dir is not None:
            return pinned_roots.verified(site.tmp_dir)
        return _shared_tmp_dir(site.task_root)
    except pinned_roots.RootReplaced as error:
        raise CageError(str(error)) from None


def claude_settings(site: RunSite, policy: Policy, subagents: int, cage: RunCage, home: Path,
                    granted: grants.ClassGrant = grants.ClassGrant()) -> dict:
    """The `--settings` file of a Claude Code run: engine hooks, sandbox, permission rules.

    The role's granted engine tools and host MCP servers are allowed by name; nothing else of MCP is."""
    secrets = _site_secrets(site, policy, home)
    hosts = [] if site.role in (grants.DIGEST_CLASS, grants.SCHEDULED_CLASS) else allowed_domains(policy, site.workspace)
    return {
        "disableAllHooks": False,
        "hooks": install.claude_hook_settings() if site.hooks else {},
        "permissions": {
            "deny": _deny_rules(site, secrets, subagents),
            # Claude Code matches hosts itself; the engine never parses a URL (the prompt server denies them all).
            "allow": [*([RUN_ROW_TOOL] if _allows_run_row(site, granted) else []),
                      *(f"Read(/{folder}/**)" for folder in site.knowledge.values()),
                      *([f"Write(/{site.output_file})", f"Edit(/{site.output_file})"] if site.output_file else []),
                      *(f"mcp__{SERVER_NAME}__{tool}" for tool in granted.tools if tool != "run_row"),
                      *(f"mcp__{server}" for server in granted.mcp),
                      *(f"WebFetch(domain:{host})" for host in hosts)],
            # Not `blockReadsOutsideWorkingDirectories` for a lane: measured on 2.1.322, it also hides from sandboxed
            # commands everything outside the working folders but system paths, the engine's own `sakura` included.
            "blockWritesOutsideWorkingDirectories": True,
            **_read_tool_limits(site),
        },
        "sandbox": {
            "enabled": True,
            "failIfUnavailable": True,
            "autoAllowBashIfSandboxed": site.role != grants.SCHEDULED_CLASS,
            "allowUnsandboxedCommands": False,
            "allowAppleEvents": False,
            "filesystem": {
                # Default-deny in the home: the narrower rule wins, so secrets stay denied inside allowed folders.
                "denyRead": with_aliases([*TOP_LEVEL_READ_DENIALS, str(home), *OUTSIDE_HOME_READ_DENIALS, *secrets,
                                       *denied_engine_paths(site, home)]),
                "allowRead": [*readable_paths(site, policy, cage, home), *_system_reads(home, secrets, site.engine_roots)],
                "allowWrite": [_real(str(cage.tmp)), _real(str(site.common_dir))],
                # The same secret list as every other enforcement point: a secret is never moved or linked away.
                "denyWrite": with_aliases([*_protected_writes(site, cage.tmp), *secrets]),
            },
            # Loopback only through the proxy and only on the site's ports, on both address families: a bare host
            # would open every local port, and `allowLocalBinding` opens every one (a host policy's choice).
            "network": {"allowedDomains": [*hosts, *loopback_domains(site)],
                        "strictAllowlist": True, "allowLocalBinding": policy.local_network},
        },
    }


def seatbelt_profile(site: RunSite, policy: Policy, cage: RunCage, home: Path, *, read_only: bool = False) -> str:
    """The Seatbelt profile of an exec role or a contract check: the same limits as a Claude Code lane.

    Every read denied (metadata included) but for the system and toolchain trees, the workspace, its Git folder, the
    engine install, the run's own tmp folder, the home's toolchains and the policy's read paths; reads of secrets and
    engine state denied over all of those (the config a test run needs excepted). Writes only in the workspace, the Git
    common folder (hooks and config excepted), the run tmp folder and the named devices (a row run: only in its own
    build folder, the source and the Git folder are read-only). No network but loopback on the site's exact ports (the
    ephemeral range too when the person approved the project's own sockets), no signals to other processes, no Apple
    Events and none of the programs that reach outside. Other processes stay opaque (no process info, so no
    environment, beyond the run's own tree), and the pasteboard, the keychain and LaunchServices are out of reach.
    Later rules win, so each exception follows the rule it narrows.
    """
    secrets = _site_secrets(site, policy, home)
    writable = [cage.tmp] if site.build_dir or read_only else [site.workspace, site.common_dir, cage.tmp]
    # The writable roots themselves may not be moved or replaced by a link: the engine reuses them in later steps.
    # Seatbelt matches resolved paths, so each is denied by its resolved spelling, as the pinned Git folders are.
    roots = [cage.tmp] if site.build_dir else [site.workspace, site.common_dir, cage.tmp]
    readable = readable_paths(site, policy, cage, home)
    system = [*SYSTEM_READ_TREES, *SYSTEM_READ_FILES, *_interpreter_trees(home, secrets, site.engine_roots)]
    lines = ["(version 1)", "(allow default)",
             # Default-deny: nothing is readable (metadata included) but what is listed below.
             "(deny file-read*)",
             f"(allow file-read* {_filters(system)} {_device_filters()})",
             f"(deny file-read* {_filters(with_aliases([str(home), *OUTSIDE_HOME_READ_DENIALS]))})",
             f"(allow file-read* {_filters(readable)})",
             f"(allow file-read* {' '.join(f'(literal {_quoted(path)})' for path in SYSTEM_LINKS)})",
             "(allow file-read-metadata " + " ".join(
                 f"(literal {_quoted(path)})" for path in _ancestors([*readable, *system], [Path("/")])) + ")",
             f"(deny file-read* {_filters(with_aliases([*secrets, *denied_engine_paths(site, home)]))})",
             # A caged command's stdout is its log file in the task's runs folder: a tool that stats its own output
             # (pytest does, and fails with a bad descriptor otherwise) gets the metadata there, never the contents.
             f"(allow file-read-metadata {_filters([str(site.task_root / RUNS_DIR)])})",
             # A secret inside a writable tree could be renamed or linked to a readable name: no writes either.
             f"(deny file-write* {_filters(secrets)})",
             f"(deny file-write* (require-not (require-any {_filters([str(path) for path in writable])} {_device_filters()})))",
             f"(deny file-write* {_filters([*_protected_writes(site, cage.tmp), *denied_engine_paths(site, home)])})",
             f"(deny file-write* {_literals(dict.fromkeys(_real(str(root)) for root in roots))})",
             f"(deny file-write* {_literals(_real(str(site.common_dir / name)) for name in PINNED_FOLDERS)})",
             "(deny network*)",
             "(deny signal)", "(allow signal (target same-sandbox))", "(deny appleevent-send)",
             f"(deny process-exec {' '.join(f'(literal {_quoted(path)})' for path in DENIED_EXECUTABLES)})",
             *PROCESS_INFO_RULES, f"(deny mach-lookup {' '.join(SENSITIVE_MACH_SERVICES)})"]
    if site.build_dir:
        # The row's own folder lies under the rows root every cage denies: this run alone gets it back.
        lines += [f"(allow file-read* file-write* {_filters([str(site.build_dir)])})",
                  f"(allow file-read-metadata (literal {_quoted(_real(str(site.build_dir.parent)))}))"]
    # Loopback: the exact ports the engine allocated and the host listed; the ephemeral range and binding anywhere only
    # when the person approved it for the project (or the host's cage policy chose `local_network`).
    lines += loopback.port_rules(site.loopback_ports)
    if site.local_sockets or policy.local_network:
        lines += loopback.socket_rules()
    return "\n".join([*lines, *_closing_denials(site, home)]) + "\n"


def _closing_denials(site: RunSite, home: Path) -> list[str]:
    """The engine's own folders, denied after every allowance (review 17, finding 1): a grant that resolves inside
    them (a build root swapped for a link) is overruled, whatever order the rules above were added in. The run's
    stdout metadata in the task's runs folder is allowed back, as it is above."""
    return [f"(deny file-read* file-write* {_filters(with_aliases(_engine_state_paths(site, home)))})",
            f"(allow file-read-metadata {_filters([str(site.task_root / RUNS_DIR)])})"]


def codex_args(site: RunSite, policy: Policy, cage: RunCage, home: Path) -> tuple[str, ...]:
    """The `-c` pairs that give a Codex run its permission profile, made from the same lists as the Claude cage.

    Both profiles read the minimal system paths, the workspace and its Git folder, what a lane reads in the home and
    Codex's own install, and nothing else; every secret path and engine folder is denied over all of that (a narrower
    rule wins, as in the Claude settings). A reviewer's profile (`sakura-review`) writes nothing and has no network (the
    model API is Codex's own connection, outside the profile). Any other role's (`sakura-work`) writes where the Claude
    cage does: the workspace, its Git folder, the run's tmp folder (a row run: its build folder and tmp only), with the
    Git hooks, config and the rest of `_protected_writes` read-only; its network is the Claude cage's list of hosts.
    """
    secrets = _site_secrets(site, policy, home)
    readable = [*readable_paths(site, policy, cage, home), *(str(home / path) for path in CODEX_INSTALL_PATHS),
                *_system_reads(home, secrets, site.engine_roots)]
    denials = {_real(path): "none" for path in with_aliases([str(home), *OUTSIDE_HOME_READ_DENIALS,
                                                                *secrets, *denied_engine_paths(site, home)])}
    writes = site.role not in config.REVIEW_ROLES
    online = writes and site.role != grants.DIGEST_CLASS  # the summarizer reads untrusted text: no network at all
    denials.update({pattern: "none" for pattern in (*CODEX_DENIED_DEVICES, *_codex_platform_patterns(writes))})
    reads = {":minimal": "read", **{_real(path): "read" for path in readable}}
    entries = {**reads, **(_codex_writes(site, cage) if writes else {}), **denials}
    profile = codex_profile.WORK_PROFILE if writes else CODEX_PROFILE
    return ("-c", f'default_permissions="{profile}"',
            "-c", f"permissions.{profile}.filesystem={codex_profile.filesystem_table(entries)}",
            "-c", f"permissions.{profile}.network={_codex_network(site, policy) if online else '{enabled=false}'}",
            *codex_profile.environment_pair({"PATH": _codex_path(readable)}),
            *(("-c", codex_profile.NETWORK_PROXY_FLAG) if online else ()))


def _codex_platform_patterns(writes: bool) -> list[str]:
    """What Codex's platform defaults read that a role does not need, as deny patterns (a plain entry loses to them).

    A glob deny beats a narrower read (measured), so a folder cannot be denied whole and read back. A role that writes
    runs Git and Python through the developer-tool shims, which need Xcode's folder and its license file: for those two
    folders every other entry is denied one by one, and Xcode's two are left (the listing is of this start)."""
    whole = CODEX_PLATFORM_DENIALS_WORK if writes else CODEX_PLATFORM_DENIALS
    patterns = [pattern for path in whole for pattern in (f"{path}*", f"{path}/**")]
    for folder, keep in (CODEX_WORK_KEEPS.items() if writes else ()):
        patterns += [pattern for entry in _listing(folder) if entry not in keep
                     for pattern in (f"{folder}/{entry}*", f"{folder}/{entry}/**")]
    return patterns


def _listing(folder: str) -> list[str]:
    try:
        return sorted(os.listdir(folder))
    except OSError:
        return []


def _codex_path(readable: list[str]) -> str:
    """The PATH of a Codex role's commands: the engine's own `bin` (where `sakura` is), the program folders the profile
    reads, then the system's. A folder the profile cannot read on the way fails a lookup with "operation not permitted"
    before the program is found, so the person's own PATH is not handed on."""
    folders = [str(KIT_BIN.parent), *(path for path in readable if os.path.basename(path) in ("bin", "sbin")),
               *git_program.SAFE_PATH, "/usr/local/bin"]
    return os.pathsep.join(dict.fromkeys(_real(folder) for folder in folders))


def _codex_writes(site: RunSite, cage: RunCage) -> dict[str, str]:
    """What a Codex run may write, as the Seatbelt profile has it (`seatbelt_profile`), and what stays read-only inside.

    A linked worktree's own folder inside the Git folder is named, because Codex protects it by itself. A glob path can
    only be denied in Codex, never kept read-only, and a glob such as `workspace/**/.git` also matches the workspace's
    own marker, which Git must read: so only the project record's glob is denied. The `.git` marker globs and the
    worktree pointer files are left out, and the engine's own check of the Git folder before every run, and its
    refusal of any marker or record it did not name, stand in."""
    if site.build_dir:
        roots = [cage.tmp, site.build_dir]
    elif site.role == grants.DIGEST_CLASS:  # the input folder is its workspace and stays read-only: it writes its output folder
        roots = [site.common_dir, cage.tmp]
    else:
        roots = [site.workspace, site.common_dir, cage.tmp, _worktree_git_folder(site)]
    protected = _protected_writes(site, cage.tmp)
    exact = {_real(path): "read" for path in protected if "*" not in path}
    records = {_real(path): "none" for path in protected if "*" in path and path.rpartition("/")[2] == engine_home.TASK_REPOSITORY_RECORD}
    return {**exact, **records, **{_real(str(root)): "write" for root in roots if root is not None}}


def _worktree_git_folder(site: RunSite) -> Path | None:
    """The folder of a linked worktree workspace inside the Git folder (its `.git` file's `gitdir:`); None for a main checkout."""
    marker = site.workspace / ".git"
    if not marker.is_file():
        return None
    match = re.match(r"gitdir:\s*(.+)", _read_small(marker))
    return Path(os.path.realpath(marker.parent / match.group(1).strip())) if match else None


def _codex_network(site: RunSite, policy: Policy) -> str:
    """The hosts the Claude cage allows, through Codex's proxy. The machine's own ports are all or none (Codex cannot name
    a port), so they are open only when the person approved local sockets or the cage policy chose `local_network`; the
    exact loopback ports of a row run stay out of reach of a Codex agent, which proves rows through the `run_row` tool
    (the engine runs them under its own Seatbelt profile with those exact ports)."""
    local = bool(site.local_sockets or policy.local_network)
    return codex_profile.network_table(allowed_domains(policy, site.workspace), local)


def require_program(argv: list[str]) -> None:
    """Raise FileNotFoundError when a Seatbelt-wrapped program does not exist, as an unwrapped spawn would."""
    if len(argv) > 3 and argv[0] == SEATBELT and shutil.which(argv[3]) is None:
        raise FileNotFoundError(f"no such program: {argv[3]}")


def readable_paths(site: RunSite, policy: Policy, cage: RunCage, home: Path) -> list[str]:
    """What a caged command may read in the home: the workspace and its Git folder, the engine install, the run
    tmp folder, the toolchains a lane needs and the policy's own extras. Secrets are denied over all of them."""
    extras = [str(home / path[2:]) if path.startswith("~/") else path for path in policy.read_paths]
    runtime = [KIT_BIN.parents[1] / name for name in ENGINE_RUNTIME_FOLDERS]
    stand_in = [interpreter.handed_test_interpreter() or ""]  # the commit brake starts it inside the cage: that one file
    paths = [site.workspace, site.common_dir, *runtime, cage.tmp, *(home / name for name in TOOLCHAIN_HOME_PATHS),
             *site.knowledge.values(), *(path for path in stand_in if path)]
    return list(dict.fromkeys([*map(str, paths), *SYSTEM_READ_BACK, *site.program_paths, *extras]))


def _require_narrow_reads(policy: Policy, home: Path, secrets: list[str], engine_roots: tuple[Path, ...]) -> None:
    """A policy read path may not be or hold the home, a secret folder or an engine home.

    Glob secrets (`.env*` files) are left to their deny rules, which win over a wider allow (measured).
    """
    real_home = Path(os.path.realpath(home))
    fixed = [Path(os.path.realpath(path)) for path in secrets if "*" not in path]
    fixed += [Path(os.path.realpath(root)) for root in engine_roots]
    for raw in policy.read_paths:
        path = Path(os.path.realpath(home / raw[2:] if raw.startswith("~/") else raw))
        for target in (real_home, *fixed):
            if path == target or path in target.parents:
                raise CageError(f"cage policy {policy.name}: read path {raw} covers {target}, which stays denied")


def denied_engine_paths(site: RunSite, home: Path) -> list[str]:
    """The engine homes, the operator state folders, the engine installation's Git metadata and the row runs' folders:
    no caged command reads or writes them."""
    return [*_engine_state_paths(site, home), str(rows_root())]


def _engine_state_paths(site: RunSite, home: Path) -> list[str]:
    """What stays denied after every allowance: the engine homes, the operator state and the installation's Git folders."""
    return [*map(str, site.engine_roots), *(str(home / relative) for relative in OPERATOR_HOME_PATHS),
            *install_git_paths(KIT_BIN.parents[1])]


def install_git_paths(install: Path) -> list[str]:
    """The resolved Git folders of the engine installation at `install`: its `.git` entry, the Git folder that entry
    names (a linked worktree's) and the common folder, read from the files themselves and never through Git, so no
    repository configuration is consulted. Their `config` and `config.worktree` can hold remote tokens and passwords."""
    entry = install / ".git"
    found = [Path(os.path.realpath(entry))]
    if entry.is_file():
        match = re.match(r"gitdir:\s*(.+)", _read_small(entry))
        if match:
            found.append(Path(os.path.realpath(entry.parent / match.group(1).strip())))
    if found[-1].is_dir() and (found[-1] / "commondir").is_file():
        found.append(Path(os.path.realpath(found[-1] / _read_small(found[-1] / "commondir").strip())))
    return list(dict.fromkeys(map(str, found)))


def _read_small(path: Path) -> str:
    try:
        return path.read_text(errors="replace")[:4096]
    except OSError:
        return ""


def rows_root() -> Path:
    """The folder under the system tmp where the engine makes each row run's writable folder (short for socket paths)."""
    return Path(os.path.realpath(tempfile.gettempdir())) / ROWS_ROOT


def new_build_dir() -> Path:
    """A fresh private folder for one row run's caches, build output and tmp, pinned (`flow/pinned_roots.py`): the
    steps of a completion session reuse it. The caller removes it."""
    root = rows_root()
    root.mkdir(mode=0o700, exist_ok=True)
    return pinned_roots.pin(Path(tempfile.mkdtemp(prefix="row-", dir=root)))


def _allows_run_row(site: RunSite, granted: grants.ClassGrant) -> bool:
    return site.role == "evaluator" and bool(site.row_env) and "run_row" in granted.tools


def server_policy(site: RunSite, cage: RunCage, granted: grants.ClassGrant = grants.ClassGrant()) -> dict:
    """What the prompt server needs: who asks, where to record refusals, the evaluator's row tool and the engine
    tools the role's class is granted.

    It allows nothing: what the cage allows is in Claude Code's own rules, so a prompt is always outside them.
    """
    from sakura.connectors import tools as connector_tools

    served = {"run": site.run, "role": site.role, "requests": str(cage.requests), "tools": list(granted.tools)}
    if prepared := connector_tools.prepare(site.role, granted, site.connector_ids):
        served["connectors"] = prepared
    if _allows_run_row(site, granted):
        served["run_row"] = {"cwd": str(site.workspace), "env": site.row_env}
    # The master has no repository of its own: its board and knowledge tools take the project in each call.
    if site.repo is None:
        return served
    if {"board_read", "board_post"} & set(granted.tools):
        # The author of a post is this run, from the engine's own record, never from a tool argument.
        served["board"] = {"root": str(engine_home.project_dir(site.repo)), "task": site.task_root.name,
                           "role": site.role, "run": site.run}
    if "ask_person" in granted.tools:
        served["ask"] = {"repo": str(site.repo), "task": site.task_root.name, "role": site.role, "run": site.run}
    if "knowledge_propose" in granted.tools:
        served["knowledge"] = {"engine": str(engine_home.root()), "repo": str(site.repo), "task": site.task_root.name,
                               "role": site.role, "run": site.run, "folders": dict(site.knowledge)}
    return served


def with_aliases(paths: list[str]) -> list[str]:
    """`paths` and the name each absolute one has on the data volume (`/System/Volumes/Data/...`), in that order."""
    aliases = [DATA_VOLUME + path for path in paths if path.startswith("/") and not path.startswith(DATA_VOLUME + "/")]
    return list(dict.fromkeys([*paths, *aliases]))


def secret_paths(policy: Policy, home: Path, workspace: Path) -> list[str]:
    """Absolute paths no caged run may read: the defaults, the toolchains' credential files, the policy's own and
    `.env*` files."""
    paths = [str(home / relative) for relative in SECRET_HOME_PATHS] + list(SECRET_SYSTEM_PATHS)
    paths += [str(path) for path in _account_folders(home) if str(path) not in paths]
    paths += _credential_files(home)
    paths += [str(home / path[2:]) if path.startswith("~/") else path for path in policy.secret_paths]
    return paths + [str(home / ENV_FILES), str(workspace / ENV_FILES)]


def _site_secrets(site: RunSite, policy: Policy, home: Path) -> list[str]:
    """`secret_paths` of a run, and the task's folders an isolated run may not see (`RunSite.denied_reads`): denied
    wherever a secret is (reads and writes, every tool, every enforcement point)."""
    return [*secret_paths(policy, home, site.workspace), *(str(path) for path in site.denied_reads),
            *(str(Path(os.path.realpath(root)) / ENV_FILES) for root in site.read_roots or ())]


def _credential_files(home: Path) -> list[str]:
    """The toolchains' credential files: their places in the home, the known ones inside the granted trees, the
    credential dotfiles anywhere in a granted tree, and the interpreter's site-wide pip config."""
    prefixes = dict.fromkeys(os.path.realpath(prefix) for prefix in (sys.base_prefix, sys.prefix))
    return [*(str(home / relative) for relative in (*CREDENTIAL_HOME_FILES, *CREDENTIAL_TOOLCHAIN_PATTERNS)),
            *(str(home / tree / "**" / name) for tree in TOOLCHAIN_HOME_PATHS for name in CREDENTIAL_DOTFILES),
            *(str(Path(prefix) / name) for prefix in prefixes for name in INTERPRETER_CREDENTIAL_FILES)]


def _account_folders(home: Path) -> list[Path]:
    """Every agent login folder (`sakura.accounts.folders`); an unreadable account registry fails closed."""
    from sakura.accounts import folders, registry

    try:
        return folders.login_folders(home)
    except registry.AccountsConfigError as error:
        raise CageError(f"cannot list the agent account folders to deny: {error}") from error


def allowed_domains(policy: Policy, workspace: Path) -> list[str]:
    """Registries the workspace uses, GitHub read endpoints, the model API and the policy's own hosts."""
    hosts = [host for marker, names in REGISTRIES.items() if (workspace / marker).exists() for host in names]
    return list(dict.fromkeys([*hosts, *GITHUB_READ, *MODEL_API, *policy.allowed_domains]))


def loopback_domains(site: RunSite) -> list[str]:
    """Claude Code's allow-list entries for the site's loopback ports: each port on IPv4, IPv6 and by name."""
    return [f"{host}:{port}" for port in site.loopback_ports for host in LOOPBACK_HOSTS]


def collect(cage: RunCage) -> list[dict]:
    """The requests the prompt server refused during the run, oldest first; an unreadable line is skipped."""
    if not cage.requests.is_file():
        return []
    entries = []
    for line in cage.requests.read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if isinstance(entry, dict):
            entries.append({key: str(entry.get(key, ""))[:300] for key in ("at", "run", "role", "tool", "detail", "reason")})
    return entries


def account_home() -> Path:
    """The account's home from the password database: `$HOME` is the agent's to change, never the cage's input."""
    return Path(pwd.getpwuid(os.getuid()).pw_dir)


def _read_tool_limits(site: RunSite) -> dict:
    """The built-in Read, Grep and Glob tools of a site with `read_roots`: blocked outside its folder and those roots.

    The sandbox governs Bash only; these tools are governed by permissions, so the master's reads need both."""
    if site.read_roots is None:
        return {}
    return {"blockReadsOutsideWorkingDirectories": True,
            "additionalDirectories": list(dict.fromkeys(os.path.realpath(root) for root in site.read_roots))}


def _read_root_rules(site: RunSite) -> list[str]:
    """Rules for the folders of `read_roots`: an extra directory is also a working folder, so the edit tools are denied
    there, and the Git folder of each is out of the Read tools' reach (Claude Code folds the case of the name)."""
    roots = with_aliases([os.path.realpath(root) for root in site.read_roots or ()])
    edits = [f"Edit(/{root}/**)" for root in roots]  # Edit rules cover every file-editing tool
    git = [f"Read(/{root}/.git{suffix})" for root in roots for suffix in ("", "/**")]
    return [*edits, *git]


def _deny_rules(site: RunSite, secrets: list[str], subagents: int) -> list[str]:
    rules = [rule for program in DENIED_PROGRAMS for rule in (f"Bash({program})", f"Bash({program} *)")]
    rules += [rule for command in DENIED_COMMANDS for rule in (f"Bash({command})", f"Bash({command} *)")]
    for path in with_aliases([*secrets, *map(str, site.engine_roots), *install_git_paths(KIT_BIN.parents[1])]):
        # `//` anchors a Read or Edit rule at the filesystem root; a folder is denied with everything under it,
        # a glob path keeps its glob. Read rules also cover Grep and Glob when those tools are on.
        patterns = [f"/{path}"] if "*" in path else [f"/{path}", f"/{path}/**"]
        rules += [f"{tool}({pattern})" for pattern in patterns for tool in ("Read", "Edit")]
    rules += [f"{tool}(./{ENV_FILES})" for tool in ("Read", "Edit")]
    rules += _read_root_rules(site)
    if site.role in ("plan_reviewer", "code_reviewer"):
        rules += list(WRITE_TOOLS)
    if site.role == grants.DIGEST_CLASS:
        rules += list(DIGEST_DENIED_TOOLS)
    if site.role == grants.SCHEDULED_CLASS:
        rules += [*DIGEST_DENIED_TOOLS, *WRITE_TOOLS, "Read"]
    if subagents == 0:
        rules += list(SUBAGENT_TOOLS)
    return rules


def _protected_writes(site: RunSite, tmp: Path) -> list[str]:
    """Paths a run must not write even inside a writable folder: Git hooks, config, attributes, the worktree
    pointers, the object database's indirection files, every `.git` marker and project record, the engine home and
    the row runs' folders."""
    common = site.common_dir
    # The pointers and per-worktree config that decide which Git folder and config a command resolves to, and the
    # attributes file: the agent's own work (index, HEAD, logs, refs, objects) stays writable.
    pointers = [str(common / "worktrees" / "**" / name) for name in WORKTREE_GITDIR_FILES]
    return [str(common / "hooks"), str(common / "config"), str(common / "config.worktree"),
            str(common / engine_home.TASK_REPOSITORY_RECORD),
            str(common / "info" / "attributes"), str(common / "refs" / "replace"),
            str(common / "info" / "grafts"), str(common / "shallow"), *pointers,
            *(str(common / name) for name in (*INDIRECTION_FILES, PACK_FOLDER_PIN)),
            *_repository_markers(site, tmp), *map(str, site.engine_roots), str(rows_root()),
            *map(str, site.protected_writes)]


def _repository_markers(site: RunSite, tmp: Path) -> list[str]:
    """What would make a folder a role writes pass for a repository the engine trusts (review 10, finding 1): the
    workspace's own `.git` marker, and a `.git` file or folder or a project record (`engine_home.TASK_REPOSITORY_RECORD`)
    anywhere a role writes: the workspace, the task's Git folder, the run's tmp folder and the system's tmp folders
    (Claude Code's own tmp folder for a lane's commands is in one), so none can be built in one and moved into
    another (a rename is checked by the moved folder's own name only). No Git command an agent
    runs in its workspace writes either; the cost is that a command cannot make a Git repository anywhere it writes.
    Case variants need no pattern of their own: measured, Seatbelt folds case on a case-insensitive volume for
    literal, subpath and regex rules alike, and the engine refuses a marker or record not named exactly."""
    marker, record = ".git", engine_home.TASK_REPOSITORY_RECORD
    # A pattern names the marker itself, never what is under it: a main checkout's `.git` folder is the Git folder,
    # whose contents the rules above already cover. A workspace's marker file is also named whole (Claude Code reads a
    # name as the file).
    own = [] if (site.workspace / marker).is_dir() else [str(site.workspace / marker)]
    roots = (site.workspace, site.common_dir, tmp, *map(Path, SYSTEM_TMP_ROOTS),
             Path(os.path.realpath(tempfile.gettempdir())))
    return [*own, *(str(root / "**" / name) for root in roots for name in (marker, record))]


def seal_source_profile(common: Path, git: str) -> str:
    """The Seatbelt profile of the source side of a seal (`git upload-pack` and every process it starts,
    `flow/sealed.py`): it reads the repository's common Git folder, the pinned Git and the system trees, and nothing
    else, so no alternates file, link or pointer an agent planted makes it read another repository's objects. It
    writes nothing but the data devices and has no network."""
    program = Path(os.path.realpath(git))
    helpers = program.parent.parent / "libexec" / "git-core"
    readable = [str(common), str(program), *([str(helpers)] if helpers.is_dir() else [])]
    system = [*SYSTEM_READ_TREES, *SYSTEM_READ_FILES]
    lines = ["(version 1)", "(allow default)", "(deny file-read*)",
             f"(allow file-read* {_filters(system)} {_filters(readable)} {_device_filters()})",
             f"(allow file-read* {_literals(SYSTEM_LINKS)})",
             f"(allow file-read-metadata {_literals(_ancestors([*readable, *system], [Path('/')]))})",
             f"(deny file-write* (require-not (require-any {_device_filters()})))",
             "(deny network*)"]
    return "\n".join(lines) + "\n"


def _private_folder(path: Path) -> None:
    """A real folder of this user with mode 0700, as Claude Code requires of its temp root; it sits in the run's
    tmp folder, which caged code writes, so it is opened without following a link and fixed through the descriptor."""
    with contextlib.suppress(FileExistsError):
        os.mkdir(path, 0o700)
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        raise CageError(f"{path} is not a real folder ({error.strerror}); remove it and run again") from None
    try:
        os.fchmod(descriptor, 0o700)
    finally:
        os.close(descriptor)


def _shared_tmp_dir(task_root: Path) -> Path:
    """The task's shared tmp folder, a short path under the system tmp (unix socket paths stay under the macOS
    limit): made and pinned with a record in the task folder on first use, checked against that record afterwards,
    from any process. Raises RootReplaced."""
    path, record = task_tmp_dir(task_root), task_root / CAGE_DIR / SHARED_TMP_RECORD
    try:
        os.mkdir(path, 0o700)
    except FileExistsError:
        return pinned_roots.verified(path, record)
    return pinned_roots.pin(path, record)


def new_isolated_tmp_dir(task_root: Path) -> Path:
    """A new private tmp folder for one isolated run, beside the task's shared one (a short path, for sockets): no
    earlier run's cage could write it. It is pinned (`flow/pinned_roots.py`); the caller records it and removes it
    when the run ends."""
    shared = task_tmp_dir(task_root)
    return pinned_roots.pin(Path(tempfile.mkdtemp(prefix=f"{shared.name}-", dir=shared.parent)))


def task_tmp_dir(task_root: Path) -> Path:
    """The tmp folder the runs of the task at `task_root` share (it may not exist yet)."""
    digest = hashlib.sha256(str(task_root).encode()).hexdigest()[:12]
    root = Path(os.path.realpath(tempfile.gettempdir())) / TMP_ROOT
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    return root / digest


def _write_json(path: Path, data: dict) -> None:
    _write_text(path, json.dumps(data, indent=2) + "\n")


def _write_text(path: Path, text: str) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}")
    temporary.write_text(text)
    temporary.replace(path)


def _ancestors(paths: list[str], roots: list[Path]) -> list[str]:
    """Each denied root and the folders between it and each readable path, so a path lookup can walk down to it."""
    found = set()
    for root in (Path(os.path.realpath(root)) for root in roots):
        found.add(str(root))
        for real in (Path(_real(path)) for path in paths):
            if root in real.parents:
                found.update(str(parent) for parent in real.parents if root in parent.parents)
    return sorted(found)


def _git_settings(site: RunSite) -> dict[str, str]:
    """Git settings for every caged command, as `GIT_CONFIG_*` variables so the person's repository config is never
    written: no signing, and the workspace's own hook folder when the engine installed one there."""
    settings = list(UNSIGNED_GIT)
    hooks = site.workspace / install.HOOKS_FOLDER
    if hooks.is_dir() and not site.build_dir:
        settings.append(("core.hooksPath", str(hooks)))
    return gitutil.config_environment(settings)


def _git_identity(repo: Path | None) -> dict[str, str]:
    """The person's commit identity, read by the engine outside the cage, for the commits agents make (none for the
    master, which commits nothing)."""
    identity = {}
    if repo is None:
        return identity
    for key, variables in GIT_IDENTITY:
        done = gitutil.run(repo, "config", key, person_config=True)
        if done.returncode == 0 and done.stdout.strip():
            identity.update({variable: done.stdout.strip() for variable in variables})
    return identity


def _filters(paths: list[str]) -> str:
    """Seatbelt path filters: a `**` pattern becomes a regex, anything else a subpath (a file or a folder).

    Seatbelt matches real paths (`/var` is `/private/var`), so every path is resolved first.
    """
    return " ".join(_filter(_real(path)) for path in paths)


def _real(path: str) -> str:
    captured = _captured(path)
    if captured is not None:
        return captured
    if "**/" in path:
        prefix, _, name = path.partition("/**/")
        return f"{os.path.realpath(prefix)}/**/{name}"
    return os.path.realpath(path)


def _captured(path: str) -> str | None:
    """`path` spelled from a captured root (or from its name on the data volume), or None when it is under none."""
    roots = _CAPTURED.get()
    if not roots:
        return None
    aliased = path.startswith(DATA_VOLUME + "/")
    bare = path[len(DATA_VOLUME):] if aliased else path
    for root, spelling in roots.items():
        if bare == root or bare.startswith(root + "/"):
            return (DATA_VOLUME if aliased else "") + spelling + bare[len(root):]
    return None


def _filter(path: str) -> str:
    if path == "/":
        return '(literal "/")'
    if "**/" in path:
        prefix, _, name = path.partition("/**/")
        pattern = "^" + re.escape(prefix) + "/(.*/)?" + re.escape(name).replace("\\*", "[^/]*") + "$"
        return f'(regex #"{pattern}")'
    return f"(subpath {_quoted(path)})"


def _system_reads(home: Path, secrets: list[str], engine_roots: tuple[Path, ...]) -> list[str]:
    """The system and toolchain paths every cage lists as readable, for the settings files that take no root entry: the
    root folder itself is the Seatbelt profile's own rule (a `/` entry would open the whole volume)."""
    return [*SYSTEM_READ_TREES, *(path for path in SYSTEM_READ_FILES if path != "/"),
            *_interpreter_trees(home, secrets, engine_roots), *SYSTEM_LINKS_READ_BY_NAME]


def _interpreter_trees(home: Path, secrets: list[str], engine_roots: tuple[Path, ...]) -> list[str]:
    """What of the interpreter the engine itself runs on a caged command may read, outside the system folders.

    Never the whole prefix: its `bin`, `include` and `lib/pythonX.Y` folders only. Under the home only a known
    per-version folder counts (`~/.pyenv/versions/<v>`, uv's `python/<v>`); and a folder that is or holds the home,
    a secret or an engine home, or lies inside one, is left out, so the grant never reaches what stays denied.
    """
    prefix = Path(os.path.realpath(sys.base_prefix))
    real_home = Path(os.path.realpath(home))
    if any(_inside(prefix, Path(root)) for root in SYSTEM_READ_TREES):
        return []
    if _inside(prefix, real_home) and not any(prefix.parent == real_home / known for known in HOME_INTERPRETER_PARENTS):
        return []
    folders = [prefix / name for name in INTERPRETER_FOLDERS if (prefix / name).is_dir()]
    denied = [*(Path(os.path.realpath(path)) for path in secrets if "*" not in path),
              *(Path(os.path.realpath(root)) for root in engine_roots)]
    return [str(folder) for folder in folders
            if folder != real_home and folder not in real_home.parents
            and not any(_inside(folder, target) or folder in target.parents for target in denied)]


def _inside(path: Path, root: Path) -> bool:
    return path == root or root in path.parents


def _device_filters() -> str:
    """The Seatbelt filters of the devices a caged command may write: exact files and the descriptor folder."""
    return " ".join([*(f"(literal {_quoted(device)})" for device in WRITABLE_DEVICES),
                     *(f"(subpath {_quoted(folder)})" for folder in WRITABLE_DEVICE_FOLDERS)])


def _quoted(text: str) -> str:
    return json.dumps(text)


def _literals(paths) -> str:
    """Seatbelt filters for exactly these paths, as given (a folder itself, nothing below it; a link by its name)."""
    return " ".join(f"(literal {_quoted(path)})" for path in paths)





def _policy(path: Path, name: str, data: dict) -> Policy:
    if RETIRED_STACK_KEY in data:
        raise CageError(f"{path}: remove the stack key; the stack is the person's own command (sakura stack up|down) "
                        "and no cage policy reaches it")
    unknown = sorted(set(data) - POLICY_KEYS)
    if unknown:
        raise CageError(f"{path}: unknown keys {', '.join(unknown)}")
    lists = {key: data.get(key, []) for key in ("secret_paths", "allowed_domains", "read_paths")}
    for key, value in lists.items():
        if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
            raise CageError(f"{path}: {key} must be a list of strings")
    if not all(item.startswith(("/", "~/")) for item in [*lists["secret_paths"], *lists["read_paths"]]):
        raise CageError(f"{path}: secret_paths and read_paths are absolute or start with ~/")
    flags = {"local_network": data.get("local_network", False)}
    if not all(isinstance(value, bool) for value in flags.values()):
        raise CageError(f"{path}: local_network must be true or false")
    return Policy(name, tuple(lists["secret_paths"]), tuple(lists["allowed_domains"]), flags["local_network"],
                  tuple(lists["read_paths"]))
