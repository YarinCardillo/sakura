"""The engine's own object store: the bytes it proves and delivers.

Agents write the Git object database their workspaces share, and Git does not hash an object again when it reads it
from a pack or a loose file for a checkout, a status, a diff or a push: a planted pack index or a rewritten loose
object makes a recorded object ID show other bytes there. So before any completion step the engine brings the
revision it is about to prove or deliver into a bare repository of its own, `<engine home>/<project>/sealed.git`,
which every cage denies (the engine home is denied whole). The copy goes through Git's file transport, never a
hardlink or an alternate: the workspace side sends a pack, and this side's `index-pack` (or `unpack-objects`) computes
every object ID from the bytes it received and runs the `fsck` checks on each object (`transfer.fsckObjects`,
`fetch.fsckObjects`). An object whose bytes do not hash to the ID its parent names never arrives, so the fetch fails
its connectivity check and the step is refused; the store never holds bytes under another object's ID. An object the
store already holds is not asked for again, so a substitution planted after a seal changes nothing.

The completion checkout (`flow/completion.py`) is a worktree of the store, and the report's diff and the push read
the store only. The store is the engine's: no agent and no repository config writes it, and every call names it.

The source side is the workspace's common Git folder, which agents write, so it must not lead to another repository:
an alternates file, a `commondir` or `.git` pointer, or an object folder replaced by a link would make the engine
import (and test, report and push) another same-user repository's private objects. The cage denies agents those
writes; before a seal the engine refuses a source whose `.git` does not name the task repository's own common folder,
that has any of those pointer files, or whose `objects`, `objects/info` or `objects/pack` is not a plain folder of
this user; and the source side of the fetch (`git upload-pack --strict` and everything it starts) runs in a Seatbelt
profile that reads only that common folder, the pinned Git and the system (`cage.seal_source_profile`), so a pointer
planted after the checks reaches nothing.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import re
import shlex
import stat
from pathlib import Path
from typing import Iterator
from urllib.parse import quote

from sakura import engine_home, git_program
from sakura.flow import cage, gitutil

STORE_NAME = "sealed.git"
# One ref per sealed revision keeps its objects reachable in the store.
REF_PREFIX = "refs/sealed/"
LOCK_NAME = "sealing.lock"
FOLDER_MODE = 0o700
OBJECT_ID = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
ALTERNATES = Path("objects") / "info" / "alternates"
# Every received object is hashed and checked; no maintenance runs in the background after a fetch.
FETCH_CONFIG = ("-c", "transfer.fsckObjects=true", "-c", "fetch.fsckObjects=true", "-c", "protocol.version=2",
                "-c", "gc.auto=0", "-c", "maintenance.auto=false")
FETCH_OPTIONS = ("--no-tags", "--no-write-fetch-head", "--no-recurse-submodules", "--no-auto-maintenance", "--quiet")
# The Seatbelt profile the source side of a fetch runs in, beside the store in the engine home.
SOURCE_PROFILE = "seal-source.sb"


class SealRefused(gitutil.GitError):
    """The revision could not be brought into the engine's store intact; nothing may be proved or delivered from it."""


def path(repo: Path) -> Path:
    """Where the engine keeps the store of the project `repo` (or any of its workspaces) belongs to; it may not exist
    yet."""
    return engine_home.project_dir(repo) / STORE_NAME


def seal(source: Path, revision: str) -> Path:
    """Bring `revision` from the object database of `source` (a workspace) into its project's store; returns the store.

    Raises SealRefused when `revision` is not a full object ID, the fetch fails (a missing object, an object whose bytes
    do not hash to its ID, an fsck error) or the store does not then hold exactly that commit with all it reaches.
    """
    if not OBJECT_ID.fullmatch(revision):
        raise SealRefused(f"the engine seals only a full object ID, not {revision!r}")
    common = _source_common_dir(source)
    _require_plain_source(common)
    store = _opened(source)
    with _locked(store):
        done = gitutil.run(store, *FETCH_CONFIG, "fetch", *FETCH_OPTIONS, f"--upload-pack={_upload_pack(source, common)}",
                           file_url(common), f"+{revision}:{REF_PREFIX}{revision}", local_transfer=True)
        if done.returncode:
            raise SealRefused(f"{revision[:12]} could not be brought into the engine's object store intact; nothing "
                              f"is proved or delivered from it: {(done.stderr or done.stdout).strip()}")
        _require_sealed(store, revision)
    return store


def _source_common_dir(source: Path) -> Path:
    """The common Git folder of `source`, verified as the task repository's own; SealRefused otherwise."""
    try:
        return gitutil.verified_common_dir(source)
    except gitutil.GitError as error:
        raise SealRefused(f"nothing is sealed from {source}: {error}") from error


def _require_plain_source(common: Path) -> None:
    """Refuse a source whose object database could lead to another repository: a pointer file (`cage.
    INDIRECTION_FILES`) or an object folder that is a link, not a folder, or another user's."""
    for name in cage.INDIRECTION_FILES:
        if os.path.lexists(common / name):
            raise SealRefused(f"{common / name} exists: it points Git at another repository's objects, and the engine "
                              "seals nothing through it; remove it")
    for name in cage.PINNED_FOLDERS:
        try:
            info = os.lstat(common / name)
        except FileNotFoundError:
            continue
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
            raise SealRefused(f"{common / name} is not a plain folder of this user (a link, a file or another "
                              "owner's): the engine seals nothing from an object database that may lead elsewhere")


def _upload_pack(source: Path, common: Path) -> str:
    """The source side of the fetch: the pinned Git's `upload-pack --strict` (exactly that folder, no `.git` lookup)
    under the read-only profile, written beside the store; Git runs this command line through the shell."""
    program = git_program.path()
    profile = engine_home.project_dir(source) / SOURCE_PROFILE
    temporary = profile.with_name(f".{profile.name}.{os.getpid()}")
    temporary.write_text(cage.seal_source_profile(common, program))
    temporary.replace(profile)
    return shlex.join([cage.SEATBELT, "-f", str(profile), program, "upload-pack", "--strict"])


def _opened(source: Path) -> Path:
    """The store of `source`'s project, made on first use (private, no template, the source's object format) and
    trusted."""
    store = path(source)
    engine_home.ensure_project_dir(source)
    if not store.is_dir():
        store.mkdir(mode=FOLDER_MODE)
        gitutil.init_store(store, gitutil.git(source, "rev-parse", "--show-object-format").strip())
    gitutil.trust_store(store)
    if os.path.lexists(store / ALTERNATES):
        raise SealRefused(f"{store / ALTERNATES} exists: the engine's store borrows objects from nowhere; remove it")
    return store


def _require_sealed(store: Path, revision: str) -> None:
    """The store's ref names `revision`, which is a commit there, and every object it reaches is there."""
    named = gitutil.run(store, "rev-parse", "--verify", "--quiet", f"{REF_PREFIX}{revision}^{{commit}}")
    if named.returncode or named.stdout.strip() != revision:
        raise SealRefused(f"the engine's object store does not hold commit {revision[:12]} after the fetch")
    connected = gitutil.run(store, "fsck", "--connectivity-only", "--no-dangling", "--no-progress", revision)
    if connected.returncode:
        raise SealRefused(f"the engine's object store misses objects of {revision[:12]}: "
                          f"{(connected.stderr or connected.stdout).strip()}")


def file_url(common: Path) -> str:
    """The file transport URL of a common Git folder: Git then runs a real fetch, never a local copy.
    Git decodes the URL's percent escapes, so the path is quoted."""
    return f"file://{quote(str(common), safe='/')}"


@contextlib.contextmanager
def _locked(store: Path) -> Iterator[None]:
    """One seal at a time per store (the engine and an evaluator's row tool may seal at once)."""
    descriptor = os.open(store / LOCK_NAME, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)
