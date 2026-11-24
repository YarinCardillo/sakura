"""Strict cockpit admission and detached startup receipts, separate from deliberate CLI recovery."""

from __future__ import annotations

import hashlib
import json
import time
import uuid
from pathlib import Path
from unicodedata import category

from sakura import text_limits
from sakura.flow import exits, inbox, owner_marks, tasks

# A flow that stands still and may be resumed from the cockpit: stopped, or blocked (`flow resume` resumes it as well; a
# failed one is not resumed, it is cleaned up or re-contracted).
STANDING_STATUSES = ("stopped", "blocked")
# The block reasons a retry cannot clear: `flow resume` runs the same step again and it blocks again at once, because what it
# waits for is not in the engine's hands (a merge on the host, a missing push address, a branch the profile forbids). Anything
# else the engine blocked on is something a retry can clear: a stall, a timeout, a missing program, a failed check, a workspace
# the engine can put right.
RETRY_FUTILE_PREFIXES = {
    "pull request not merged": "its pull request is not merged yet; merge it, or clean the lane up",
    "no push URL was recorded": "it has no push address; clean the lane up",
    "refusing to push": "its branch may not be pushed; clean the lane up",
}
STARTUP_SECONDS = 10
STARTUP_POLL_SECONDS = 0.05


def note_problem(note: str) -> str | None:
    """Return the reason an explicitly supplied Resume note is refused, or None."""
    if len(note) > owner_marks.MAX_NOTE or not note.strip():
        return f"must contain text and be at most {owner_marks.MAX_NOTE} characters"
    if any(character not in "\n\t" and category(character) == "Cc" for character in note):
        return "contains a control character other than newline or tab"
    return text_limits.hidden_character_problem(note)


def revision(state: dict) -> str:
    """Return the revision of this persisted state, including its resume generation."""
    return hashlib.sha256(json.dumps(state, sort_keys=True).encode()).hexdigest()


def retry_futile(state: dict, owner_marked: bool = False) -> str:
    """Why resuming this blocked flow would only block again (an empty string when a retry may clear it, and for a flow that
    is not blocked): decided from the engine's own block reason, here and nowhere else, by the snapshot's actions and by
    the admission check alike. A flow blocked on owner rows may resume only once every row is marked (`owner_marked`)."""
    if state.get("status") != "blocked":
        return ""
    if owner_marks.is_blocked_on_owner_rows(state):
        return "" if owner_marked else "owner rows are still open; mark them first"
    reason = str(state.get("reason") or "")
    return next((text for prefix, text in RETRY_FUTILE_PREFIXES.items() if reason.startswith(prefix)), "")


def require_stopped(state: dict, expected: str, owner_marked: bool = False) -> None:
    """Refuse a stale or no longer stopped flow before any driver work occurs.

    `owner_marked`: the flow is blocked only on owner rows and every one is marked, so it stands still as a stopped
    flow does and its next step is the same resume. A blocked flow stands still too.
    """
    live = any(run.get("state") in ("starting", "running") for run in (state.get("runs") or {}).values())
    standing = (state.get("status") in STANDING_STATUSES or owner_marked) and not retry_futile(state, owner_marked)
    if not standing or live or revision(state) != expected:
        raise tasks.TaskStartError("cannot resume: the stopped flow has changed; read its state again")


def admit(state: dict, expected: str, note: str, actor: str) -> None:
    """Consume the revision and queue its steering note in the same state write."""
    if revision(state) != expected:
        raise tasks.TaskStartError("cannot resume: the stopped flow has changed; read its state again")
    state.pop(tasks.PENDING_START_KEY, None)
    state["resume_generation"] = state.get("resume_generation", 0) + 1
    if note:
        command = {"id": str(uuid.uuid5(uuid.NAMESPACE_URL, f"sakura:resume:{expected}")),
                   "kind": "steer", "value": note, "actor": actor}
        state.setdefault("steer_pending", []).append(inbox.entry_of(command))


def acknowledge(path: str | None, outcome: str) -> None:
    """Publish the child's startup outcome; no receipt means the outcome is unknown."""
    if path:
        Path(path).write_text(outcome)


def wait_for_start(path: Path) -> int:
    """Wait briefly for the child, returning failure rather than guessing after a timeout."""
    deadline = time.monotonic() + STARTUP_SECONDS
    while time.monotonic() < deadline:
        if path.exists():
            outcome = path.read_text()
            if outcome in ("started", "refused"):
                return exits.OK if outcome == "started" else exits.USAGE
        time.sleep(STARTUP_POLL_SECONDS)
    return exits.FAILED
