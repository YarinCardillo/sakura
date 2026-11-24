"""How the engine launches a command that comes from configuration: the host's own, or a repository's.

Every such command runs in the workspace, where it executes the workspace's content (tests, conftest files, package
scripts), which is untrusted whoever wrote it. So every one runs in the checks cage (writes only in its workspace, no
secrets, no engine home, no network beyond the app's loopback) with the scrubbed environment, whatever its origin.
Origin decides only whether the person is asked first. A command of the personal or project layer, a preset or a
flag is the host's word: it needs no approval and runs through the shell as written. A command a repository supplies
(`.sakura/flow.toml` or `.sakura/local.toml`) must be on the host's approved list, remembered, or approved by the
person at the `checks` gate; otherwise it is refused.
"""

from __future__ import annotations

import contextlib
import os
import re
import secrets
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

from sakura.flow import cage, checks, process
from sakura.flow.config import FlowConfig

NAME_UNSAFE = re.compile(r"[^A-Za-z0-9_.-]")


class CommandNotApproved(RuntimeError):
    """A repository's command has not been approved; it was not run."""


@dataclass(frozen=True)
class Launch:
    """The argv and environment a configured command runs with."""

    argv: list[str]
    env: dict[str, str]


def plan(store, config: FlowConfig, command: str, cwd: Path, label: str, application_port: int = 0,
         build_dir: Path | None = None) -> Launch:
    """How to run `command` in `cwd`: in the checks cage with the scrubbed environment, approved first unless the
    host configured it.

    A caged command reaches loopback only on `application_port` and the host's approved dependent ports. With
    `build_dir` (the completion checkout) `cwd` is read-only and that folder is the only writable one.
    Raises CommandNotApproved when a repository's command is not approved, and cage.CageError when the cage policy
    cannot be written.
    """
    listed = config.values["checks"]
    is_host = command in checks.host_commands(config)
    require_approved(store, config, command)
    name = f"repo-{label}-{NAME_UNSAFE.sub('-', cwd.name)}"
    run_cage = checks.checks_cage(store, cwd, name, cage.load_policy(config.values["knobs"]["cage"]),
                                  checks.loopback_ports(config, application_port), build_dir, listed["local_sockets"])
    words = ["sh", "-c", command] if is_host else checks.argv(command, listed["approved"], listed["remembered"])
    return Launch(run_cage.wrap(words), {**process.scrubbed_environment(os.environ), **run_cage.env})


def require_approved(store, config: FlowConfig, command: str) -> None:
    """Raise CommandNotApproved unless `command` is the host's own, on its list, remembered or approved."""
    if command not in checks.host_commands(config):
        _require_approved(store, command, config.values["checks"])


def _require_approved(store, command: str, listed: dict) -> None:
    if set(checks.unapproved_commands([command], listed["approved"], listed["remembered"])) - set(checks.granted(store.read_state())):
        raise CommandNotApproved(f"the repository's command {command!r} waits for the person's approval "
                                 f"(sakura flow approve {store.task_id} {checks.APPROVAL_GATE})")


# A fresh file the engine creates itself: never an existing name, never through a link.
SCRATCH_OPEN = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_APPEND
FINAL_OPEN = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW
SCRATCH_NAME_BYTES = 8
SCRATCH_MODE = 0o600


@dataclass(frozen=True)
class ScratchLog:
    """A caged command's output file in the cage's own writable folder, made fresh and held open by the engine."""

    path: Path
    file: BinaryIO
    identity: tuple[int, int]


def scratch_log(env: dict, final: Path) -> ScratchLog:
    """A new output file for a caged command, in the cage's own writable folder, under a name nobody chose before.

    A program that reopens its stdout by path (pytest does) needs write access to that path, and the cage denies
    the whole engine home, where the task's logs live. The folder is the cage's, so the engine creates the file
    with O_EXCL and O_NOFOLLOW (a planted name or link raises FileExistsError) and keeps the descriptor: the
    command's stdout is that descriptor, and `keep_log` reads it back, never the path.
    """
    path = Path(env["TMPDIR"]) / f".{final.name}.{secrets.token_hex(SCRATCH_NAME_BYTES)}"
    descriptor = os.open(path, SCRATCH_OPEN, SCRATCH_MODE)
    info = os.fstat(descriptor)
    return ScratchLog(path, os.fdopen(descriptor, "ab+"), (info.st_dev, info.st_ino))


def keep_log(scratch: ScratchLog, final: Path) -> None:
    """Copy what a finished caged command wrote to `final` in the task folder, then close and remove the scratch file.

    The bytes come from the descriptor the engine holds, at most `process.OUTPUT_CAP_BYTES`, and only while it is
    the regular file the engine made; whatever the command did to the path in its folder changes nothing.
    """
    try:
        descriptor = scratch.file.fileno()
        info = os.fstat(descriptor)
        same = stat.S_ISREG(info.st_mode) and (info.st_dev, info.st_ino) == scratch.identity
        data = _read_capped(descriptor) if same else b""
        final.parent.mkdir(parents=True, exist_ok=True)
        with os.fdopen(os.open(final, FINAL_OPEN, SCRATCH_MODE), "wb") as stream:
            stream.write(data)
    finally:
        scratch.file.close()
        # The cage's own folder: whatever stands at the path now (a link, a file) is removed, never followed.
        with contextlib.suppress(FileNotFoundError, IsADirectoryError, PermissionError):
            os.unlink(scratch.path)


def _read_capped(descriptor: int) -> bytes:
    chunks, offset = [], 0
    while offset < process.OUTPUT_CAP_BYTES:
        chunk = os.pread(descriptor, min(process.OUTPUT_CAP_BYTES - offset, 1 << 20), offset)
        if not chunk:
            break
        chunks.append(chunk)
        offset += len(chunk)
    return b"".join(chunks)
