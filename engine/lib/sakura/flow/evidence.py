"""Evidence-before-claims: a session must read an evidence file before writing that something passed."""

from __future__ import annotations

import json
import re
from pathlib import Path

from sakura import engine_home

CLAIM = re.compile(r"\b(PASS|DONE|done)\b|\"passes\"\s*:\s*true")
SESSIONS_DIR = "flow-sessions"
CLAIM_FILES = ("ledger.md", "progress.md")
NOT_CLAIMS = ("spec.md", "plan.md", "plan.json", "input.md", "report-body.md")
EVIDENCE_DIR_NAME = "evidence"
EXCHANGE_DIR = ".sakura-role"  # flow/exchange.py; named here so the brakes need no exchange import
EXCHANGE_GITIGNORE = b"*\n"  # what the engine writes in `.sakura-role/.gitignore`; it is how the engine's own folder is told apart
# Where a role's own evidence lives (review 14, finding 3): `evidence/role/` and, on calibration, `evidence/base/role/`.
# Every other evidence path is the engine's (its row logs among them): a role may cite it, never write it.
ROLE_EVIDENCE_DIR = "role"
ROLE_EVIDENCE_PREFIXES = ((EVIDENCE_DIR_NAME, ROLE_EVIDENCE_DIR), (EVIDENCE_DIR_NAME, "base", ROLE_EVIDENCE_DIR))


def is_role_evidence(parts: tuple[str, ...]) -> bool:
    """Whether the evidence path made of `parts` lies in a role's own evidence namespace."""
    return any(len(parts) > len(prefix) and parts[:len(prefix)] == prefix for prefix in ROLE_EVIDENCE_PREFIXES)


def is_claim(text: str) -> bool:
    """Whether `text` says that something passed or is done."""
    return CLAIM.search(text) is not None


def is_claim_file(task_root: Path, path: Path) -> bool:
    """Whether a write to `path` is a claim channel: a ledger, a progress note, or a file of the task folder
    or of a role's exchange folder other than evidence."""
    if path.name in CLAIM_FILES:
        return True
    for folder in (task_root, _exchange_run(path)):
        if folder is not None and folder in path.parents:
            return EVIDENCE_DIR_NAME not in path.relative_to(folder).parts[:1] and path.name not in NOT_CLAIMS
    return False


def is_evidence(task_root: Path, path: Path) -> bool:
    """Whether `path` lies under the task's evidence directory or the evidence directory of an exchange folder."""
    run = _exchange_run(path)
    return any(folder is not None and (folder / EVIDENCE_DIR_NAME) in path.parents for folder in (task_root, run))


def _exchange_run(path: Path) -> Path | None:
    """`<workspace>/.sakura-role/<run>` when `path` lies in a role's exchange folder (`flow/exchange.py`)."""
    parts = path.parts
    for index, part in enumerate(parts[:-2]):
        if part == EXCHANGE_DIR:
            return Path(*parts[:index + 2])
    return None


def mark(repo: Path, session: str) -> None:
    """Record that `session` read an evidence file."""
    target = _session_file(repo, session)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"evidence_read": True}))


def is_marked(repo: Path, session: str) -> bool:
    """Whether `session` has read an evidence file."""
    try:
        return json.loads(_session_file(repo, session).read_text()).get("evidence_read") is True
    except (OSError, ValueError, AttributeError):
        return False


def _session_file(repo: Path, session: str) -> Path:
    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", session or "anonymous")
    return engine_home.kit_dir(repo) / SESSIONS_DIR / f"{safe}.json"
