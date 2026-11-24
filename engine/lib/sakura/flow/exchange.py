"""The role exchange folder: a role reads its inputs and writes its outputs inside its own workspace.

A role never touches the engine home: the brakes refuse any mention of it. Before a run, every prompt value
that names a path in the task folder is mapped to the same relative path under
`<workspace>/.sakura-role/<run>/`, and the inputs that exist are copied there. After the run, only known
output files are copied back into the task folder: the names the phase declares, plus the evidence files
the evaluator's result names. `.sakura-role/` holds a `.gitignore` of `*` that keeps it out of Git and stays
behind; removing it could race with another run in the same workspace.

The workspace belongs to the agent, which may plant symlinks or swap files while the engine works. So every
step goes through folder descriptors opened without following links: `.sakura-role` must be a real folder,
no folder is walked, an output must be a regular file of at most MAX_OUTPUT_BYTES with a single name (a
second name is a hardlink to an inode the agent may not own) and the same device and inode before and after
the read, and it lands in the task folder as a new temporary file renamed into place. The engine creates the
declared output files itself, empty, before the role starts, so the role writes into inodes the engine made;
an output still empty after the run counts as not written.
"""

from __future__ import annotations

import contextlib
import os
import re
import shutil
import stat
import uuid
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Iterator

from sakura.flow.evidence import EVIDENCE_DIR_NAME, EXCHANGE_DIR, EXCHANGE_GITIGNORE, is_role_evidence
from sakura.flow.roles import RoleRefused
from sakura.flow.store import TaskStore

IGNORE_EVERYTHING = EXCHANGE_GITIGNORE
RUN_ID_LENGTH = 12
MAX_OUTPUT_BYTES = 8 * 1024 * 1024  # a full-page screenshot fits; anything larger is refused
CHUNK_BYTES = 64 * 1024
FILE_MODE = 0o644
FOLDER_MODE = 0o700
DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
# O_NONBLOCK: opening a FIFO the agent planted must fail or return at once, never wait for a peer.
READ = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
CREATE = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_NONBLOCK
TRUNCATE = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW | os.O_NONBLOCK
# What each phase's role hands back to the engine, relative to the task folder.
OUTPUTS = {
    "intake": ("spec.md",),
    "contract": ("contract.json",),
    "plan": ("plan.md", "plan.json"),
    "report": ("report-body.md", "owner-report.md"),
}
OUTPUT_NAMES = frozenset(name for names in OUTPUTS.values() for name in names)
LINKED = "has more than one name (a hardlink), or changed while it was read"
EVIDENCE_PHASES = ("calibrate", "evaluate")
OTHERS_FILE = "others.md"
OTHERS_PATH_VALUE = "others_path"


@dataclass(frozen=True)
class Prepared:
    """One run's exchange: its folder, the prompt values that point into it, and where outputs go back."""

    folder: Path
    task_root: Path
    values: dict[str, str]
    outputs: tuple[str, ...]


def prepare(store: TaskStore, request, others: str = "") -> Prepared:
    """Create the run's folder in `request.cwd`, copy the inputs it names and map their paths into it.

    `others` is the text of the run's `others.md` (`flow/others.py`); its path is the prompt value `others_path`.

    Raises RoleRefused when `.sakura-role` or its `.gitignore` is anything but a real folder and file.
    """
    parent = request.cwd / EXCHANGE_DIR
    name = f"{request.phase}-{uuid.uuid4().hex[:RUN_ID_LENGTH]}"
    with _real_folder(parent) as parent_fd:
        _write(parent_fd, ".gitignore", IGNORE_EVERYTHING, TRUNCATE)
        os.mkdir(name, FOLDER_MODE, dir_fd=parent_fd)
        with _descend(parent_fd, (name,), create=False) as folder_fd:
            declared = OUTPUTS.get(request.phase, ())
            values = {key: _mapped(store.root, parent / name, folder_fd, key, value, declared)
                      for key, value in request.values.items()}
            _create_outputs(folder_fd, OUTPUTS.get(request.phase, ()))
            _write(folder_fd, OTHERS_FILE, others.encode(), CREATE)
            values[OTHERS_PATH_VALUE] = str(parent / name / OTHERS_FILE)
    return Prepared(parent / name, store.root, values, OUTPUTS.get(request.phase, ()))


def add_input(prepared: Prepared, name: str, data: bytes) -> None:
    """Write an engine-made input into the run's folder: a new regular file, never through a link."""
    with _real_folder(prepared.folder.parent) as parent_fd, \
            _descend(parent_fd, (prepared.folder.name,), create=False) as folder_fd:
        _write(folder_fd, name, data, CREATE)


def evidence_names(result: dict | None) -> tuple[str, ...]:
    """The evidence files an evaluator's result names: `evidence/...` paths made of plain components."""
    rows = result.get("rows") if isinstance(result, dict) else None
    names = [row.get("evidence") for row in rows if isinstance(row, dict)] if isinstance(rows, list) else []
    kept = [name for name in names if _plain_relative(name) and PurePosixPath(name).parts[0] == EVIDENCE_DIR_NAME]
    return tuple(dict.fromkeys(kept))


def collect(prepared: Prepared, extra: tuple[str, ...] = ()) -> list[tuple[str, str]]:
    """Copy the known outputs back into the task folder and remove the run's folder; returns the refusals.

    Call it only once the run's process has exited.
    """
    refused = []
    with _real_folder(prepared.folder.parent) as parent_fd:
        try:
            with _descend(parent_fd, (prepared.folder.name,), create=False) as folder_fd:
                for name in prepared.outputs:
                    problem = _bring_back(folder_fd, prepared.task_root, name)
                    if problem:
                        refused.append((name, problem))
                for name in extra:
                    problem = _bring_back_evidence(folder_fd, prepared.task_root, name)
                    if problem:
                        refused.append((name, problem))
        except OSError as error:
            refused.append((prepared.folder.name, f"run folder is not a real folder ({error.strerror})"))
        shutil.rmtree(prepared.folder.name, dir_fd=parent_fd, ignore_errors=True)
    return refused


# -- inputs ---------------------------------------------------------------------------------------------------

def _mapped(task_root: Path, folder: Path, folder_fd: int, key: str, value: str, declared: tuple[str, ...]) -> str:
    """The value a prompt gets: a path in the task folder becomes its copy in the exchange, anything else stays.

    A role can read only what is in the exchange, so a task file that does not exist and that the phase does not
    declare as an output is refused here rather than left for the role to fail on.
    """
    path = Path(value)
    if not re.fullmatch(_task_path(task_root), value):
        return _without_task_folder(task_root, folder, key, value)
    relative = path.relative_to(task_root)
    if relative.parts:
        if not os.path.lexists(path) and not _is_declared_output(relative, declared):
            raise RoleRefused(f"prompt value {key} names {relative}, which is not in the task folder and is not an "
                              f"output of this phase")
        _copy_in(path, folder_fd, relative.parts)
    return str(folder / relative)


def _without_task_folder(task_root: Path, folder: Path, key: str, value: str) -> str:
    """A text that names the task folder (an error message about a file) gets the exchange folder in its place.

    A named file that exists is an input the engine forgot to declare: the role could not read it from the
    exchange, so that is refused instead of rewritten.
    """
    def rewrite(match: re.Match) -> str:
        if os.path.lexists(match.group(0)) and match.group(0) != str(task_root):
            raise RoleRefused(f"prompt value {key} names the task file {match.group(1)} inside a longer text; the role "
                              f"cannot read it there, so give the file a prompt value of its own")
        return str(folder) + match.group(1)

    return re.sub(_task_path(task_root), rewrite, value)


def _task_path(task_root: Path) -> str:
    """The pattern of a path in the task folder: the folder and plain names below it."""
    return re.escape(str(task_root)) + r"((?:/[\w.\-]+)*)"


def _is_declared_output(relative: Path, declared: tuple[str, ...]) -> bool:
    return relative.as_posix() in declared or relative.parts[0] == EVIDENCE_DIR_NAME


def _copy_in(source: Path, folder_fd: int, parts: tuple[str, ...]) -> None:
    """Copy an engine-owned file or folder into the run's folder; every write creates a new file, no link followed."""
    if source.is_symlink() or not (source.is_file() or source.is_dir()):
        return
    with _descend(folder_fd, parts[:-1], create=True) as fd:
        if source.is_file():
            _write(fd, parts[-1], source.read_bytes(), CREATE)
            return
        with contextlib.suppress(FileExistsError):
            os.mkdir(parts[-1], FOLDER_MODE, dir_fd=fd)
    for child in sorted(source.iterdir()):
        _copy_in(child, folder_fd, (*parts, child.name))


def _create_outputs(folder_fd: int, outputs: tuple[str, ...]) -> None:
    """Create each declared output, empty and engine-side, unless an older copy was already brought in."""
    for name in outputs:
        parts = PurePosixPath(name).parts
        with _descend(folder_fd, parts[:-1], create=True) as fd:
            with contextlib.suppress(FileExistsError, RoleRefused):
                _write(fd, parts[-1], b"", CREATE)


# -- outputs --------------------------------------------------------------------------------------------------

def _bring_back_evidence(folder_fd: int, task_root: Path, name: str) -> str:
    """Copy one evidence file the result names into the task folder when it lies in a role's own namespace
    (`evidence.ROLE_EVIDENCE_PREFIXES`); any other evidence path is the engine's: the role may cite it, and a copy it
    wrote there is refused, never copied over the engine's file."""
    if not _plain_relative(name) or is_role_evidence(PurePosixPath(name).parts):
        return _bring_back(folder_fd, task_root, name)
    parts = PurePosixPath(name).parts
    try:
        with _descend(folder_fd, parts[:-1], create=False) as fd:
            os.stat(parts[-1], dir_fd=fd, follow_symlinks=False)
    except FileNotFoundError:
        return ""  # a cited engine file the role did not write
    except OSError as error:
        return f"not a regular file in a real folder ({error.strerror})"
    return ("an engine-owned evidence path: a role's evidence goes under evidence/role/ (evidence/base/role/ on "
            "calibration), and nothing it wrote replaces the engine's file")


def _bring_back(folder_fd: int, task_root: Path, name: str) -> str:
    """Copy one known output into the task folder; returns why it was refused, or "" (also when it is absent)."""
    if not _plain_relative(name):
        return "not a plain relative path"
    parts = PurePosixPath(name).parts
    try:
        with _descend(folder_fd, parts[:-1], create=False) as fd:
            data = _read_regular(fd, parts[-1])
    except FileNotFoundError:
        return ""
    except OSError as error:
        return f"not a regular file in a real folder ({error.strerror})"
    if isinstance(data, str):
        return data
    if not data and name in OUTPUT_NAMES:
        return ""  # still the empty file the engine created: the role wrote nothing
    target = task_root.joinpath(*parts)
    target.parent.mkdir(parents=True, exist_ok=True)
    with _real_folder(target.parent) as fd:
        temporary = f".{target.name}.{uuid.uuid4().hex}.part"
        _write(fd, temporary, data, CREATE)
        os.replace(temporary, target.name, src_dir_fd=fd, dst_dir_fd=fd)  # a rename replaces a link, never follows it
    return ""


def _read_regular(dir_fd: int, name: str) -> bytes | str:
    """The bytes of a regular file opened without following a link, or the reason it is refused."""
    fd = os.open(name, READ, dir_fd=dir_fd)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            return "not a regular file"
        if before.st_nlink != 1:
            return LINKED
        os.set_blocking(fd, True)
        chunks, total = [], 0
        while total <= MAX_OUTPUT_BYTES and (chunk := os.read(fd, CHUNK_BYTES)):
            chunks.append(chunk)
            total += len(chunk)
        if total > MAX_OUTPUT_BYTES:
            return f"larger than {MAX_OUTPUT_BYTES} bytes"
        after = os.fstat(fd)
        if after.st_nlink != 1 or (after.st_dev, after.st_ino) != (before.st_dev, before.st_ino):
            return LINKED
        return b"".join(chunks)
    finally:
        os.close(fd)


# -- descriptors ----------------------------------------------------------------------------------------------

@contextlib.contextmanager
def _real_folder(path: Path) -> Iterator[int]:
    """`path` opened as a real folder (created when missing); RoleRefused when it is a link or anything else."""
    with contextlib.suppress(FileExistsError):
        os.mkdir(path, 0o755)
    try:
        fd = os.open(path, DIRECTORY)
    except OSError as error:
        raise RoleRefused(f"{path} is not a real folder ({error.strerror}); remove it and run again") from None
    try:
        yield fd
    finally:
        os.close(fd)


@contextlib.contextmanager
def _descend(base_fd: int, parts: tuple[str, ...], create: bool) -> Iterator[int]:
    """The folder `parts` below `base_fd`, each component opened without following a link."""
    fd = os.dup(base_fd)
    try:
        for part in parts:
            if create:
                with contextlib.suppress(FileExistsError):
                    os.mkdir(part, FOLDER_MODE, dir_fd=fd)
            child = os.open(part, DIRECTORY, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd
    finally:
        os.close(fd)


def _write(dir_fd: int, name: str, data: bytes, flags: int) -> None:
    try:
        fd = os.open(name, flags, FILE_MODE, dir_fd=dir_fd)
    except OSError as error:
        raise RoleRefused(f"cannot write {name} in the exchange folder ({error.strerror}); remove it and run again") from None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise RoleRefused(f"{name} in the exchange folder is not a regular file; remove it and run again")
        os.set_blocking(fd, True)
        view = memoryview(data)
        while view:
            view = view[os.write(fd, view):]
    finally:
        os.close(fd)


def _plain_relative(raw: object) -> bool:
    if not isinstance(raw, str) or not raw or "\\" in raw:
        return False
    path = PurePosixPath(raw)
    return not path.is_absolute() and all(part not in ("", ".", "..") for part in raw.split("/"))
