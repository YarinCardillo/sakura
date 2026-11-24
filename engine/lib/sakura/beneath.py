"""Files in folders caged code can write, opened by the engine without following a link and only as regular files.

The engine runs outside every cage. A workspace, a cage's own folders and the Git common folder can be written by
caged code, so any name there may turn into a link, a FIFO or another file at any moment. The engine opens such a
file below a folder it trusts one component at a time, each with O_NOFOLLOW (folders with O_DIRECTORY, the file
with O_NONBLOCK, so a FIFO cannot block it), checks that the descriptor is a regular file with a single name, and
reads through that descriptor only. A write never opens the existing name: it goes to a new file under a fresh
name in the same folder (O_EXCL), which is then renamed over the target, so a link or a second name standing at the
target is replaced, never written through.
"""

from __future__ import annotations

import contextlib
import hashlib
import os
import secrets
import stat
from pathlib import Path, PurePosixPath
from typing import Iterator

DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
READ = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
CREATE_NEW = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
TEMPORARY_NAME_BYTES = 8
FOLDER_MODE = 0o755
CHUNK_BYTES = 1 << 20


class FileTooLarge(OSError):
    """The file is larger than the limit the caller allows; nothing of it was read."""


class FileChanged(OSError):
    """The file grew, shrank, was rewritten or was replaced while it was being read."""


class NotRegular(OSError):
    """The name is there but is not a regular file with a single name (a folder, a FIFO, a device, a hard link)."""


def read_bytes(root: Path, relative: str) -> bytes:
    """The content of regular file `relative` below `root`, opened without following any link.

    Raises FileNotFoundError when it is missing, OSError (ELOOP) when a component is a link, NotRegular otherwise.
    """
    with _opened(root, relative) as descriptor:
        chunks = []
        while chunk := os.read(descriptor, CHUNK_BYTES):
            chunks.append(chunk)
        return b"".join(chunks)


def read_capped(root: Path, relative: str, limit: int) -> bytes:
    """The content of regular file `relative` below `root`, read once and only if it is at most `limit` bytes.

    The size comes from the open descriptor and a larger file is refused before any read (a sparse or huge one never
    reaches memory); exactly that many bytes are read, a byte more or less is a change; and the descriptor and the
    name are checked afterwards: the same file, with the same size, times and link count. A file that changed while
    it was read, or was swapped for another at its name, raises FileChanged. Raises as `read_bytes` does otherwise.
    """
    parts = _parts(relative)
    with _folder(root, parts[:-1], create=False) as folder:
        descriptor = os.open(parts[-1], READ, dir_fd=folder)
        try:
            _require_regular(descriptor, relative)
            os.set_blocking(descriptor, True)
            before = os.fstat(descriptor)
            if before.st_size > limit:
                raise FileTooLarge(f"{relative} is {before.st_size} bytes, more than the {limit} allowed")
            data = _read_exactly(descriptor, before.st_size, relative)
            if os.read(descriptor, 1):
                raise FileChanged(f"{relative} changed while it was read (it grew)")
            _require_unchanged(descriptor, folder, parts[-1], before, relative)
            return data
        finally:
            os.close(descriptor)


def digest(root: Path, relative: str) -> str:
    """The SHA-256 of regular file `relative` below `root`, streamed; raises as `read_bytes` does."""
    found = hashlib.sha256()
    with _opened(root, relative) as descriptor:
        while chunk := os.read(descriptor, CHUNK_BYTES):
            found.update(chunk)
    return found.hexdigest()


def readlink(root: Path, relative: str) -> str:
    """The target of link `relative` below `root`, its folders opened without following a link."""
    parts = _parts(relative)
    with _folder(root, parts[:-1], create=False) as folder:
        return os.readlink(parts[-1], dir_fd=folder)


def is_executable(root: Path, relative: str) -> bool:
    """Whether regular file `relative` below `root` has an execute bit; raises as `read_bytes` does."""
    with _opened(root, relative) as descriptor:
        return bool(os.fstat(descriptor).st_mode & 0o111)


def write_bytes(root: Path, relative: str, data: bytes, mode: int) -> None:
    """Write `data` to `relative` below `root` with `mode`, creating missing folders: a new file under a fresh name,
    renamed over the target, so nothing already at the target (a link, a second name of another file) is written."""
    parts = _parts(relative)
    with _folder(root, parts[:-1], create=True) as folder:
        temporary = f".{parts[-1]}.{secrets.token_hex(TEMPORARY_NAME_BYTES)}.part"
        descriptor = os.open(temporary, CREATE_NEW, mode, dir_fd=folder)
        try:
            try:
                os.fchmod(descriptor, mode)
                view = memoryview(data)
                while view:
                    view = view[os.write(descriptor, view):]
            finally:
                os.close(descriptor)
            os.rename(temporary, parts[-1], src_dir_fd=folder, dst_dir_fd=folder)
        except BaseException:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(temporary, dir_fd=folder)
            raise


def replace_symlink(root: Path, relative: str, target: str) -> None:
    """Make `relative` below `root` a link to `target`, creating missing folders: a new link under a fresh name,
    renamed over the target, so nothing already there is followed or written through."""
    parts = _parts(relative)
    with _folder(root, parts[:-1], create=True) as folder:
        temporary = f".{parts[-1]}.{secrets.token_hex(TEMPORARY_NAME_BYTES)}.part"
        os.symlink(target, temporary, dir_fd=folder)
        try:
            os.rename(temporary, parts[-1], src_dir_fd=folder, dst_dir_fd=folder)
        except BaseException:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(temporary, dir_fd=folder)
            raise


@contextlib.contextmanager
def _opened(root: Path, relative: str) -> Iterator[int]:
    parts = _parts(relative)
    with _folder(root, parts[:-1], create=False) as folder:
        descriptor = os.open(parts[-1], READ, dir_fd=folder)
        try:
            _require_regular(descriptor, relative)
            os.set_blocking(descriptor, True)
            yield descriptor
        finally:
            os.close(descriptor)


@contextlib.contextmanager
def _folder(root: Path, parts: tuple[str, ...], create: bool) -> Iterator[int]:
    """The folder `parts` below `root` (which the engine trusts), each component opened without following a link."""
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in parts:
            if create:
                with contextlib.suppress(FileExistsError):
                    os.mkdir(part, FOLDER_MODE, dir_fd=descriptor)
            child = os.open(part, DIRECTORY, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        yield descriptor
    finally:
        os.close(descriptor)


def _parts(relative: str) -> tuple[str, ...]:
    parts = PurePosixPath(relative).parts
    if not parts or PurePosixPath(relative).is_absolute() or any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"{relative!r} is not a plain relative path")
    return parts


def _require_regular(descriptor: int, relative: str) -> None:
    info = os.fstat(descriptor)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise NotRegular(f"{relative} is not a regular file with a single name")


def _read_exactly(descriptor: int, size: int, relative: str) -> bytes:
    chunks, remaining = [], size
    while remaining:
        chunk = os.read(descriptor, min(remaining, CHUNK_BYTES))
        if not chunk:
            raise FileChanged(f"{relative} changed while it was read (it shrank)")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _require_unchanged(descriptor: int, folder: int, name: str, before: os.stat_result, relative: str) -> None:
    after = os.fstat(descriptor)
    try:
        named = os.stat(name, dir_fd=folder, follow_symlinks=False)
    except FileNotFoundError:
        raise FileChanged(f"{relative} changed while it was read (it was removed or replaced)") from None
    if _identity(after) != _identity(before) or (named.st_dev, named.st_ino) != (before.st_dev, before.st_ino):
        raise FileChanged(f"{relative} changed while it was read")


def _identity(info: os.stat_result) -> tuple[int, ...]:
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)
