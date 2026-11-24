"""Folders the engine makes, gives caged code to write, and uses again in a later step: pinned from creation.

Caged code may replace such a folder between two steps: move it away and put a link at its name. The next cage would
then create its folders through that link and resolve the link into a write grant (review 15, finding 1). So the
engine keeps the identity of each one from the moment it made it: its device and inode, read through a descriptor
opened without following a link, in this process; for the task's shared tmp folder, which runs of later processes
reuse, also in a record in the task folder, which no role writes. Before it creates anything in such a folder or names
it in a cage, it checks the path is still that folder: a real folder, reached through no link, of the pinned identity;
and whatever it creates there goes through a descriptor opened the same way and checked against that identity. A
folder that is not refuses the step (RootReplaced), and nothing names it. No descriptor stays open between steps: a
long engine process pins a folder for every run, and descriptors kept for each would run out (measured: the full
suite's process passed 1024, where `select` stops working).

A folder the engine did not pin in this process and holds no record of (a workspace made by an earlier process) is
pinned at its first check, once it is a real folder reached through no link: a link is never followed, and a real
folder at the same path is no widening.
"""

from __future__ import annotations

import contextlib
import json
import os
import stat
import threading
from pathlib import Path

DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
FOLDER_MODE = 0o700

_held: dict[str, tuple[int, int]] = {}
# The canonical spelling of each pinned folder, taken when it was pinned: a cage names a grant by this text and never
# resolves the folder again (a surviving child can point a link at its name for the moment a profile is generated).
_canonical: dict[str, str] = {}
_lock = threading.Lock()


class RootReplaced(RuntimeError):
    """A pinned folder is no longer the folder the engine made: it was replaced, or a link is on its path."""


def pin(path: Path, record: Path | None = None) -> Path:
    """Pin the folder the engine just made at `path` (and write its identity to `record`); returns `path`."""
    descriptor = os.open(path, DIRECTORY)
    try:
        info = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    identity = (info.st_dev, info.st_ino)
    text = spelling(path)
    with _lock:
        _held[str(path)] = identity
        _canonical[str(path)] = text
    if record is not None:
        _write_record(record, str(path), identity)
    return path


def verified(path: Path, record: Path | None = None) -> Path:
    """`path`, once checked to be the folder pinned at its creation: a real folder reached through no link, of the
    pinned identity (pinned now when this process holds none and `record` names none). Raises RootReplaced."""
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        raise RootReplaced(f"{path} is gone; nothing is granted on it") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise RootReplaced(f"{path} was replaced by a link or another kind of file; nothing is granted on it")
    if os.path.realpath(path) != spelling(path):
        raise RootReplaced(f"{path} is reached through a link; nothing is granted on it")
    with _lock:
        captured = _canonical.get(str(path))
    if captured is not None and captured != spelling(path):
        raise RootReplaced(f"{path} no longer lies where it was made; nothing is granted on it")
    expected = _identity(path, record) or (info.st_dev, info.st_ino)
    if (info.st_dev, info.st_ino) != expected:
        raise RootReplaced(f"{path} was replaced by another folder; nothing is granted on it")
    with _lock:
        held = str(path) in _held
    if not held:
        try:
            pin(path)
        except OSError as error:
            raise RootReplaced(f"{path} cannot be opened as a real folder ({error.strerror})") from None
        if _identity(path, None) != expected:
            raise RootReplaced(f"{path} was replaced while it was checked; nothing is granted on it")
    return path


def canonical(path: Path) -> str:
    """The canonical spelling of the pinned folder `path`, as it was when the engine pinned it: a grant is written
    from this text, never from a fresh resolution. Raises RootReplaced for a folder that is not pinned in this process."""
    with _lock:
        text = _canonical.get(str(path))
    if text is None:
        raise RootReplaced(f"{path} is not pinned in this process")
    return text


def make_folder(root: Path, name: str) -> Path:
    """The folder `name` directly in the pinned folder `root`, made through a descriptor of `root` opened without
    following a link and checked against its pinned identity (an existing link at `name` is refused, not followed)."""
    with _lock:
        identity = _held.get(str(root))
    if identity is None:
        raise RootReplaced(f"{root} is not pinned in this process")
    try:
        descriptor = os.open(root, DIRECTORY)
    except OSError as error:
        raise RootReplaced(f"{root} is not a real folder ({error.strerror}); nothing is granted on it") from None
    try:
        info = os.fstat(descriptor)
        if (info.st_dev, info.st_ino) != identity:
            raise RootReplaced(f"{root} was replaced by another folder; nothing is granted on it")
        with contextlib.suppress(FileExistsError):
            os.mkdir(name, FOLDER_MODE, dir_fd=descriptor)
        os.close(os.open(name, DIRECTORY, dir_fd=descriptor))
    except OSError as error:
        raise RootReplaced(f"{root / name} is not a real folder ({error.strerror}); nothing is granted on it") from None
    finally:
        os.close(descriptor)
    return root / name


def release(path: Path) -> None:
    """Forget the pin of `path` once the engine removed the folder."""
    with _lock:
        _held.pop(str(path), None)
        _canonical.pop(str(path), None)


def spelling(path: Path) -> str:
    """The canonical text of a folder (a pinned one is read back with `canonical`): its parent resolved, its own name kept (a link at the name is not followed)."""
    return os.path.join(os.path.realpath(path.parent), path.name)


def _identity(path: Path, record: Path | None) -> tuple[int, int] | None:
    with _lock:
        held = _held.get(str(path))
    if held:
        return held
    if record is None:
        return None
    try:
        recorded = json.loads(record.read_text()).get(str(path))
    except (OSError, ValueError):
        return None
    return tuple(recorded) if isinstance(recorded, list) and len(recorded) == 2 else None


def _write_record(record: Path, path: str, identity: tuple[int, int]) -> None:
    record.parent.mkdir(parents=True, exist_ok=True, mode=FOLDER_MODE)
    try:
        data = json.loads(record.read_text())
    except (OSError, ValueError):
        data = {}
    data[path] = list(identity)
    temporary = record.with_name(f".{record.name}.{os.getpid()}")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.replace(record)
