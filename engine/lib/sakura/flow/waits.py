"""One list of what every task waits on from the person, across projects.

A wait is a point a task cannot pass without the person: an approval at a gate, an owner row to mark, a blocked task.
Everything here comes from the engine's records (task state, gate reason, contract rows); agent text is only
displayed, with control characters removed. A request the cage denied is information for the log and the report, never
a wait (`owner_marks.denials`). The cockpit attaches the actions of each wait; this module knows nothing of it.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from collections.abc import Mapping
from dataclasses import dataclass, replace
from pathlib import Path

from sakura import beneath, engine_home, proposals
from sakura.flow import (
    ask_words,
    checks,
    config,
    contract,
    controls,
    delivery,
    final_head,
    lane_questions,
    owner_marks,
    owner_report,
    store,
    tasks,
)
from sakura.flow.render import strip_controls
from sakura.scheduled import inbox as scheduled_inbox

APPROVAL, OWNER_ROW, BLOCKED, QUESTION, PROPOSAL = "approval", "owner_row", "blocked", "question", "proposal"
REVIEW = "review"
REVIEWED_DELIVERY = "reviewed_delivery"
AUTOMATION = "automation"  # the master proposed an automation; the project is `AUTOMATIONS_PROJECT`, the task its name
AUTOMATIONS_PROJECT, AUTOMATIONS_ROOT = "Automations", "/"
FLOW_SOURCE = "flow"
TRACK_SOURCE, TRACK_DECISION = "track", "track_decision"
STATUS_AWAITING = "awaiting_approval"
BLOCKED_STATUSES = ("blocked", "failed")
FINISHED_STATUSES = ("report_ready", "cleaned")
QUESTION_TITLE_CHARACTERS = 200
GATE_IN_REASON = re.compile(r"flow approve \S+ (\S+?)(?=[\s()]|$)")
DEFAULT_GATE = "spec"
# What a spec approval shows: the spec itself, as the lane wrote it, when it is at most this large.
SPEC_DETAIL_BYTES = 256 * 1024
SPEC_NOT_SHOWN = "The spec cannot be shown here."
READ_CLAUSE = re.compile(r"read \S+ first")
OVERLAP_GATE = "overlap"
# The budget gates and the state key of the guard wait (`engine.BUDGET_GATE`, `engine.BUDGET_WAIT_GATE`, `budget_guard.WAIT_KEY`).
BUDGET_GATE, BUDGET_WAIT_GATE, BUDGET_WAIT_KEY = "budget", "budget-wait", "waiting"
BUDGET_WAIT_GENERATION_KEY = "wait_generation"  # `budget_guard.WAIT_GENERATION_KEY`
FINAL_REVIEW_GATE, OPUS_GATE_PREFIX = "final-review", "opus:"  # `engine.FINAL_REVIEW_GATE`, `engine.OPUS_GATE_PREFIX`
FINAL_REVIEW_KEY = "final_review"  # `phases.final_review.STATE_KEY`
CONFIG_KEY, CONTRACT_KEY, QUESTIONS_KEY = "config", "contract", "questions"
# The phases from which the engine's head is final, so a mark the person gives is the one delivery checks (the mark
# names the head the person tried, and the build and evaluation steps still move it before the pr phase).
OWNER_ROW_PHASES = ("pr", "report", "cleanup")
# A wait's key names what the person decides and the revision of it that they were shown (`<name>@<revision>`), so an
# answer meant for one candidate or one head never lands on the next one (review 17, finding 4).
REVISION_SEPARATOR = "@"
REVISION_LENGTH = 16
# The count of candidates the contract phase has discarded or planned for the task: it only grows, and it is part of
# the revision of an approval, so a retry that reproduces the same contract bytes is still another candidate.
CANDIDATE_GENERATION_KEY = "candidate_generation"
NO_LONGER_WAITING = "no longer waiting: the engine has moved on since the page was drawn; nothing was done"


@dataclass(frozen=True)
class Wait:
    """One point a task waits on. `id` is stable across reads; `since` is a UTC ISO string or None.

    `source` is `flow` for the engine's own waits or `outside` for an outside agent's; `author` and `choices` are
    empty for an engine wait (an outside question carries its choices, an outside wait its author).
    """

    id: str
    kind: str
    repo: str
    project: str
    task: str
    title: str
    detail: str
    since: str | None
    source: str = FLOW_SOURCE
    author: str = ""
    choices: tuple[str, ...] = ()
    # An owner row: where workspace 0 is and where the engine's sealed head is, as the person was shown them. When they
    # differ a mark would be refused, and the cockpit offers the engine's restore (`sakura flow restore-head`) beside
    # the mark, which rechecks both under the task lock.
    workspace_head: str = ""
    engine_head: str = ""
    # The engine's own line when `detail` is a plain sentence made from it (a failed lookup, a command, a path): for a view
    # that shows technical detail on request; "" when the detail is the line.
    technical: str = ""
    # The title the lane was given (`flow/tasks.split_title`), for an ask of a titled lane; empty when it has none.
    lane_title: str = ""
    # What the reviewer found, one plain line each, for an ask that wants the person's call on a review (`final-review`); empty
    # for every other ask.
    findings: tuple[str, ...] = ()
    # Titled links an outside agent put in its text, as `(label, https URL)`; the text itself carries none of them.
    links: tuple[tuple[str, str], ...] = ()

    is_read: bool = False
    snoozed_until: str | None = None
    is_snoozed: bool = False
    wants_note: bool = False
    folder_missing: bool = False
    finding: dict | None = None
    track: dict[str, str] | None = None

    @property
    def head_moved(self) -> bool:
        """Whether workspace 0 is away from the engine's head."""
        return bool(self.workspace_head and self.engine_head and self.workspace_head != self.engine_head)


def bound_key(name: str, revision: str) -> str:
    """The key of a wait: its `name` (a gate, a row) and the `revision` of it the person is shown."""
    return f"{name}{REVISION_SEPARATOR}{revision}"


def split_key(key: str) -> tuple[str, str]:
    """`(name, revision)` of a wait key; the revision is empty for a key without one."""
    name, separator, revision = key.rpartition(REVISION_SEPARATOR)
    return (name, revision) if separator and name and revision else (key, "")


def approval_revision(task: store.TaskStore, state: dict, gate: str) -> str:
    """What the person is shown at an approval gate: the gate, the spec, the candidate commands and the contract they
    belong to, the plan, the saved contract and the engine's head, and at the overlap gate the lane and files the
    overlap names. A planner retry that writes a new candidate under the same gate, a spec edited after the page was
    drawn or an overlap with another lane is another revision, and so is a candidate whose contract and commands are
    the very same bytes: the generation advances whenever a candidate is discarded or planned again."""
    pending = state.get(checks.PENDING_KEY) or {}
    spec = hashlib.sha256(task.spec.read_bytes()).hexdigest() if task.spec.is_file() else ""
    overlap = json.dumps(state.get("overlap") or {}, sort_keys=True) if gate == OVERLAP_GATE else ""
    return _revision(gate, spec, pending.get("contract", ""), pending.get("sha256", ""), state.get("plan_sha256", ""),
                     state.get("contract_sha256", ""), final_head.recorded(state),
                     str(state.get(CANDIDATE_GENERATION_KEY, 0)), overlap, _budget_wait(state, gate))


def _budget_wait(state: dict, gate: str) -> str:
    """At the budget-wait gate, the whole waiting record (rule, action, deadline, reason) and the count of waits so far: another
    wait at the same head, or the same wait moved to another deadline, is another revision."""
    if gate != BUDGET_WAIT_GATE:
        return ""
    return json.dumps([state.get(BUDGET_WAIT_KEY) or {}, state.get(BUDGET_WAIT_GENERATION_KEY, 0)], sort_keys=True)


def advance_candidate_generation(state: dict) -> None:
    """Count one more candidate in `state` (`CANDIDATE_GENERATION_KEY`): called wherever one is discarded or planned."""
    state[CANDIDATE_GENERATION_KEY] = int(state.get(CANDIDATE_GENERATION_KEY, 0)) + 1


def owner_revision(state: dict, row_id: str) -> str:
    """What the person is shown at an owner row: the row, the saved contract and the engine's head. A mark counts only
    for that contract and head (`owner_marks.holds`), so a move of either makes the row another wait."""
    return _revision(row_id, state.get("contract_sha256", ""), final_head.recorded(state))


def _revision(*seen: str) -> str:
    return hashlib.sha256(json.dumps(seen, separators=(",", ":")).encode()).hexdigest()[:REVISION_LENGTH]


def collect(repos: list[Path]) -> list[Wait]:
    """Every wait of every task of `repos`, oldest first (a wait with no known time last); nothing is written."""
    return collect_stores({repo: store.task_stores(repo) for repo in repos})


def collect_stores(tasks: Mapping[Path, list[store.TaskStore]], *,
                   missing_projects: frozenset[Path] = frozenset(), read_state: store.ReadState | None = None) -> list[Wait]:
    """`collect` for tasks already listed, by repository."""
    found = [wait for repo, stores in tasks.items() for task in stores for wait in _of_task(repo, task, read_state)]
    proposals = [wait for repo in tasks for wait in _proposal_waits(repo, folder_missing=repo in missing_projects)]
    return ordered(found + proposals + _automation_waits() + scheduled_inbox.pending_waits())


def ordered(found: list[Wait]) -> list[Wait]:
    """`found` oldest first, a wait with no known time last."""
    return sorted(found, key=lambda wait: (wait.since is None, wait.since or "", wait.id))


def log_read_problem(error: Exception) -> None:
    """Keep the diagnostic from a failed lane read in the host log."""
    print(f"cockpit: {strip_controls(str(error))}", file=sys.stderr)


def _of_task(repo: Path, task: store.TaskStore, read_state: store.ReadState | None) -> list[Wait]:
    try:
        state = read_state(task) if read_state else task.read_state()
    except store.StateError as error:
        log_read_problem(error)
        return [_make(repo, repo.name, task, BLOCKED, "state", ask_words.BLOCKED_TITLE, ask_words.STATE_UNREADABLE, None)]
    try:
        cfg = config.load(repo, overrides=state.get("overrides") or None, folder_missing=task.folder_missing)
    except config.FlowConfigError as error:
        log_read_problem(error)
        return [_make(repo, repo.name, task, BLOCKED, CONFIG_KEY, ask_words.BLOCKED_TITLE,
                      ask_words.CONFIG_UNREADABLE, _since(state))]
    project = owner_report.project(cfg.values, repo.name)
    try:
        rows = _rows(task, state)
    except contract.ContractError as error:
        log_read_problem(error)
        return [_make(repo, project, task, BLOCKED, CONTRACT_KEY, ask_words.BLOCKED_TITLE,
                      "Lane contract unreadable. Check the host logs.", _since(state))]
    steps = config.steps(cfg.values)
    found = _status_waits(repo, project, task, state, rows, steps, cfg)
    found += _owner_waits(repo, project, task, state, rows, steps) + _question_waits(repo, project, task, state)
    title = str(state.get(tasks.TITLE_KEY) or "")
    return [replace(wait, lane_title=title) for wait in found]


def review_wait(task: store.TaskStore, state: dict) -> Wait | None:
    """Describe the current, unresolved pull request delivery before checking its remote state."""
    found = delivery.of(state)
    if not found or found["kind"] != delivery.PULL_REQUEST or not found["url"].startswith("https://"):
        return None
    url = found["url"]
    title = strip_controls(str(state.get(tasks.TITLE_KEY) or task.task_id))
    try:
        cfg = config.load(task.repo, overrides=state.get("overrides") or None, folder_missing=task.folder_missing)
        project = owner_report.project(cfg.values, task.repo.name)
    except config.FlowConfigError:
        project = task.repo.name
    wait = _make(task.repo, project, task, REVIEW, bound_key(REVIEW, _revision(url, found["head"])),
                 f"Review {title}", "Review the pull request for this lane.", delivery.delivered_at(state) or _since(state))
    if state.get(REVIEWED_DELIVERY) == wait.id:
        return None
    return replace(wait, lane_title=title, links=(("Open pull request", url),))


def waiting_gate(state: dict) -> str:
    """The gate the task waits at for the person's approval now (the engine's own record: its status and the reason it
    wrote), or an empty string when no gate waits."""
    if state.get("status") != STATUS_AWAITING:
        return ""
    match = GATE_IN_REASON.search(strip_controls(str(state.get("reason") or "")))
    return match[1] if match else DEFAULT_GATE


def gate_pending(state: dict, gate: str) -> bool:
    """Whether `gate` has an approval pending now, decided here for every caller: the gate the task awaits (its own status
    and reason), the budget gate while a preflight waits for the person, and the budget wait gate while a guard wait runs
    (both are pending whatever the task's status is)."""
    if waiting_gate(state) == gate:
        return True
    if gate == BUDGET_GATE:
        return bool((state.get("budget") or {}).get("preflight"))
    return gate == BUDGET_WAIT_GATE and bool(state.get(BUDGET_WAIT_KEY))


def _status_waits(repo: Path, project: str, task: store.TaskStore, state: dict, rows: list[contract.Row],
                  steps: config.PresetSteps, cfg: config.FlowConfig) -> list[Wait]:
    status, reason = state.get("status"), strip_controls(str(state.get("reason") or ""))
    since = _since(state)
    if status == STATUS_AWAITING:
        match = GATE_IN_REASON.search(reason)
        gate = match[1] if match else DEFAULT_GATE
        title, detail = _approval_words(task, state, gate, reason, cfg)
        return [replace(_make(repo, project, task, APPROVAL, bound_key(gate, approval_revision(task, state, gate)),
                              title, detail, since), findings=_review_findings(state, gate))]
    late = late_sentence(state)
    if status in BLOCKED_STATUSES or late:
        detail, technical = _blocked_reason(task, state, reason, rows, steps) if status in BLOCKED_STATUSES else ("", "")
        wants_note = status == "blocked" and detail.startswith(ask_words.NOTHING_NEW_LEAD)
        detail = " ".join(part for part in (late, detail) if part)
        history = state.get("history")
        generation = str(len(history)) if isinstance(history, list) else ""
        key = bound_key(BLOCKED, _revision(str(status), str(state.get("phase")), reason, since or "", generation))
        return [replace(_make(repo, project, task, BLOCKED, key, ask_words.BLOCKED_TITLE, detail, since),
                        technical=technical, findings=_stuck_findings(state), wants_note=wants_note)]
    return []


def _stuck_findings(state: dict) -> tuple[str, ...]:
    """The findings of a final review whose fix the builder did not act on (`final_review.stuck`); none for any other stop."""
    return _review_findings(state, FINAL_REVIEW_GATE) if (state.get(FINAL_REVIEW_KEY) or {}).get("stuck") else ()


def _review_findings(state: dict, gate: str) -> tuple[str, ...]:
    """The findings of the review the person is asked to decide on, in plain words; none at any other gate."""
    if gate != FINAL_REVIEW_GATE:
        return ()
    return ask_words.plain_findings((state.get(FINAL_REVIEW_KEY) or {}).get("findings") or [])


def _approval_words(task: store.TaskStore, state: dict, gate: str, reason: str,
                    cfg: config.FlowConfig) -> tuple[str, str]:
    """The title and the body of the ask at `gate`, in the words of `flow/ask_words.py`."""
    if gate == DEFAULT_GATE:
        return ask_words.SPEC_TITLE, _spec_text(task)
    if gate == checks.APPROVAL_GATE:
        commands = [controls.escaped(str(command)) for command in (state.get(checks.PENDING_KEY) or {}).get("commands") or []]
        return ask_words.checks_title(commands), ask_words.checks_detail(commands)
    if gate == BUDGET_WAIT_GATE:
        record = state.get(BUDGET_WAIT_KEY) or {}
        return ask_words.budget_wait(record, _account_label(record.get("account") or cfg.values["budget"]["account"]))
    if gate == OVERLAP_GATE:
        return ask_words.OVERLAP_TITLE, ask_words.overlap_detail(_overlap_conflicts(task, state))
    if gate == BUDGET_GATE:
        return ask_words.BUDGET_START_TITLE, _reason_clause(reason)
    if gate == FINAL_REVIEW_GATE:
        asked = state.get(FINAL_REVIEW_KEY) or {}
        rounds = int(asked.get("asked_rounds", asked.get("rounds", 0)))  # an ask made before the count was kept says the reviews
        return ask_words.FINAL_REVIEW_TITLE, ask_words.final_review_detail(rounds)
    if gate.startswith(OPUS_GATE_PREFIX):
        return ask_words.OPUS_TITLE, ask_words.OPUS_DETAIL
    return ask_words.OTHER_TITLE, _reason_clause(reason)


LATE_KEY = "late_work"  # {"count", "pushed"} of a stopped or failed lane, {"count", "reason"} of a blocked one (`flow/resumed_work.py`)
NOTE_ONLY_STATUSES = ("stopped", "failed")


def late_sentence(state: dict) -> str:
    """What a stopped or failed lane says of the commits a message to its builder left after it went on, "" when it has none."""
    found = state.get(LATE_KEY)
    if not found or state.get("status") not in NOTE_ONLY_STATUSES:
        return ""
    return ask_words.late_work(found["count"], state["status"], bool(found.get("pushed")))


def gate_title(gate: str) -> str:
    """The title of the ask at `gate` when only the gate is known (an answer kept after its ask went away)."""
    titles = {DEFAULT_GATE: ask_words.SPEC_TITLE, checks.APPROVAL_GATE: ask_words.checks_title([]),
              BUDGET_WAIT_GATE: ask_words.BUDGET_WAIT_TITLE, OVERLAP_GATE: ask_words.OVERLAP_TITLE,
              BUDGET_GATE: ask_words.BUDGET_START_TITLE, FINAL_REVIEW_GATE: ask_words.FINAL_REVIEW_TITLE}
    if gate.startswith(OPUS_GATE_PREFIX):
        return ask_words.OPUS_TITLE
    return titles.get(gate, ask_words.OTHER_TITLE)


def lane_title_of(repo: Path, task: str) -> str:
    """The title the lane `task` of `repo` was given; empty when it has none or its state cannot be read."""
    try:
        title = store.TaskStore(repo, task).read_state().get(tasks.TITLE_KEY)
    except (store.StateError, OSError, ValueError, RuntimeError):  # RuntimeError: a repository or Git that cannot be read
        return ""
    return strip_controls(str(title or ""))


def _account_label(account: str) -> str:
    """The label Leeway shows for `account`, or an empty string when it is not known or Leeway cannot be asked."""
    from sakura.budget import leeway_numbers
    from sakura.usage import leeway

    if not account:
        return ""
    try:
        row = leeway_numbers.row_of(leeway.status()["accounts"], account)
    except leeway.LeewayError:
        return ""
    return strip_controls(str(row["label"])) if row else ""


def _overlap_conflicts(task: store.TaskStore, state: dict) -> list[dict]:
    """The conflicts the overlap approval binds, each lane named by the title it was given (else its name)."""
    found = state.get("overlap") or {}
    conflicts = found.get("conflicts") or [{"lane": found.get("lane", "another lane"), "files": found.get("files")}]
    return [{"lane": _lane_name(task, str(conflict.get("lane", "another lane"))),
             "files": [strip_controls(str(name)) for name in conflict.get("files") or []]} for conflict in conflicts]


def _lane_name(task: store.TaskStore, lane: str) -> str:
    """`lane` as the overlap record names it (`<project>/<task>`): the lane's title when it has one, else as recorded."""
    try:
        other = store.TaskStore(task.repo, lane.rpartition("/")[2], folder_missing=task.folder_missing)
        title = other.read_state().get(tasks.TITLE_KEY)
    except (store.StateError, OSError, ValueError):
        title = None
    return strip_controls(str(title or lane))


def _spec_text(task: store.TaskStore) -> str:
    """The spec the lane wrote, as the person is asked to read it before approving; a sentence when it cannot be shown."""
    try:
        return strip_controls(beneath.read_capped(task.root, task.spec.name, SPEC_DETAIL_BYTES).decode(errors="replace"))
    except OSError:
        return SPEC_NOT_SHOWN


def _reason_clause(reason: str) -> str:
    """What the engine's waiting line says in parentheses, without the file it names to read first: the line itself is
    a command and a host path, which a person approving in a window has no use for."""
    start, end = reason.find("("), reason.rfind(")")
    parts = reason[start + 1:end].split("; ") if 0 <= start < end else []
    return "; ".join(part for part in parts if part and not READ_CLAUSE.fullmatch(part))


def _blocked_reason(task: store.TaskStore, state: dict, reason: str, rows: list[contract.Row],
                    steps: config.PresetSteps) -> tuple[str, str]:
    """The engine's reason in a person's words, and the engine's own line apart (`ask_words.blocked_words`); while it waits
    for owner rows, how many are still open, as they are now."""
    conflict = state.get("merge_conflict")
    if conflict and reason.startswith("merging "):
        return ask_words.merge_conflict(conflict["titles"], conflict["files"]), reason
    if not owner_marks.is_blocked_on_owner_rows(state):
        return ask_words.blocked_words(reason)
    still_open = owner_marks.open_rows(rows, steps, state)
    return (ask_words.owner_rows_detail(len(still_open)) if still_open else ask_words.ALL_TRIED_DETAIL), ""


def _owner_waits(repo: Path, project: str, task: store.TaskStore, state: dict, rows: list[contract.Row],
                 steps: config.PresetSteps) -> list[Wait]:
    if state.get("phase") not in OWNER_ROW_PHASES:
        return []
    workspace_head = "" if task.folder_missing else owner_marks.workspace_head(state)
    engine_head = final_head.recorded(state)
    return [replace(_make(repo, project, task, OWNER_ROW, bound_key(row.id, owner_revision(state, row.id)),
                          ask_words.owner_row_title(strip_controls(row.flow)), strip_controls(row.flow), _since(state)),
                    workspace_head=workspace_head, engine_head=engine_head)
            for row in owner_marks.open_rows(rows, steps, state)]


def _question_waits(repo: Path, project: str, task: store.TaskStore, state: dict) -> list[Wait]:
    """The questions the lane's roles asked that the person has not answered; a finished lane waits for nobody. A
    question log that cannot be read whole is shown as blocked, never read shortened."""
    if state.get("status") in FINISHED_STATUSES:
        return []
    try:
        found = lane_questions.open_questions(task)
    except lane_questions.QuestionsBroken as error:
        log_read_problem(error)
        return [_make(repo, project, task, BLOCKED, QUESTIONS_KEY, ask_words.BLOCKED_TITLE,
                      "The lane's questions could not be read. Check the host logs.", _since(state))]
    return [_make_question(repo, project, task, entry) for entry in found]


def _make_question(repo: Path, project: str, task: store.TaskStore, entry: dict) -> Wait:
    text = strip_controls(str(entry["text"]))
    first = " ".join(text.split("\n", 1)[0].split())[:QUESTION_TITLE_CHARACTERS]
    identity = json.dumps(["wait", str(repo), task.task_id, QUESTION, bound_key(entry["id"], entry["digest"])],
                          separators=(",", ":"))
    return Wait(identity, QUESTION, str(repo), project, task.task_id, first, text, entry.get("asked_at"),
                author=f"flow:{strip_controls(str(entry.get('role', '')))}",
                choices=tuple(strip_controls(str(choice)) for choice in entry.get("choices") or []))


def _proposal_waits(repo: Path, *, folder_missing: bool) -> list[Wait]:
    """The proposals an agent made for a knowledge folder of this project that the person has not decided."""
    found = proposals.pending(engine_home.root(), repo)
    if not found:
        return []
    try:
        project = owner_report.project(config.load(repo, folder_missing=folder_missing).values, repo.name)
    except config.FlowConfigError:
        project = repo.name
    return [_make_proposal(repo, project, proposal) for proposal in found]


def _make_proposal(repo: Path, project: str, proposal: proposals.Proposal) -> Wait:
    title = (f"Change {proposal.path} in the {proposal.folder} knowledge folder" if proposal.base_digest
             else f"Add {proposal.path} to the {proposal.folder} knowledge folder")
    # The text is shown whole: `proposals.MAX_TEXT` is what a proposal may hold, and the key names its digest.
    detail = f"Reason: {strip_controls(proposal.reason)}\n\n{strip_controls(proposal.text)}"
    identity = json.dumps(["wait", str(repo), proposal.task, PROPOSAL, bound_key(proposal.id, proposal.digest)],
                          separators=(",", ":"))
    return Wait(identity, PROPOSAL, str(repo), project, proposal.task, title, detail, proposal.created_at,
                author=f"flow:{strip_controls(proposal.role)}")


def _automation_waits() -> list[Wait]:
    """The automations the master proposed that the person has not decided: host-wide, so no project and no task."""
    from sakura.automations import registry as automations

    try:
        pending = automations.pending()
    except automations.AutomationRefused:
        return []
    return [Wait(json.dumps(["wait", AUTOMATIONS_ROOT, found.name, AUTOMATION, bound_key(found.name, found.digest)],
                            separators=(",", ":")), AUTOMATION, AUTOMATIONS_ROOT, AUTOMATIONS_PROJECT, found.name,
                 f"Run {found.verb} {found.schedule}: {strip_controls(found.name)}",
                 f"{strip_controls(found.description)}\n\nSchedule: {found.schedule}\nRuns: sakura {found.verb}\n"
                 f"Proposed by {strip_controls(found.author)}. Only this engine verb can run; nothing is installed until you accept.",
                 found.proposed_at or None, author=strip_controls(found.author)) for found in pending]


def _rows(task: store.TaskStore, state: dict) -> list[contract.Row]:
    """The contract's rows: the bound contract's, else the candidate that waits for the person's approval of its commands
    (the state binds a contract only once that is given), so the approval counts the rows the lane's progress counts."""
    digest = state.get("contract_sha256") or (state.get(checks.PENDING_KEY) or {}).get("contract")
    if not digest:
        return []
    return contract.load(task, digest)


def _make(repo: Path, project: str, task: store.TaskStore, kind: str, key: str, title: str, detail: str,
          since: str | None) -> Wait:
    identity = json.dumps(["wait", str(repo), task.task_id, kind, key], separators=(",", ":"))
    return Wait(identity, kind, str(repo), project, task.task_id, title, detail, since)


def _since(state: dict) -> str | None:
    history = state.get("history")
    last = history[-1] if isinstance(history, list) and history else None
    at = last.get("at") if isinstance(last, dict) else None
    return at if isinstance(at, str) and at else None
