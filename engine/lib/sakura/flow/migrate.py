"""One-time copy of agentkit-era reports and logs, kept in the Git common directory, into an archive.

The old `.git` folder was writable by the agents that worked in the repository, so nothing there carries authority
into the engine: task state, contracts, plans, rulings, evidence, calibration and anything else that could hold an
approval, a pass, a receipt, a mark, a phase or a remembered check is never migrated, and neither are trust records
or the old host folders (`~/.config/agentkit`, `~/.local/state/agentkit`). Only reports and logs (`ledger.md`,
`report.md`, `local-report.md` and `runs/**/*.log` of each task) are copied, as UTF-8 text, into
`<project>/migrated/<task>/`, a folder the engine never reads; a new task starts from nothing.

The whole plan is built first and printed before anything is copied. Nothing is deleted or overwritten: a file
already present at its new place is kept, so a second run copies nothing.

The source folders stay writable by agents while the plan is carried out, so the plan records each source's size
and SHA-256, read without following links or blocking from the regular, single-name file it names. The copy opens
the source the same way, requires the same device and inode, reads exactly the planned size into a new temporary
file beside the target (a file grown since the plan is refused) and gives it its final name only when its digest is
the planned one.
"""

from __future__ import annotations

import hashlib
import os
import stat
import uuid
from dataclasses import dataclass, field
from pathlib import Path

from sakura import engine_home, repository

OLD_NAME = "agentkit"
OLD_FLOW_DIR = "flow"
ARCHIVE_DIR = "migrated"
REPORT_NAMES = frozenset({"ledger.md", "report.md", "local-report.md"})
RUNS_DIR = "runs"
LOG_SUFFIX = ".log"
MAX_FILE_BYTES = 16 * 1024 * 1024
TRUST_RECORD = "trust record, not migrated: re-establish it (sakura test, sakura trust, sakura trust-codex)"
AUTHORITY = "task state or engine record (it may carry approvals, passes, receipts, marks or a phase), not migrated"
HOST_FOLDER = "agentkit host config or state, not migrated: copy what you still want by hand"
SYMLINK = "symlink, refused"
NOT_REGULAR = "not a regular single-name file, refused"
NOT_TEXT = "not a UTF-8 text file, refused"
TOO_LARGE = f"larger than {MAX_FILE_BYTES} bytes, refused"
CHANGED_SINCE_PLAN = "changed after the plan was made (not the same file, size and digest), not copied"
SOURCE_OPEN = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK  # a FIFO opens at once instead of waiting for a writer
TEMPORARY_OPEN = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
CHUNK_BYTES = 64 * 1024


@dataclass(frozen=True)
class Copy:
    """One file to copy, with what the plan saw of it."""

    source: Path
    target: Path
    device: int
    inode: int
    size: int
    sha256: str


@dataclass
class Plan:
    """What a migration will copy, what it keeps because the new place has it, and what it refuses and why."""

    project: Path
    copies: list[Copy] = field(default_factory=list)
    kept: list[Path] = field(default_factory=list)
    refused: list[tuple[Path, str]] = field(default_factory=list)


def plan(repo: Path, home: Path) -> Plan:
    """Plan the copy of `repo`'s old reports and logs; everything else old is listed as refused, with the reason."""
    common = repository.common_dir(repo)
    found = Plan(engine_home.project_dir(repo))
    old_flow = common / OLD_FLOW_DIR
    if old_flow.is_symlink():
        found.refused.append((old_flow, SYMLINK))
    elif old_flow.is_dir():
        for task in sorted(old_flow.iterdir()):
            _plan_task(task, found.project / ARCHIVE_DIR / task.name, found)
    _refuse_children(common / OLD_NAME, TRUST_RECORD, found)
    for folder in (home / ".config" / OLD_NAME, home / ".local" / "state" / OLD_NAME):
        if folder.exists() or folder.is_symlink():
            found.refused.append((folder, HOST_FOLDER))
    return found


def apply(found: Plan) -> list[tuple[Path, str]]:
    """Copy every planned file into the archive, created private to the user.

    Returns the planned sources that were not copied because they changed after the plan was made.
    """
    if found.copies:
        engine_home.ensure_private(found.project)
    changed = []
    for copy in found.copies:
        copy.target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if not _copy_verified(copy):
            changed.append((copy.source, CHANGED_SINCE_PLAN))
    return changed


def _plan_task(task: Path, archive: Path, found: Plan) -> None:
    """Plan a task's reports and logs; refuse its other entries, and anything at the top that is not a task."""
    if task.is_symlink() or not task.is_dir():
        found.refused.append((task, SYMLINK if task.is_symlink() else AUTHORITY))
        return
    for entry in sorted(task.iterdir()):
        if entry.name in REPORT_NAMES:
            _plan_file(entry, archive / entry.name, found)
        elif entry.name == RUNS_DIR and entry.is_dir() and not entry.is_symlink():
            _plan_logs(entry, archive / RUNS_DIR, found)
        else:
            found.refused.append((entry, AUTHORITY))


def _plan_logs(folder: Path, target: Path, found: Plan) -> None:
    for entry in sorted(folder.iterdir()):
        if entry.is_dir() and not entry.is_symlink():
            _plan_logs(entry, target / entry.name, found)
        elif entry.suffix == LOG_SUFFIX:
            _plan_file(entry, target / entry.name, found)
        else:
            found.refused.append((entry, AUTHORITY))


def _plan_file(source: Path, target: Path, found: Plan) -> None:
    """Record size and digest of a regular single-name UTF-8 file, read through one descriptor; else refuse it."""
    try:
        descriptor = os.open(source, SOURCE_OPEN)
    except OSError:  # gone, or a symlink (ELOOP)
        found.refused.append((source, SYMLINK if source.is_symlink() else NOT_REGULAR))
        return
    try:
        info = os.fstat(descriptor)
        reason = _refusal(info)
        data = b"" if reason else _read_exactly(descriptor, info.st_size)
    finally:
        os.close(descriptor)
    reason = reason or ("" if data is not None and _is_text(data) else NOT_TEXT)
    if reason:
        found.refused.append((source, reason))
    elif target.exists() or target.is_symlink():
        found.kept.append(target)
    else:
        found.copies.append(Copy(source, target, info.st_dev, info.st_ino, info.st_size,
                                 hashlib.sha256(data).hexdigest()))


def _refusal(info: os.stat_result) -> str:
    if stat.S_ISLNK(info.st_mode):
        return SYMLINK
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        return NOT_REGULAR
    return TOO_LARGE if info.st_size > MAX_FILE_BYTES else ""


def _copy_verified(copy: Copy) -> bool:
    """Copy `copy.source` only if it still is the planned file, size and digest; never overwrite the target.

    Returns False when it is not. Other I/O errors propagate.
    """
    try:
        source_fd = os.open(copy.source, SOURCE_OPEN)
    except OSError:  # gone, or now a symlink (ELOOP)
        return False
    try:
        info = os.fstat(source_fd)
        if _refusal(info) or (info.st_dev, info.st_ino, info.st_size) != (copy.device, copy.inode, copy.size):
            return False
        data = _read_exactly(source_fd, copy.size)
        if data is None or hashlib.sha256(data).hexdigest() != copy.sha256:
            return False
        _publish(data, copy.target, stat.S_IMODE(info.st_mode))
        return True
    finally:
        os.close(source_fd)


def _read_exactly(descriptor: int, size: int) -> bytes | None:
    """The first `size` bytes, or None when the file holds fewer, or more (it grew while it was read)."""
    os.set_blocking(descriptor, True)
    chunks, left = [], size
    while left:
        chunk = os.read(descriptor, min(CHUNK_BYTES, left))
        if not chunk:
            return None
        chunks.append(chunk)
        left -= len(chunk)
    return None if os.read(descriptor, 1) else b"".join(chunks)


def _publish(data: bytes, target: Path, mode: int) -> None:
    """Write `data` to a new temporary file beside `target` and link it there; an existing target is kept."""
    temporary = target.with_name(f".{target.name}.{uuid.uuid4().hex}.part")
    descriptor = os.open(temporary, TEMPORARY_OPEN, mode & 0o600)
    try:
        view = memoryview(data)
        while view:
            view = view[os.write(descriptor, view):]
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, target)  # fails when the target appeared meanwhile: nothing is overwritten
    except FileExistsError:
        pass
    finally:
        os.unlink(temporary)


def _refuse_children(source: Path, reason: str, found: Plan) -> None:
    if source.is_dir() and not source.is_symlink():
        found.refused.extend((child, reason) for child in sorted(source.iterdir()))


def _is_text(data: bytes) -> bool:
    if b"\0" in data:
        return False
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True
