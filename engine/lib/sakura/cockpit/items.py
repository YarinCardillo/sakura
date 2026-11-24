"""The rows of the cockpit view: one per flow task, one per manual lane, one per lane group."""

from __future__ import annotations

import shlex
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from functools import cache
from pathlib import Path

from sakura.cockpit import digest_reading, lane_facts, times
from sakura.flow import ask_words, config, delivery, gitutil, inbox, lane_ceiling, owner_marks, progress, resume, store, tasks, waits
from sakura.lanes.config import LaneGroup
from sakura.outside import inbox as outside_inbox
from sakura.outside import store as outside_store
from sakura.outside import waits as outside_waits
from sakura.outside.records import Lane
from sakura.lanes.status import LaneRow
from sakura.usage.model import Usage

FLOW_STATES = ("gate", "blocked", "working", "waiting")
LIVE_RUN_STATES = ("starting", "running")
BLOCKED_STATUSES = lane_ceiling.BLOCKED_STATUSES
STATUS_CLEANED = "cleaned"
STATUS_STOPPED = "stopped"
STATUS_REPORT_READY = "report_ready"
# A finished lane waits for nobody, so its state is not "waiting" (the state an outside lane has with nothing open).
FINISHED_STATUSES = (STATUS_REPORT_READY, STATUS_CLEANED)
STATUS_AWAITING_APPROVAL = "awaiting_approval"
LAST_COMMIT_FORMAT = "--format=%h %s"
TEXT_PLACEHOLDER = "<text>"
NAME_PLACEHOLDER = "<name>"


@dataclass(frozen=True)
class Item:
    """One flow task or manual lane. `key` is `("flow", repo, task)` or `("lane", group, name)`.

    `pending` lists the queued operator commands, and a problem found while reading the task.
    A blank string means the figure is not known; `usage` None means it is not readable from local logs.
    `since` (when the item entered its state) and `last_activity_at` are UTC ISO strings, None when not known.
    `progress` is the engine's done-over-total of a flow task or outside lane, None for a manual lane.
    `approval_revision` is the revision of the gate a flow task waits at (`waits.approval_revision`), empty otherwise.
    `title`, `author`, `links` (label, https URL) and `unread` belong to an outside lane (`("outside", lane)`) and are
    blank for the others.
    """

    kind: str
    key: tuple
    repo: Path
    worktree: Path | None
    branch: str
    last_commit: str
    stack: str
    agent_state: str
    pr: str
    models: dict[str, str]
    pending: list[str]
    usage: Usage | None
    next_command: str
    since: str | None = None
    last_activity_at: str | None = None
    resume_revision: str = ""
    progress: progress.Progress | None = None
    title: str = ""
    author: str = ""
    links: tuple[tuple[str, str], ...] = ()
    unread: int = 0
    approval_revision: str = ""
    # A flow task's engine status and phase and whether it has a saved contract: what decides which of resume, stop,
    # clean and recontract the engine accepts (`cockpit/actions.py`). Empty for the other kinds, and for a card made
    # without them, which offers what it always offered.
    status: str = ""
    phase: str = ""
    has_contract: bool = False
    # Why resuming a blocked flow would only block again (`flow/resume.retry_futile`), empty when a retry may clear it.
    no_retry: str = ""
    # What the lane delivered (`flow/delivery.py`: kind, head, branch, url), or None.
    delivery: dict | None = None
    # The lane's goal, steps, opener, preset, pace and reviews (`cockpit/lane_facts.py`); empty for a lane that has none.
    facts: dict = field(default_factory=dict)
    folder_missing: bool = False


@dataclass(frozen=True)
class GroupRow:
    """One lane group; the "open lane" action lives here. `problem` is why its lanes could not be read."""

    name: str
    repo: Path
    session: str
    lanes: int
    problem: str
    next_command: str

    @property
    def key(self) -> tuple:
        return ("group", self.name)


def group_row(group: LaneGroup, lanes: int, problem: str) -> GroupRow:
    """Return the row of `group` with `lanes` open lanes."""
    command = f"sakura lane open {NAME_PLACEHOLDER} --group {shlex.quote(group.name)}"
    return GroupRow(group.name, group.repo, group.session, lanes, problem, command)


def lane_item(row: LaneRow, group: LaneGroup, usage: Usage | None) -> Item:
    """Return the item of one lane row."""
    waiting = row.state == "waiting"
    say = (f"sakura lane say {shlex.quote(row.name)} --group {shlex.quote(row.group)} "
           f"{shlex.quote(TEXT_PLACEHOLDER)}")
    models = {row.tool or "agent": row.model} if row.model else {}
    since, last_activity_at = times.lane_times(row, group.repo)
    return Item("lane", ("lane", row.group, row.name), group.repo, Path(row.worktree), row.branch, row.last_commit,
                row.stack, row.state, row.pr, models, [], usage, say if waiting else "", since, last_activity_at)


def outside_items(root: Path) -> list[Item]:
    """One item per open outside lane under the store folder `root`; an unreadable lane is left out."""
    lanes, _ = outside_store.scan(root)
    return [outside_item(lane, outside_store.log(root, lane.id)) for lane in lanes]


def outside_item(lane: Lane, log: list[dict]) -> Item:
    """Build a row from an open outside lane and its history."""
    waiting = bool(outside_waits.of_lane(lane, log))
    done = progress.Progress(outside_inbox.done_rows(lane, log), len(lane.rows), lane.phase)
    activity = log[-1]["at"] if log else lane.updated_at
    reading = digest_reading.of_lane(lane, log)
    kind, title = (digest_reading.KIND_DIGEST, digest_reading.DIGEST_TITLE) if reading else ("outside", lane.title)
    facts = {**lane_facts.outside_facts(lane.title, lane.about), **({"reading": reading, "date": reading["date"]} if reading else {})}
    return Item(kind, ("outside", lane.id), lane.agent.where, None, "", "", "", "waiting" if waiting else "unknown",
                "", {}, [], None, "", lane.opened_at, activity, "", done, title, outside_waits.author_of(lane),
                tuple((link.label, link.url) for link in lane.links), outside_inbox.unread(lane, log), facts=facts)


def flow_items(repo: Path, usage_of) -> list[Item]:
    """Return one item per task of `repo`; `usage_of(key)` gives the item's consumption or None."""
    return flow_items_for_tasks(task_stores(repo), usage_of)


PendingReader = Callable[[store.TaskStore], list[tuple[Path, inbox.Command]]]


def flow_items_for_tasks(tasks: Iterable[store.TaskStore], usage_of, *,
                         read_state: store.ReadState | None = None, read_pending: PendingReader | None = None) -> list[Item]:
    """Build items for one collection, sharing display lookups for the same checkout and ref."""
    log_line = cache(_log_line)
    return [flow_item(task, usage_of, log_line=log_line, read_state=read_state, read_pending=read_pending)
            for task in tasks]


def task_stores(repo: Path) -> list[store.TaskStore]:
    """Return the store of every task directory of `repo` that holds a state file."""
    return store.task_stores(repo)


def _log_line(path: Path, ref: str) -> str:
    try:
        return gitutil.git(path, "log", "-1", LAST_COMMIT_FORMAT, ref, "--").strip()
    except gitutil.GitError:
        return ""


def flow_item(task: store.TaskStore, usage_of, *, log_line=_log_line,
              read_state: store.ReadState | None = None, read_pending: PendingReader | None = None) -> Item:
    """Return the item of one task; a task whose state cannot be read is shown blocked with the reason."""
    key = ("flow", str(task.repo), task.task_id)
    try:
        state = read_state(task) if read_state else task.read_state()
    except store.StateError:
        next_command = "" if task.folder_missing else f"sakura flow status {shlex.quote(task.task_id)}"
        return Item("flow", key, task.repo, None, "", "", "", "blocked", "", {}, [ask_words.STATE_UNREADABLE],
                    usage_of(key), next_command, *times.unknown_times(), folder_missing=task.folder_missing)
    cfg, problems = _config(task, state)
    progressed, trouble = progress.of_task(task, state, cfg)
    worktree = _worktree(state)
    agent_state = "waiting" if _owner_rows_marked(task, state, cfg) else _agent_state(state)
    last_commit = "" if task.folder_missing else _last_commit(task.repo, state, worktree, log_line)
    next_command = "" if task.folder_missing else _next_command(task.task_id, state, agent_state)
    pending = read_pending(task) if read_pending else inbox.pending(task)
    return Item("flow", key, task.repo, worktree, str(state.get("branch") or ""), last_commit,
                "up" if state.get("active_stacks") else "down", agent_state, _pull_request(state),
                dict(cfg.values["roles"]) if cfg else {}, [*_pending(task, pending), *problems, *trouble], usage_of(key),
                next_command, *times.flow_times(task, state),
                resume.revision(state), progressed, approval_revision=_approval_revision(task, state, agent_state),
                status=str(state.get("status") or ""), phase=str(state.get("phase") or ""),
                has_contract=bool(state.get("contract_sha256")), title=str(state.get(tasks.TITLE_KEY) or ""),
                no_retry=resume.retry_futile(state, agent_state == "waiting"), delivery=delivery.of(state),
                facts=lane_facts.lane_facts(task.root, state, cfg.values) if cfg else {}, folder_missing=task.folder_missing)


def _approval_revision(task: store.TaskStore, state: dict, agent_state: str) -> str:
    """The revision of the gate the task waits at, the one `flow approve --revision` checks under the task lock."""
    if agent_state != "gate":
        return ""
    gate = waits.GATE_IN_REASON.search(str(state.get("reason") or ""))
    return waits.approval_revision(task, state, gate[1] if gate else waits.DEFAULT_GATE)


def _owner_rows_marked(task: store.TaskStore, state: dict, cfg: config.FlowConfig | None) -> bool:
    """Whether the task was blocked on owner rows and all are marked now: it stands still until a resume."""
    return cfg is not None and owner_marks.all_marked(task, state, config.steps(cfg.values))


def _agent_state(state: dict) -> str:
    status = state.get("status")
    if status == STATUS_AWAITING_APPROVAL:
        return "gate"
    if status in BLOCKED_STATUSES:
        return lane_ceiling.BLOCKED_WORD
    if status in FINISHED_STATUSES:
        if delivery.of(state):
            return lane_ceiling.DELIVERED_WORD
        return STATUS_CLEANED if status == STATUS_CLEANED else "unknown"
    if status == STATUS_STOPPED:
        return STATUS_STOPPED
    live = any(isinstance(run, dict) and run.get("state") in LIVE_RUN_STATES
               for run in (state.get("runs") or {}).values())
    return "working" if live else "waiting"


def _next_command(task_id: str, state: dict, agent_state: str) -> str:
    name = shlex.quote(task_id)
    status = state.get("status")
    if agent_state == "gate":
        gate = waits.GATE_IN_REASON.search(str(state.get("reason") or ""))
        return f"sakura flow approve {name} {shlex.quote(gate[1]) if gate else waits.DEFAULT_GATE} --continue"
    if agent_state == "working":
        return f"sakura flow attach {name}"
    if status == STATUS_CLEANED:
        return ""
    return f"sakura flow report {name}" if status == STATUS_REPORT_READY else f"sakura flow resume {name}"


def _config(task: store.TaskStore, state: dict) -> tuple[config.FlowConfig | None, list[str]]:
    try:
        return config.load(task.repo, overrides=state.get("overrides") or None, folder_missing=task.folder_missing), []
    except config.FlowConfigError:
        return None, [ask_words.CONFIG_UNREADABLE]


def _worktree(state: dict) -> Path | None:
    path = ((state.get("workspaces") or {}).get("0") or {}).get("path")
    return Path(path) if isinstance(path, str) and path else None


def _last_commit(repo: Path, state: dict, worktree: Path | None, log_line) -> str:
    branch = state.get("branch")
    if worktree and worktree.is_dir():
        return log_line(worktree, "HEAD")
    return log_line(repo, f"refs/heads/{branch}") if isinstance(branch, str) and branch else ""


def _pull_request(state: dict) -> str:
    url = (state.get("pr") or {}).get("url")
    if not isinstance(url, str) or not url:
        return ""
    number = url.rstrip("/").rsplit("/", 1)[-1]
    return f"#{number}" if number.isdigit() else url


def _pending(task: store.TaskStore, commands: list[tuple[Path, inbox.Command]]) -> list[str]:
    return [_pending_line(task, path, command) for path, command in commands]


def _pending_line(task: store.TaskStore, path: Path, command: dict) -> str:
    try:
        registered = inbox.is_registered(task, path)
    except OSError:  # consumed between the listing and the read
        return "unreadable"
    if not registered:
        return "unauthenticated"
    if not inbox.is_well_formed(command):
        return "unreadable"
    if command["kind"] in inbox.STEER_KINDS:
        return f"{command['kind']} ({len(str(command.get('value', '')))} chars)"
    return " ".join(part for part in (command["kind"], command.get("role", ""), command.get("value", "")) if part)
