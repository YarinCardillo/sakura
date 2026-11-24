"""Verify executables for host-side digest sources."""

from __future__ import annotations

import json
import os
import shutil
import stat
from dataclasses import dataclass

from sakura import git_program, programs

SHEBANG_BYTES = 256
UNPROTECTED = "the digest program and its interpreter must be in a protected installation"
CHANGED = "the digest program installation changed; reconnect the program"


class InstallationError(ValueError):
    """The host cannot verify a digest source's executable."""


@dataclass(frozen=True)
class Installation:
    argv: tuple[str, ...]
    files: tuple[tuple[str, tuple[int, ...]], ...]

    def identity(self) -> str:
        return json.dumps(self.files, separators=(",", ":"))

    def verify(self) -> None:
        try:
            unchanged = all(_identity(os.stat(path, follow_symlinks=False)) == identity for path, identity in self.files)
        except OSError:
            unchanged = False
        if not unchanged:
            raise InstallationError(CHANGED)


def inspect(argv: tuple[str, ...]) -> Installation:
    """Resolve a script's interpreter and capture the identities of its protected executable files."""
    try:
        roots = programs.forbidden_roots()
    except (OSError, ValueError, RuntimeError):
        raise InstallationError(UNPROTECTED) from None
    executable, arguments = argv[0], argv[1:]
    program, identity, header = _read_executable(executable, roots)
    files = [(program, identity)]
    command = (program,)
    if header.startswith(b"#!"):
        interpreter, options = _interpreter(header)
        runtime, runtime_id, runtime_header = _read_executable(interpreter, roots)
        if runtime_header.startswith(b"#!"):
            raise InstallationError(UNPROTECTED)
        files.append((runtime, runtime_id))
        command = (runtime, *options, program)
    resolved = _arguments(arguments, roots, files)
    return Installation((*command, *resolved), tuple(files))


def _arguments(arguments: tuple[str, ...], roots: list[str], files: list) -> list[str]:
    resolved = []
    for argument in arguments:
        if os.path.isabs(argument):
            argument = _protected_path(argument, roots)
            try:
                info = os.stat(argument, follow_symlinks=False)
                if _shared_file(info):
                    raise InstallationError(UNPROTECTED)
                files.append((argument, _identity(info)))
            except OSError:
                raise InstallationError(UNPROTECTED) from None
        resolved.append(argument)
    return resolved


def _protected_path(path: str, roots: list[str]) -> str:
    canonical = os.path.realpath(path)
    if git_program.holding_folder(path, roots) or programs.untrusted(canonical, roots=roots):
        raise InstallationError(UNPROTECTED)
    return canonical


def _shared_file(info: os.stat_result) -> bool:
    return info.st_uid != 0 and info.st_nlink != 1


def _read_executable(path: str, roots: list[str]) -> tuple[str, tuple[int, ...], bytes]:
    if not os.path.isabs(path):
        raise InstallationError(UNPROTECTED)
    canonical = _protected_path(path, roots)
    if not os.access(canonical, os.X_OK):
        raise InstallationError(UNPROTECTED)
    try:
        flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
        with os.fdopen(os.open(canonical, flags), "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or _shared_file(info):
                raise InstallationError(UNPROTECTED)
            header = stream.read(SHEBANG_BYTES)
    except OSError:
        raise InstallationError(UNPROTECTED) from None
    return canonical, _identity(info), header


def _interpreter(header: bytes) -> tuple[str, tuple[str, ...]]:
    line, separator, _ = header[2:].partition(b"\n")
    if not separator:
        raise InstallationError(UNPROTECTED)
    try:
        words = line.decode("utf-8").strip().split(maxsplit=1)
    except UnicodeError:
        raise InstallationError(UNPROTECTED) from None
    if not words:
        raise InstallationError(UNPROTECTED)
    program, *options = words
    if program == "/usr/bin/env":
        if len(options) != 1 or not options[0].isalnum():
            raise InstallationError(UNPROTECTED)
        found = shutil.which(options[0], path=os.pathsep.join(git_program.SAFE_PATH))
        if not found:
            raise InstallationError(UNPROTECTED)
        return found, ()
    return program, tuple(options)


def _identity(info: os.stat_result) -> tuple[int, ...]:
    return info.st_dev, info.st_ino, info.st_size, info.st_ctime_ns, info.st_mode, info.st_nlink
