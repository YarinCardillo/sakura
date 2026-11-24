"""One read-only picture of the machine: flow tasks, outside lanes, mode, load and consumption."""

from __future__ import annotations

import threading
import time
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field, replace
from datetime import datetime
from pathlib import Path

from sakura import engine_home, git_program, loadinfo, repository
from sakura.budget import settings as budget_settings
from sakura.cockpit import accounts as account_rows
from sakura.cockpit import (
    attention,
    decided,
    items,
    project_actions,
    proposal_actions,
    review_waits,
    wait_actions,
)
from sakura.cockpit import events as event_log
from sakura.cockpit.accounts import AccountRow
from sakura.cockpit.items import GroupRow, Item
from sakura.flow import config, gates, gitutil, inbox, store, waits
from sakura.outside import store as outside_store
from sakura.outside import waits as outside_waits
from sakura.scheduled import findings as scheduled_findings
from sakura.tracks import projection
from sakura.usage import aggregate, command
from sakura.usage.model import Usage

GIT_DIR_NAME = ".git"
# What reading one repository can raise; each becomes one problem line for that repository.
_REPOSITORY_ERRORS = (repository.RepositoryError, git_program.GitProgramError, gitutil.GitError,
                      engine_home.EngineHomeError, OSError)

__all__ = ["AccountRow", "CachedSnapshot", "GroupRow", "Item", "Snapshot", "collect"]


@dataclass(frozen=True)
class Snapshot:
    """What the cockpit shows. `mode` is `day` or `night`; `deferred` are the pages held back at night; `waits` is
    every point a task waits on from the person (`flow/waits.py`)."""

    mode: str
    deferred: list[dict]
    load: dict
    usage: aggregate.WeekUsage
    groups: list[GroupRow]
    items: list[Item]
    events: list[dict]
    problems: list[str] = field(default_factory=list)
    accounts: list[AccountRow] = field(default_factory=list)
    waits: list[waits.Wait] = field(default_factory=list)
    # The registered projects by main worktree, including folders that have gone missing.
    projects: list[Path] = field(default_factory=list)
    # The asks the person answered in the last week, newest first, with the answer given (`cockpit/decided.py`).
    decided: list[dict] = field(default_factory=list)
    missing_projects: frozenset[Path] = frozenset()
    _flow_reads: _FlowReads | None = field(default=None, repr=False, compare=False)

    tracks: projection.View | None = None

    def flow_state(self, task: store.TaskStore) -> dict:
        """Read the authenticated state used to build this snapshot."""
        return self._flow_reads.state(task) if self._flow_reads else task.read_state()

    def flow_pending(self, task: store.TaskStore) -> list[tuple[Path, inbox.Command]]:
        """Read the pending commands listed for this snapshot."""
        return self._flow_reads.pending(task) if self._flow_reads else inbox.pending(task)

    @property
    def visible_waits(self) -> list[waits.Wait]:
        """Waits to present in Inbox. Snoozed waits still accept their answers."""
        return [wait for wait in self.waits if not wait.is_snoozed]

    def wait_target(self, wait: waits.Wait) -> wait_actions.WaitTarget:
        """Bind a wait to the registered projects and its lane actions in this snapshot."""
        key = ("flow", wait.repo, wait.task)
        lane = None
        if wait.kind == waits.BLOCKED and wait.source == waits.FLOW_SOURCE:
            lane = next((item for item in self.items if item.key == key), None)
        return wait_actions.WaitTarget(wait, projects=tuple(self.projects), lane=lane,
                                       missing_projects=self.missing_projects)

    def project_target(self, repo: Path) -> project_actions.ProjectTarget:
        """Bind the project's folder availability to its action target."""
        return project_actions.target_of(repo, folder_missing=repo in self.missing_projects)


class CachedSnapshot:
    """Serves one snapshot for a few seconds to reuse the local reads.

    One reader refreshes a stale snapshot while the others wait for it; a failed read is never kept.
    """

    def __init__(self, source: Callable[[], Snapshot], seconds: float, clock: Callable[[], float] = time.monotonic) -> None:
        self._source, self._seconds, self._clock = source, seconds, clock
        self._lock = threading.Lock()
        self._value: Snapshot | None = None
        self._read_at = 0.0

    def __call__(self) -> Snapshot:
        with self._lock:
            if self._value is None or self._clock() - self._read_at >= self._seconds:
                self._value, self._read_at = None, 0.0
                self._value = self._source()
                self._read_at = self._clock()
            return self._value


def collect(repos: Iterable[Path], now: datetime, *, outside_root: Path | None = None) -> Snapshot:
    """Read the cockpit and locally unresolved pull request reviews.

    `repos` are the registered repositories (a repository listed twice, or through two of its worktrees, counts
    once), `now` an aware time. The old tmux lanes are shelved: `groups` stays in the document, always empty.
    Machine state (mode, deferred pages, lane log, load) is read under the home the process sees.
    `outside_root` is the store of outside lanes (default: under the engine home); reading it creates nothing.
    """
    opened = _read_repositories(repos)
    roots = list(opened.tasks)
    tasks = [task for found in opened.tasks.values() for task in found]
    flow_reads = _FlowReads()
    week = aggregate.week([root for root in roots if root not in opened.missing], [], command.codex_sessions(), now,
                          read_state=flow_reads.state)
    flow = items.flow_items_for_tasks(tasks, _usage_lookup(week), read_state=flow_reads.state,
                                     read_pending=flow_reads.pending)
    settings, problems = _gate_config()
    mode, _ = gates.effective_mode(settings, now.astimezone(), gates.default_mode_file())
    accounts, account_problems = account_rows.collect(_global_reserve(settings), now)
    outside, outside_problems = _outside(outside_root)
    outside_waiting = [wait for wait in outside.waits if not proposal_actions.outcome(wait.id).get("answer")]
    flow_waits = waits.collect_stores(opened.tasks, missing_projects=opened.missing, read_state=flow_reads.state)
    flow_waits += review_waits.collect(opened.tasks, read_state=flow_reads.state)
    flow_waits = [replace(wait, folder_missing=True) if Path(wait.repo) in opened.missing else wait for wait in flow_waits]
    track_view = projection.read(now, opened.tasks, opened.missing, flow, flow_reads.state)
    waiting = waits.ordered(flow_waits + outside_waiting + scheduled_findings.pending_waits() + track_view.waits)
    try:
        waiting = attention.apply(waiting, now)
    except (OSError, ValueError):
        problems.append("Inbox read and snooze marks could not be loaded.")
    return Snapshot(mode, gates.deferred_since_last_day(gates.default_deferred_dir(), settings, now), _load(settings),
                    week, [], flow + outside.items,
                    event_log.latest(tasks, outside_histories=outside.histories),
                    opened.problems + problems + account_problems + outside_problems, accounts,
                    waiting, roots,
                    decided.recent(decided.default_state_dir(), now=now, track_entries=track_view.done),
                    opened.missing, flow_reads, track_view)


class _FlowReads:
    def __init__(self) -> None:
        self._states: dict[tuple[Path, str, bool], dict | str] = {}
        self._pending: dict[tuple[Path, str, bool], list[tuple[Path, inbox.Command]]] = {}

    def state(self, task: store.TaskStore) -> dict:
        key = task.repo, task.task_id, task.folder_missing
        if key not in self._states:
            try:
                self._states[key] = task.read_state()
            except store.StateError as error:
                self._states[key] = str(error)
        state = self._states[key]
        if isinstance(state, str):
            raise store.StateError(state)
        return state

    def pending(self, task: store.TaskStore) -> list[tuple[Path, inbox.Command]]:
        key = task.repo, task.task_id, task.folder_missing
        if key not in self._pending:
            self._pending[key] = inbox.pending(task)
        return self._pending[key]


@dataclass(frozen=True)
class _Outside:
    histories: list[outside_store.History]
    items: list[Item]
    waits: list[waits.Wait]


def _outside(outside_root: Path | None) -> tuple[_Outside, list[str]]:
    """The outside lanes' items and waits, and one line for each record or store that cannot be read."""
    try:
        root = outside_root or outside_store.path()
    except engine_home.EngineHomeError as error:
        return _Outside([], [], []), [f"outside lanes not read: {error}"]
    histories, problems = outside_store.open_histories(root)
    rows = [items.outside_item(lane, log) for lane, log in histories]
    waiting = [wait for lane, log in histories for wait in outside_waits.of_lane(lane, log)]
    return _Outside(histories, rows, waiting), problems


@dataclass(frozen=True)
class _Repositories:
    """The registered projects with their tasks, missing project paths and repository problems."""

    tasks: dict[Path, list[store.TaskStore]]
    problems: list[str]
    missing: frozenset[Path]


def _read_repositories(repos: Iterable[Path]) -> _Repositories:
    """Open every registered repository and list its tasks; one that fails is a problem line, never an exception.

    Every repository is trusted before any is read, so the order of the registry does not matter.
    """
    roots, problems = _distinct_repositories(repos)
    missing = frozenset(repo for repo in roots if not repo.is_dir())
    tasks: dict[Path, list[store.TaskStore]] = {}
    for root in roots:
        try:
            tasks[root] = store.task_stores(root, folder_missing=root in missing)
        except _REPOSITORY_ERRORS as error:
            problems.append(f"repository {root} not read: {error}")
            if root in missing:
                tasks[root] = []
    return _Repositories(tasks, problems, missing)


def _distinct_repositories(repos: Iterable[Path]) -> tuple[list[Path], list[str]]:
    """One path per registered project, in order of first appearance, and a line for each unreadable project.

    A repository registered through a linked worktree is named by its main worktree, so item keys do not
    depend on which worktree was registered first.
    """
    seen: dict[Path, Path] = {}
    problems: list[str] = []
    for repo in repos:
        if not repo.is_dir():
            seen.setdefault(repo, repo)
            continue
        try:
            common = _open(repo)
        except _REPOSITORY_ERRORS as error:
            problems.append(f"repository {repo} not read: {error}")
            continue
        seen.setdefault(common, _main_worktree(common, repo))
    return list(dict.fromkeys(seen.values())), problems


def _open(repo: Path) -> Path:
    """The common Git folder of `repo`, after the engine trusts it (a folder with no repository is never trusted)."""
    git_program.common_dir_of(repo)
    gitutil.trust_repository(repo)
    return repository.common_dir(repo)


def _main_worktree(common_dir: Path, registered: Path) -> Path:
    """The main worktree owning `common_dir`; the registered path when there is none (a bare repository)."""
    main = common_dir.parent
    return main if common_dir.name == GIT_DIR_NAME and main.is_dir() else registered


def _usage_lookup(week: aggregate.WeekUsage):
    totals: dict[tuple, list[Usage]] = {}
    for provider in week.providers:
        for agent in provider.agents:
            totals.setdefault(agent.item, []).append(agent.usage)
    return lambda key: aggregate.sum_usage(totals[key]) if key in totals else None


def _gate_config() -> tuple[config.FlowConfig, list[str]]:
    """Gate settings come only from the personal file, so the home directory stands in for a repository.

    A personal file that does not load gives the defaults and a problem line, never a failed snapshot.
    """
    home = Path.home()
    try:
        return config.load(home), []
    except config.FlowConfigError as error:
        waits.log_read_problem(error)
        return config.defaults(), ["The personal flow config could not be read. Default gate mode is active."]


def _global_reserve(settings: config.FlowConfig) -> float | None:
    """`budget.reserve_week_pct` of the personal config; None when the budget table does not load."""
    try:
        return budget_settings.load(settings.values).reserve_week_pct
    except budget_settings.BudgetSettingsError:
        return None


def _load(settings: config.FlowConfig) -> dict:
    return loadinfo.current_load(settings.values["load"]["slots"])
