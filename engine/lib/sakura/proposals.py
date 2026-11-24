"""Proposals: an agent's suggested change to a knowledge folder, applied only when the person accepts it.

A proposal is one file, `<engine home>/proposals/<id>.json`, made by the engine from what the agent gave (folder,
path, text, reason) and what the engine knows (who asked, from which project, the digest of the target as it was).
Nothing reaches a knowledge folder before `accept`, which only the person's commands call (`proposal_cli.py`): it
refuses when the folder is not the one the proposal was made for, when the target is not a plain file below the
folder (a link anywhere on the way refuses, `beneath.py`), and when the target changed since the proposal; then it
writes the exact text under a fresh name and renames it into place. A decided proposal keeps its record in `decided/`.
Accepting twice is safe: the second finds no waiting proposal, and a retry after a crash between the write and the
record finds the target already holding the text and only records the decision.
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import json
import os
import re
import secrets
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from sakura import beneath, knowledge, text_limits
from sakura.flow import board

FOLDER = "proposals"
DECIDED = "decided"
LOCK = ".lock"
# What the Waiting detail shows of a proposal, in characters and in bytes: the person decides on the whole text, never on a cut one.
MAX_TEXT = 4000
MAX_REASON = 300
MAX_PENDING = 50
MAX_PATH = 200
MAX_DEPTH = 6
FILE_MODE = 0o644
PRIVATE_FILE_MODE, PRIVATE_FOLDER_MODE = 0o600, 0o700
ID_HEX_BYTES = 4
PROPOSAL_ID = re.compile(rf"p[0-9a-f]{{{ID_HEX_BYTES * 2}}}")
PATH_PART = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}")
AUTHOR_KINDS = ("role", "master")


class ProposalRefused(ValueError):
    """Raised, with a one-line reason, when a proposal is not acceptable, or an accept or reject does not apply."""


@dataclass(frozen=True)
class Proposal:
    """One proposal as stored; `base_digest` is the SHA-256 of the target when proposed ("" when it did not exist)."""

    id: str
    folder: str
    folder_path: str
    path: str
    text: str
    reason: str
    base_digest: str
    repo: str
    author_kind: str
    task: str
    role: str
    run: str
    created_at: str

    @property
    def digest(self) -> str:
        """A short digest of everything the person is shown (folder, path, reason, text): an answer names it."""
        return text_limits.content_digest({"folder": self.folder, "path": self.path, "reason": self.reason,
                                           "text": self.text})


def propose(root: Path, folders: Mapping[str, Path], repo: Path, folder: str, path: str, text: str, reason: str,
            author: board.Author, now: datetime) -> Proposal:
    """Record a proposal to put `text` at `path` in knowledge folder `folder` (one of `folders`) and return it.

    `root` is the engine home. Nothing is written to the folder. Raises ProposalRefused for a folder that is not
    configured, a path, text or reason outside the limits, a target that is not a plain file, an author that is not
    a role or the master, and when `MAX_PENDING` proposals already wait.
    """
    _require_author(author)
    base = _require_folder(folders, folder)
    _require_path(path)
    _require_text(text, reason)
    made = Proposal(f"p{secrets.token_hex(ID_HEX_BYTES)}", folder, str(base), path, text, reason, _digest_of(base, path),
                    str(repo), author.kind, author.task, author.role, author.run, text_limits.utc_text(now))
    with _locked(root):
        if len(pending(root)) >= MAX_PENDING:
            raise ProposalRefused(f"{MAX_PENDING} proposals are already waiting for the person")
        _write_json(_file(root, made.id), _record(made))
    return made


def pending(root: Path, repo: Path | None = None) -> list[Proposal]:
    """The proposals nobody has decided, oldest first; of `repo`'s project only when given. A file that is not a whole
    proposal is skipped."""
    folder = root / FOLDER
    files = folder.glob("p*.json") if folder.is_dir() else ()
    found = [proposal for path in files if PROPOSAL_ID.fullmatch(path.stem) and (proposal := _read(path))]
    return sorted((proposal for proposal in found if repo is None or proposal.repo == str(repo)),
                  key=lambda proposal: (proposal.created_at, proposal.id))


def accept(root: Path, proposal_id: str, digest: str, folders: Mapping[str, Path], by: str, now: datetime) -> Proposal:
    """Write the proposal's text to its target, as the person `by` (an actor text) decided, and record the decision.

    `digest` names the content the person saw. `folders` are the knowledge folders as configured now. Raises ProposalRefused: no longer waiting (also for another digest), the folder is not
    the one proposed for, the target is not a plain file, or it changed since the proposal.
    """
    with _locked(root):
        found = waiting(root, proposal_id, digest)
        _write_target(found, _require_same_folder(found, folders))
        _decide(root, found, "accepted", by, now)
    return found


def reject(root: Path, proposal_id: str, digest: str, by: str, now: datetime) -> Proposal:
    """Record that the person `by` refused the proposal; nothing is written. Raises ProposalRefused when not waiting."""
    with _locked(root):
        found = waiting(root, proposal_id, digest)
        _decide(root, found, "rejected", by, now)
    return found


def waiting(root: Path, proposal_id: str, digest: str) -> Proposal:
    """The undecided proposal `proposal_id` whose content has `digest`; raises ProposalRefused for a malformed id and
    for one not waiting, which includes one whose content is not what `digest` names."""
    if not PROPOSAL_ID.fullmatch(proposal_id):
        raise ProposalRefused(f"{proposal_id[:20]!r} is not a proposal id (p and eight hex digits)")
    found = _read(_file(root, proposal_id))
    if found is None or found.digest != digest:
        raise ProposalRefused(f"no longer waiting: proposal {proposal_id} was decided, changed or never made")
    return found


def _require_author(author: board.Author) -> None:
    fields = (author.task, author.role, author.run)
    if author.kind not in AUTHOR_KINDS or not all(board.IDENTIFIER.fullmatch(field) for field in fields):
        raise ProposalRefused(f"refused author {author.kind!r}: a role or the master, with plain identifiers")


def _require_folder(folders: Mapping[str, Path], folder: str) -> Path:
    if folder not in knowledge.NAMES:
        raise ProposalRefused(f"folder must be one of {', '.join(knowledge.NAMES)}")
    if folder not in folders:
        raise ProposalRefused(f"no {folder} knowledge folder is configured")
    return folders[folder]


def _require_path(path: str) -> None:
    parts = path.split("/")
    if (len(path) > MAX_PATH or len(parts) > MAX_DEPTH or not all(PATH_PART.fullmatch(part) for part in parts)
            or not path.endswith(".md")):
        raise ProposalRefused(f"path must be a relative .md path of up to {MAX_DEPTH} plain names (letters, digits, "
                              f"dot, dash, underscore; none starting with a dot), at most {MAX_PATH} characters")


def _require_text(text: str, reason: str) -> None:
    problem = text_limits.text_problem(text, MAX_TEXT) or text_limits.hidden_character_problem(text)
    if not problem and "\r" in text:
        problem = "a carriage return (use newlines), so that what the person sees is what is written"
    if not problem and len(text.encode()) > MAX_TEXT:
        problem = f"{len(text.encode())} bytes; the limit is {MAX_TEXT}"
    if problem:
        raise ProposalRefused(f"text: {problem}")
    problem = text_limits.one_line_problem(reason, MAX_REASON)
    if problem:
        raise ProposalRefused(f"reason: {problem}")


def _digest_of(folder: Path, path: str) -> str:
    """The SHA-256 of the target, "" when it does not exist; ProposalRefused when it or a folder on the way is no plain file."""
    try:
        return beneath.digest(folder, path)
    except FileNotFoundError:
        return ""
    except OSError as error:
        raise ProposalRefused(f"{path} is not a plain file below the folder (a link or another kind of file is in "
                              f"the way)") from error


def _sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def _require_same_folder(found: Proposal, folders: Mapping[str, Path]) -> Path:
    current = folders.get(found.folder)
    if current is None or str(current) != found.folder_path:
        raise ProposalRefused(f"the {found.folder} knowledge folder changed since the proposal; propose again")
    return current


def _write_target(found: Proposal, folder: Path) -> None:
    current = _digest_of(folder, found.path)
    if current == _sha(found.text):
        return
    if current != found.base_digest:
        raise ProposalRefused(f"{found.path} changed since the proposal was made; nothing was written")
    try:
        beneath.write_bytes(folder, found.path, found.text.encode(), FILE_MODE)
    except OSError as error:
        raise ProposalRefused(f"{found.path} is not a plain file below the folder: {error.strerror}") from error


def _decide(root: Path, found: Proposal, outcome: str, by: str, now: datetime) -> None:
    record = {**_record(found), "decision": {"outcome": outcome, "by": by, "at": text_limits.utc_text(now)}}
    path = _file(root, found.id)
    _write_json(path, record)
    decided = root / FOLDER / DECIDED
    decided.mkdir(mode=PRIVATE_FOLDER_MODE, exist_ok=True)
    # The decision is already in the file, which no longer counts as waiting: failing to tidy it away is harmless.
    with contextlib.suppress(OSError):
        os.replace(path, decided / path.name)


def _read(path: Path) -> Proposal | None:
    """The proposal stored at `path` when it is whole and undecided, else None."""
    try:
        record = json.loads(path.read_text())
        if "decision" in record:
            return None
        return Proposal(**{name: record[name] for name in Proposal.__dataclass_fields__})
    except (OSError, ValueError, KeyError, TypeError):
        return None


def _record(found: Proposal) -> dict:
    return {name: getattr(found, name) for name in Proposal.__dataclass_fields__}


def _file(root: Path, proposal_id: str) -> Path:
    return root / FOLDER / f"{proposal_id}.json"


def _write_json(path: Path, record: dict) -> None:
    temporary = path.with_name(f".{path.name}.part")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, PRIVATE_FILE_MODE)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(record, stream)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


@contextlib.contextmanager
def _locked(root: Path) -> Iterator[None]:
    """Hold the exclusive lock of the proposals folder, made when missing."""
    folder = root / FOLDER
    folder.mkdir(mode=PRIVATE_FOLDER_MODE, exist_ok=True)
    with os.fdopen(os.open(folder / LOCK, os.O_RDWR | os.O_CREAT, PRIVATE_FILE_MODE), "r+") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        yield
