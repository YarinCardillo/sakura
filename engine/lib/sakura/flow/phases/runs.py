"""Mapping of a failed agent run to the phase outcome, shared by every phase that runs an agent."""

from __future__ import annotations

from pathlib import Path

from sakura.flow import ask_words, flowfiles
from sakura.flow.engine import PhaseOutcome
from sakura.flow.gitutil import git
from sakura.flow.notify import ESCALATION_EVENT
from sakura.flow.render import strip_controls

APPROVE, REVISE, REJECT = "APPROVE", "REVISE", "REJECT"
# REJECT is the reviewer's hard no; it blocks exactly like REVISE: a fix round, then the person.
VERDICTS = (APPROVE, REVISE, REJECT)
MAX_DECLARED_ERROR_CHARS = 300
NO_RESULT_FINDING = "agent returned no result"
REJECTED_FINDING = "reviewer rejected the change"


def failure_outcome(result, role: str, phase: str) -> PhaseOutcome | None:
    """Return the outcome for a stopped, stalled or failed run of `role`, or None when it exited cleanly."""
    who = role.replace("_", " ")
    if result.stopped:
        return PhaseOutcome("stopped", f"stopped during {phase}")
    if getattr(result, "overflowed", False):
        return PhaseOutcome("blocked", f"output cap exceeded: {phase} was terminated; see {result.log_path}",
                            event=ESCALATION_EVENT)
    if getattr(result, "log_replaced", False):
        return PhaseOutcome("blocked", f"log replaced during the run: {phase} was terminated; see {result.log_path}",
                            event=ESCALATION_EVENT)
    if getattr(result, "left_running", False):
        return PhaseOutcome("blocked", f"{who} left processes running after it exited during {phase}; the engine "
                                       f"terminated them and takes no result from the run; see {result.log_path}",
                            event=ESCALATION_EVENT)
    if result.stalled:
        label = "timed out" if getattr(result, "timed_out", False) else "stalled"
        return PhaseOutcome("blocked", f"{label}: {phase}; see {result.log_path}", event=ESCALATION_EVENT)
    if result.exit_code != 0:
        return PhaseOutcome("blocked", f"{who} exited {result.exit_code} during {phase}; see {result.log_path}")
    declared = _declared_error(getattr(result, "result", None))
    if declared:
        return PhaseOutcome("blocked", f"{who} could not do its job in {phase}: {declared}; see {result.log_path}",
                            event=ESCALATION_EVENT)
    return None


def _declared_error(parsed: dict | None) -> str:
    """What a role says stopped it (`{"error": "..."}` in its result line), cleaned for the ledger; empty when it says nothing."""
    text = parsed.get("error") if isinstance(parsed, dict) else None
    cleaned = strip_controls(text).strip() if isinstance(text, str) else ""
    return "" if cleaned.casefold() == "none" else cleaned[:MAX_DECLARED_ERROR_CHARS]


def workspace_changes(cwd: Path, role: str, phase: str, head: str) -> PhaseOutcome | None:
    """Block when a read-only agent left uncommitted changes or a new commit in `cwd` (HEAD was `head`). The caches its tools
    leave behind and what the project's `.gitignore` ignores are no change."""
    found = flowfiles.uncommitted_beyond_caches(cwd)
    is_committed = git(cwd, "rev-parse", "HEAD").strip() != head.strip()
    if not found and not is_committed:
        return None
    return PhaseOutcome("blocked", ask_words.changed_workspace(role, phase, found, is_committed))


def parse_verdict(result: dict | None) -> tuple[str, list[str]]:
    """Read a review result (verdict in any case); anything but a well-formed verdict is a REVISE with `NO_RESULT_FINDING`.

    Only APPROVE lets a task go on: REVISE and REJECT both block it.
    """
    verdict = result.get("verdict") if isinstance(result, dict) else None
    verdict = verdict.upper() if isinstance(verdict, str) else verdict
    if verdict not in VERDICTS:
        return REVISE, [NO_RESULT_FINDING]
    findings = result.get("findings")
    if not isinstance(findings, list) or not all(isinstance(item, str) for item in findings):
        return REVISE, [NO_RESULT_FINDING]
    if verdict == REVISE and not findings:
        return REVISE, [NO_RESULT_FINDING]
    if verdict == REJECT and not findings:
        return REJECT, [REJECTED_FINDING]
    return verdict, findings
