"""Reopening a finished run's conversation: the safe command, where it runs and the environment it needs."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura.flow import acp, goose_lane, resume_account, run_records
from sakura.flow.render import strip_controls
from sakura.flow.store import TaskStore

_NOT_ALPHANUMERIC = re.compile(r"[^A-Za-z0-9]")


class ReopenRefused(RuntimeError):
    """Raised when a run cannot be reopened now; the message says why and is safe to print."""


class TranscriptMissing(ReopenRefused):
    """Raised when a Claude run's conversation is no longer on disk, so a resume would start an empty one."""


# What a person is told when a run cannot be continued: one sentence each, no path in them.
NOT_AN_AGENT = "It is not an agent you can write to."
NO_SESSION = "It kept no conversation to continue."
FOLDER_GONE_DELIVERED = "Its folder is gone: the lane was delivered and its workspace removed."
FOLDER_GONE_ENDED = "Its folder is gone: its workspace was removed when its work ended."
DELIVERED_STATUSES = ("report_ready", "cleaned")
NO_TRANSCRIPT = "Its conversation is no longer on this Mac."


@dataclass(frozen=True)
class Reopening:
    """What to start in a window: the resume argv, the directory of the original run and the environment to add."""

    argv: list[str]
    cwd: Path
    env: dict[str, str]
    acp: bool = False


def plan_reopen(record: dict, store: TaskStore, state: dict, common_dir: Path, environ: Mapping[str, str] | None = None,
                home: Path | None = None) -> Reopening:
    """Return how to reopen the run `record` interactively, or raise ReopenRefused.

    The argv is the run's own safe resume command, built with the checked directory. Stored paths are not
    trusted: the directory must be absolute and still the repository or a workspace recorded in `state`, and the
    provider config root comes from the operator's environment `environ` (default os.environ), never from the
    record, because a config directory can carry hooks. The run must have been recorded under that same root. The environment holds the run's allowlisted flow
    variables, so the engine brakes apply, plus the provider's config variable when the root is not the default.
    A Claude run also needs its transcript on disk, or `claude --resume` would start an empty conversation.
    """
    name = strip_controls(str(record.get("name", "?")))
    cwd = _checked_directory(record, store, state, name)
    if record.get("provider") == "goose":
        if record.get("state") not in run_records.RESUMABLE_STATES:
            raise ReopenRefused("The Goose turn is still running.")
        if problem := resume_problem(record, store, state, common_dir, environ, home):
            raise ReopenRefused(problem)
        return Reopening([], cwd, {}, acp=True)
    try:
        argv = run_records.resume_argv({**record, "cwd": str(cwd)}, common_dir)
    except run_records.RunNotResumable as refusal:
        raise ReopenRefused(strip_controls(str(refusal))) from refusal
    environ = os.environ if environ is None else environ
    root = _operator_root(record, name, environ, home)
    if record["provider"] == "claude":
        _require_transcript(cwd, root, record["session_id"], name)
    return Reopening(argv, cwd, _environment(record, root, home or Path.home()))


def folder_gone(state: dict) -> str:
    """Why a run's folder is not there, in words that are true of the lane: delivered lanes have their workspaces removed;
    otherwise (a reviewer's copy of the code, removed when its review ended) the folder went with the work it was made for."""
    return FOLDER_GONE_DELIVERED if state.get("status") in DELIVERED_STATUSES else FOLDER_GONE_ENDED


def resume_problem(record: dict, store: TaskStore, state: dict, common_dir: Path,
                   environ: Mapping[str, str] | None = None, home: Path | None = None) -> str:
    """Why an ended run cannot be continued by writing to it, in a sentence for the person; "" when it can be.

    The account the conversation ran on is found again from the account configuration (`flow/resume_account.py`), and a
    Claude conversation must still be on disk under that account's folder. The causes a person can meet have a fixed
    sentence (no session recorded, the folder gone, the conversation gone); any other refusal keeps its own words."""
    if record.get("provider") not in run_records.TRANSCRIPT_ROOTS:
        return NOT_AN_AGENT
    is_goose = record.get("provider") == "goose"
    identity = record.get("session_id", "")
    has_session = acp.valid_session_id(identity) if is_goose else bool(run_records.SESSION_ID.fullmatch(str(identity)))
    if not has_session:
        return NO_SESSION
    if not Path(str(record.get("cwd", ""))).is_dir():
        return folder_gone(state)
    name = strip_controls(str(record.get("name", "?")))
    try:
        cwd = _checked_directory(record, store, state, name)
        resolved = resume_account.resolve_record(record, environ)
        if is_goose:
            return goose_lane.resume_problem(store, record, cwd)
        if record["provider"] == "claude":
            _require_transcript(cwd, resolved.root, record["session_id"], name)
    except resume_account.AccountProblem as problem:
        return str(problem)
    except TranscriptMissing:
        return NO_TRANSCRIPT
    except ReopenRefused as refusal:
        return str(refusal)
    return ""


def _checked_directory(record: dict, store: TaskStore, state: dict, name: str) -> Path:
    cwd = Path(str(record.get("cwd", "")))
    if not cwd.is_absolute():
        raise ReopenRefused(f"working directory {strip_controls(str(cwd))} of {name} is not absolute")
    if not cwd.is_dir():
        raise ReopenRefused(f"working directory {strip_controls(str(cwd))} of {name} no longer exists")
    try:
        return run_records.checked_cwd(store, state, cwd)
    except run_records.RunRecordError as refusal:
        raise ReopenRefused(strip_controls(str(refusal))) from refusal


def _operator_root(record: dict, name: str, environ: Mapping[str, str], home: Path | None) -> Path:
    root = Path(run_records.transcript_root(record["provider"], environ, home)).resolve()
    recorded = str(record.get("transcript_root", ""))
    if not recorded or Path(recorded).resolve() != root:
        raise ReopenRefused(f"{name} was recorded under another config root than {strip_controls(str(root))}")
    return root


def _require_transcript(cwd: Path, root: Path, session_id: str, name: str) -> None:
    slug = _NOT_ALPHANUMERIC.sub("-", str(cwd))
    if not (root / "projects" / slug / f"{session_id}.jsonl").is_file():
        raise TranscriptMissing(f"no transcript of {name} found under {strip_controls(str(root))}; "
                                "its conversation cannot be resumed")


def _environment(record: dict, root: Path, home: Path) -> dict[str, str]:
    env = run_records.flow_env(record.get("env") or {})
    variable, default = run_records.TRANSCRIPT_ROOTS[record["provider"]]
    if root != (home / default).resolve():
        env[variable] = str(root)
    return env
