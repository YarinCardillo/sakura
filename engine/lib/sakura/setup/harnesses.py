"""Harnesses offered during first launch."""

import contextlib
import hashlib
import json
import os
import re
import signal
import stat
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from sakura import programs
from sakura.accounts.providers import (
    ADAPTERS,
    GOOSE,
    GOOSE_LOCAL_PROVIDER,
    GOOSE_NATIVE_DIRS,
)
from sakura.flow import acp, cage

ACP_VERSION = 1
GOOSE_VERSION = "1.60.2"
ACP_TIMEOUT = 10
ACP_ADAPTERS = {"claude": "claude-agent-acp", "codex": "codex-acp", "goose": "goose"}
NATIVE_ENV = {"claude": "CLAUDE_CODE_EXECUTABLE", "codex": "CODEX_PATH"}


Capabilities = acp.Capabilities
probe_message = acp.probe_message


@dataclass(frozen=True)
class Installation:
    """The adapter, runtime and native executable qualified for one harness."""

    harness: str
    adapter: programs.QualifiedProgram
    agent_name: str
    native: programs.QualifiedProgram | None
    runtime: programs.QualifiedProgram | None
    controlled: tuple[str, ...]
    setup_safe: bool = False
    lane_models: tuple[tuple[str, str], ...] = ()
    master_safe: bool = False

    @property
    def argv(self) -> tuple[str, ...]:
        """The absolute adapter command, with the exact protocol subcommand when required."""
        runner = (self.runtime.path,) if self.runtime else ()
        args = ("acp",) if self.harness == "goose" else ()
        return (*runner, self.adapter.path, *args)

    def recheck(self, cwd: Path) -> None:
        """Validate the directory and installed executable identities for this launch."""
        if not cwd.is_absolute() or cwd.resolve() != cwd or not cwd.is_dir():
            raise programs.ProgramError("invalid_launch_directory")
        try:
            controlled = list(dict.fromkeys((*self.controlled, *programs.controlled_folders(cwd))))
        except programs.ProgramError:
            raise programs.ProgramError("invalid_launch_directory") from None
        for program in (self.native, self.runtime, self.adapter):
            if program:
                program.recheck(controlled)

    def environment(self, account: Path | None = None) -> dict[str, str]:
        """Build a scrubbed adapter environment for the selected subscription account."""
        env = programs.environment("system", self.argv[0])
        if self.native:
            env[NATIVE_ENV[self.harness]] = self.native.path
        if account is not None and self.harness in NATIVE_ENV:
            adapter = ADAPTERS[self.harness]
            if self.harness != "claude" or account.resolve() != Path(adapter.default_dir).expanduser().resolve():
                env[adapter.env_var] = str(account)
        return env

    @contextlib.contextmanager
    def connect(self, cwd: Path, *, goose_root: Path | None = None, provider: str | None = None,
                account: Path | None = None, launch: tuple[tuple[str, ...], dict] | None = None,
                timeout: float = ACP_TIMEOUT, checkpoint: Callable[[], None] | None = None):
        """Open a bounded ACP probe and dispose of its process group on every exit."""
        argv, env = launch if launch is not None else (self.argv, self.environment(account))
        if goose_root is not None:
            env.update(self.goose_environment(goose_root, provider))
        self.recheck(cwd)
        try:
            child = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, start_new_session=True)
        except OSError:
            raise acp.AcpError("adapter_could_not_start") from None
        with child:
            try:
                client = acp.Client(child, timeout, checkpoint=checkpoint)
                yield client, acp.initialize(client, self.agent_name, self.adapter.version)
            finally:
                with contextlib.suppress(ProcessLookupError, PermissionError):
                    os.killpg(child.pid, signal.SIGKILL)


    def goose_environment(self, root: Path, provider: str | None = None) -> dict[str, str]:
        """Build the Goose setup environment for the verified configuration root."""
        if self.harness != GOOSE or not self.setup_safe:
            raise programs.ProgramError("safe_setup_required")
        home = cage.account_home().resolve()
        if not HARNESS_BY_ID[GOOSE].matches(root, home) or root.resolve() != root or not root.is_dir():
            raise programs.ProgramError("configuration_changed")
        config = root / "config"
        if (not _private_path(root, stat.S_ISDIR) or not _private_path(config, stat.S_ISDIR, may_be_missing=True)
                or not _private_path(config / "config.yaml", stat.S_ISREG, may_be_missing=True)):
            raise programs.ProgramError("configuration_changed")
        env = {"HOME": str(home), "GOOSE_PATH_ROOT": str(root)}
        if provider is not None:
            env.update(GOOSE_PROVIDER=provider, GOOSE_MODE="approve", GOOSE_TELEMETRY_ENABLED="false")
            if provider == GOOSE_LOCAL_PROVIDER:
                env["OLLAMA_HOST"] = "http://127.0.0.1:11434"
        return env


def _private_path(path: Path, check_mode: Callable[[int], bool], *, may_be_missing: bool = False) -> bool:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return may_be_missing
    return (check_mode(info.st_mode) and info.st_uid == os.getuid()
            and not info.st_mode & (stat.S_IWGRP | stat.S_IWOTH))


def selected_program(name: str, location: Path | None, reason: str) -> str:
    """Resolve a required program and report a fixed, display-safe failure code."""
    try:
        return programs.path(name, location)
    except programs.ProgramMissing:
        raise programs.ProgramError(reason) from None
    except programs.ProgramError:
        raise programs.ProgramError("program_untrusted") from None


def qualify(harness: str, location: Path | None = None) -> Installation:
    """Match the selected adapter and its dependencies to trusted install metadata."""
    if harness not in ACP_ADAPTERS:
        raise programs.ProgramError("unsupported_harness")
    native_path = selected_program(harness, location, "install_program")
    adapter_path = (native_path if harness == "goose" else
                    selected_program(ACP_ADAPTERS[harness], location, "install_adapter"))
    record = programs.acp_install(harness)
    agent_name = record.get("agent_name")
    if not isinstance(agent_name, str) or not re.fullmatch(r"[a-zA-Z0-9_.@/-]{1,100}", agent_name):
        raise programs.ProgramError("qualification_required")
    roots = programs.controlled_folders(location)
    adapter = programs.qualify(adapter_path, record.get("adapter"), roots)
    if harness == "goose" and adapter.version != GOOSE_VERSION:
        raise programs.ProgramError("incompatible_install")
    native = programs.qualify(native_path, record.get("native"), roots) if harness in NATIVE_ENV else None
    runtime = _runtime(record.get("runtime"), location, roots)
    lane = record.get("lane_models")
    models = tuple((key, value) for key, value in lane.items()
                   if isinstance(value, str) and 0 < len(value) <= 256) if isinstance(lane, dict) else ()
    return Installation(harness, adapter, agent_name, native, runtime, tuple(roots), record.get("setup_safe") is True,
                        models, record.get("master_safe") is True)


def _runtime(record: object, location: Path | None, roots: list[str]) -> programs.QualifiedProgram | None:
    if record is None:
        return None
    return programs.qualify(selected_program("node", location, "install_adapter"), record, roots)


def master_acp(harness: str, account: str | None, folder: str) -> dict:
    """Describe the Master session binding while the unified launch still requires release proof."""
    identity = hashlib.sha256(json.dumps([harness, account, os.path.realpath(folder) if folder else None])
                              .encode()).hexdigest()
    return {"protocol_version": ACP_VERSION, "readiness": "unavailable", "reason": "release_proof_required",
            "binding": {"harness": harness, "account": account, "context": "master",
                        "storage_namespace": f"master:{harness}:{identity}"}}


@dataclass(frozen=True)
class Harness:
    """Trusted executable names and the known folder markers for one harness."""

    id: str
    name: str
    login_folder: re.Pattern[str]
    directory_marker: str
    file_markers: tuple[str, ...] = ()

    @property
    def adapter(self) -> str:
        """The trusted program name used for this harness's ACP connection."""
        return ACP_ADAPTERS[self.id]

    def matches(self, folder: Path, home: Path) -> bool:
        """Whether the candidate names an account root under the person's home."""
        return folder.parent == home and self.login_folder.fullmatch(folder.name) is not None

    def native_folders(self, home: Path) -> tuple[Path, ...]:
        """Known/configured default folders for this harness, in display order."""
        base = home / ADAPTERS[self.id].default_dir.removeprefix("~/")
        return (base, *(home / path for path in GOOSE_NATIVE_DIRS)) if self.id == GOOSE else (base,)

    def is_candidate(self, folder: Path, home: Path) -> bool:
        """Recognize a configurable root or a standard native installation directory."""
        return self.matches(folder, home) or (self.id == GOOSE and folder in self.native_folders(home))

    def has_login_structure(self, folder: Path) -> bool:
        """Recognize known structure using metadata only."""
        try:
            if self.id == GOOSE and folder.name == GOOSE:
                return (folder / "config.yaml").is_file()
            return ((folder / self.directory_marker).is_dir()
                    and (not self.file_markers or any((folder / marker).is_file() for marker in self.file_markers)))
        except OSError:
            return False


HARNESSES = (
    Harness("claude", "Claude", re.compile(r"\.[\w.-]*claude[\w.-]*", re.IGNORECASE),
            ADAPTERS["claude"].transcript_subdir, ("history.jsonl", "settings.json")),
    Harness("codex", "Codex", re.compile(r"\.[\w.-]*codex[\w.-]*", re.IGNORECASE),
            ADAPTERS["codex"].transcript_subdir),
    Harness(GOOSE, "Goose", re.compile(r"\.goose(?:-[\w.-]+)?", re.IGNORECASE), "config", ("config/config.yaml",)),
)
HARNESS_BY_ID = {harness.id: harness for harness in HARNESSES}
