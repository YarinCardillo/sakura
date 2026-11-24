"""Durable Track reservations and cross-process flow dispatch."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import stat
import subprocess
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import Literal

from sakura import actors, engine_home, interpreter, text_limits
from sakura.flow import config, exits, pace, presets, registry, store, surface, tasks
from sakura.lanes.input import MAX_TEXT
from sakura.tracks.database import PRIVATE_FILE_MODE, Database
from sakura.tracks.domain import (
    OPERATION_LIMIT,
    LaneLink,
    LaneState,
    ProjectLink,
    Refusal,
    RefusalCode,
)
from sakura.tracks.service import (
    LaneRead,
    TrackService,
    _actor_scope,
    _add_project,
    _Change,
    _folder_missing,
    _get_track,
    _json,
    _lane_row,
    _require_open,
    _text,
    _transaction,
    _verified_task_folder,
    label,
    lifecycle_guard,
    resolve_project,
)

START_TIMEOUT_SECONDS = 30
WAITING = ("pending", "interrupted")
StartState = Literal["pending", "interrupted", "started", "removed"]

_RESERVATIONS = """SELECT s.*, p.folder, p.name FROM track_starts s
                  JOIN track_projects p USING (track_id, project_id)"""


@dataclass(frozen=True)
class LaunchInputs:
    """Frozen options that the ordinary project Start lane path accepts."""

    text: str
    preset: str | None = None
    level: str | None = None
    account: str | None = None

    def options(self) -> list[str]:
        """Build flow options after validation, with each user value bound to its flag."""
        _text("lane text", self.text, MAX_TEXT)
        if problem := text_limits.text_problem(self.text, MAX_TEXT):
            raise Refusal(RefusalCode.INVALID_INPUT, f"Invalid lane text: {problem}")
        options = [f"--message={self.text}"]
        if self.preset is not None:
            if self.preset not in presets.names():
                raise Refusal(RefusalCode.INVALID_INPUT, "Choose a known preset")
            options += [f"--preset={self.preset}", "--relax=preset"]
        if self.level is not None:
            if self.level not in pace.LANE_LEVELS:
                raise Refusal(RefusalCode.INVALID_INPUT, "Choose a valid lane pace")
            options.append(f"--set=knobs.lane_pace={self.level}")
        if self.account is not None:
            if not isinstance(self.account, str) or not config.ACCOUNT_ID.fullmatch(self.account):
                raise Refusal(RefusalCode.INVALID_INPUT, "Choose a valid account")
            options.append(f"--set=budget.account={self.account}")
        return options


@dataclass(frozen=True)
class Reservation:
    """A stable lane identity with the actor and inputs that first reserved it."""

    op_id: str
    track_id: str
    project: ProjectLink
    task: str
    title: str
    inputs: LaunchInputs
    actor: str
    state: StartState


@dataclass(frozen=True)
class PendingStart:
    """One pending or interrupted member and the engine's available recovery actions."""

    reservation: Reservation
    actions: tuple[str, ...]


class TrackStarts:
    """Serialize launch and removal in the process that can materialize the task."""

    def __init__(self, service: TrackService | None = None) -> None:
        self._service = service if service is not None else TrackService()

    def reserve(self, track_id: str, project_id: str, inputs: LaunchInputs,
                actor: actors.Actor, op_id: str) -> Reservation:
        """Reserve membership and launch inputs in the same transaction as the operation."""
        _text("operation id", op_id, OPERATION_LIMIT)
        task = task_id_for(op_id)

        def apply(change):
            inputs.options()
            _require_open(_get_track(change.db, track_id))
            project = _registered_project(project_id)
            _require_folder(project)
            prior = _lane_row(change.db, project_id, task)
            used = change.db.execute("SELECT 1 FROM track_starts WHERE task = ?", (task,)).fetchone()
            if prior or used or self._service._lane_reader(project, task).state != LaneState.GONE:
                raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The reserved lane identity is already in use or unreadable")
            title = label(tasks.split_title(inputs.text)[0], "Lane")
            saved = Reservation(op_id, track_id, project, task, title, inputs, actors.format(actor), "pending")
            _insert(change, saved)
            return asdict(saved)

        result = self._service._mutate("reserve_start", (track_id, project_id, asdict(inputs)), actor, op_id,
                                       apply, master=True)
        return _from_result(result)

    def launch(self, start_op: str, actor: actors.Actor, *, run: Callable = subprocess.run) -> str:
        """Dispatch through normal flow start or return its committed result."""
        saved = self._read("op_id", start_op)
        _require_launcher(saved, actor)
        _require_not_removed(saved)
        if saved.state == "started":
            return saved.task
        _require_folder(saved.project)
        argv = [str(surface.KIT_BIN), "flow", "start", *dispatch_options(saved.task, actors.format(actor))]
        env = {**os.environ, **interpreter.environment(), actors.TRUSTED_ENV: "1"}
        done = run(argv, cwd=saved.project.folder, env=env, capture_output=True, text=True,
                   stdin=subprocess.DEVNULL, timeout=START_TIMEOUT_SECONDS, check=False)
        if done.returncode != exits.OK:
            raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The lane did not start. Retry the reservation")
        if tasks.created_lane_id(done.stdout) != saved.task or self._read("op_id", start_op).state != "started":
            raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The lane start could not be verified")
        return saved.task

    def execute(self, project_id: str, task: str, actor: actors.Actor,
                run: Callable[[Reservation], tuple[int, str]]) -> tuple[int, str]:
        """Run a frozen start inside the launcher process and verify its outcome before answering."""
        saved = self._read("task", task)
        _require_launcher(saved, actor)
        if saved.project.id != project_id:
            raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The reservation belongs to a different project")
        with _attempt(saved.project, task):
            saved = self._read("op_id", saved.op_id)
            _require_not_removed(saved)
            if saved.state == "started":
                return exits.OK, task
            _require_folder(saved.project)
            observed = self._service._lane_reader(saved.project, task)
            _require_recoverable_task(saved, observed)
            if _has_task(observed) and not observed.is_pending:
                self._set_state(saved, "started")
                return exits.OK, task
            self._set_state(saved, "interrupted")
            code, created = run(saved)
            if code != exits.OK:
                return code, ""
            self._verify_created(saved, created)
            self._set_state(replace(saved, state="interrupted"), "started")
            return exits.OK, task

    def retry(self, track_id: str, start_op: str, *, actor: actors.Actor, op_id: str,
              run: Callable = subprocess.run) -> str:
        """Bind a person retry request to a reservation before dispatching its frozen start."""

        def apply(change):
            saved = _get(change.db, "op_id", start_op)
            _require_track(saved, track_id)
            _require_not_removed(saved)
            return {"start_op": saved.op_id}

        self._service._mutate("retry_start", (track_id, start_op), actor, op_id, apply)
        return self.launch(start_op, actor, run=run)

    def remove(self, start_op: str, *, actor: actors.Actor, op_id: str,
               track_id: str | None = None) -> Reservation:
        """Remove a verified absent reservation and retain its terminal operation."""
        _actor_scope(actor, False)
        saved = self._read("op_id", start_op)
        if track_id is not None:
            _require_track(saved, track_id)
        with _attempt(saved.project, saved.task):
            def apply(change):
                current = _get(change.db, "op_id", start_op)
                observed = self._service._lane_reader(current.project, current.task)
                if current.state not in WAITING or observed.state != LaneState.GONE:
                    raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The reservation cannot be removed")
                row = _lane_row(change.db, current.project.id, current.task)
                change.db.execute("DELETE FROM track_lanes WHERE start_op = ?", (start_op,))
                change.db.execute("UPDATE track_starts SET state = 'removed' WHERE op_id = ?", (start_op,))
                change.audit(current.track_id, {"lane": dict(row), "state": current.state}, {"state": "removed"})
                return asdict(replace(current, state="removed"))

            result = self._service._mutate("remove_start", (start_op,), actor, op_id, apply)
        return _from_result(result)

    def read_pending(self) -> tuple[PendingStart, ...]:
        """Read only unresolved reservation rows with current Retry and Remove eligibility."""
        with _transaction(self._service._connect, write=False) as db:
            if db is None:
                return ()
            pending = tuple(_reservation(row) for row in db.execute(
                _RESERVATIONS + " WHERE s.state IN ('pending', 'interrupted') ORDER BY s.op_id"))
        result = []
        for saved in pending:
            with _attempt(saved.project, saved.task, blocking=False) as held:
                if held:
                    current = self._read("op_id", saved.op_id)
                    if current.state not in WAITING:
                        continue
                    result.append(PendingStart(current, self._actions(current)))
                else:
                    result.append(PendingStart(saved, ()))
        return tuple(result)

    def _actions(self, saved: Reservation) -> tuple[str, ...]:
        try:
            _require_folder(saved.project)
        except Refusal:
            can_retry = False
        else:
            can_retry = True
        return _recovery_actions(saved, self._service._lane_reader(saved.project, saved.task), can_retry)

    def _read(self, field: Literal["op_id", "task"], value: str) -> Reservation:
        with _transaction(self._service._connect, write=False) as db:
            return _get(db, field, value)

    def _verify_created(self, saved: Reservation, created: str) -> None:
        observed = self._service._lane_reader(saved.project, saved.task)
        if created != saved.task or not _has_task(observed) or observed.start_marker != saved.task:
            raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The created lane could not be verified")

    def _set_state(self, saved: Reservation, state: StartState) -> None:
        if saved.state == state:
            return
        with _transaction(self._service._connect, write=True) as db:
            db.execute("UPDATE track_starts SET state = ? WHERE op_id = ?", (state, saved.op_id))
            change = _Change(db, actors.Actor("engine", "flow"), "engine", "start_lane", saved.op_id,
                             self._service._clock().isoformat())
            change.audit(saved.track_id, {"state": saved.state}, {"state": state, "task": saved.task})


def task_id_for(op_id: str) -> str:
    """Derive the Track-specific flow id from the caller's operation id."""
    available = store.MAX_TASK_ID_LENGTH - len(tasks.TRACK_START_PREFIX)
    return tasks.TRACK_START_PREFIX + hashlib.sha256(op_id.encode()).hexdigest()[:available]


def dispatch_options(task: str, actor: str | None) -> list[str]:
    """Select a reservation. Only its saved row can supply the launch options."""
    return ["--new", "--detach", f"--inbox-op={task}", *([f"--actor={actor}"] if actor is not None else [])]


@contextmanager
def admit_task_creation(task: store.TaskStore, track_start: bool) -> Iterator[None]:
    """Coordinate fresh flow creation with closing and protect a reserved task id."""
    database = Database()
    with _transaction(lambda write: database.connect(write, create=False), write=True) as db:
        if db is not None:
            _admit_new_task(db, task, track_start)
        yield


def _admit_new_task(db, task, track_start):
    project_id = _engine_project_id(task)
    row = db.execute("SELECT project_id FROM track_starts WHERE task = ?", (task.task_id,)).fetchone()
    if row and not track_start and row["project_id"] == project_id:
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "Retry this lane from its reservation")
    link = _lane_row(db, project_id, task.task_id)
    if link is not None:
        _require_open(_get_track(db, link["track_id"]))


@contextmanager
def admit_reactivation(task: store.TaskStore) -> Iterator[None]:
    """Keep close from observing a finished lane as it returns to live."""
    with lifecycle_guard():
        database = Database()
        with _transaction(database.connect, write=False) as db:
            if db is not None:
                link = _lane_row(db, _engine_project_id(task), task.task_id)
                if link is not None:
                    _require_open(_get_track(db, link["track_id"]))
        yield


def _engine_project_id(task: store.TaskStore) -> str:
    return engine_home.project_id_for_path(task.person_repository)


@contextmanager
def _attempt(project: ProjectLink, task: str, *, blocking: bool = True, create: bool = True) -> Iterator[bool]:
    # Keep lock files after removal so late workers always use the same inode.
    directory = engine_home.root()
    key = hashlib.sha256(f"{project.id}/{task}".encode()).hexdigest()
    flags = os.O_RDWR | os.O_NOFOLLOW | (os.O_CREAT if create else 0)
    try:
        fd = os.open(directory / f"start-{key}.lock", flags, PRIVATE_FILE_MODE)
    except FileNotFoundError:
        if create:
            raise
        yield True
        return
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
        except BlockingIOError:
            yield False
        else:
            yield True
    finally:
        os.close(fd)


def _registered_project(project_id):
    for folder in registry.repos(include_missing=True):
        if engine_home.project_id_for_path(folder) != project_id:
            continue
        project = resolve_project(folder)
        if project.id == project_id:
            return project
    raise Refusal(RefusalCode.NOT_FOUND, "Choose a registered project")


def _require_folder(project):
    folder = Path(project.folder)
    try:
        verified = not _folder_missing(folder) and resolve_project(folder) == project
    except OSError:
        verified = False
    if not verified:
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The project folder is missing or cannot be verified")


def _insert(change, saved):
    _add_project(change.db, saved.track_id, saved.project)
    change.db.execute("INSERT INTO track_starts VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                      (saved.op_id, saved.track_id, saved.project.id, saved.task, saved.title,
                       _json(asdict(saved.inputs)), saved.actor, saved.state))
    link = asdict(LaneLink(saved.track_id, saved.project.id, saved.task, saved.title, saved.op_id))
    change.db.execute("INSERT INTO track_lanes VALUES (:project_id, :task, :track_id, :title, :start_op)", link)
    change.audit(saved.track_id, {}, {"lane": link, "state": saved.state})


def _get(db, field, value):
    sql = " WHERE s.op_id = ?" if field == "op_id" else " WHERE s.task = ?"
    row = db.execute(_RESERVATIONS + sql, (value,)).fetchone() if db is not None else None
    if row is None:
        raise Refusal(RefusalCode.NOT_FOUND, "The lane reservation was not found")
    return _reservation(row)


def _reservation(row):
    project = ProjectLink(row["project_id"], row["folder"], row["name"])
    return Reservation(row["op_id"], row["track_id"], project, row["task"], row["title"],
                       LaunchInputs(**json.loads(row["inputs"])), row["actor"], row["state"])


def _from_result(result):
    return Reservation(**{**result, "project": ProjectLink(**result["project"]),
                          "inputs": LaunchInputs(**result["inputs"])})


def _require_launcher(saved, actor):
    actor_class, scope = _actor_scope(actor, True)
    if actor_class == "master" and scope != saved.actor:
        raise Refusal(RefusalCode.FORBIDDEN, "Master can launch only a reservation from this session")


def _require_track(saved, track_id):
    if saved.track_id != track_id:
        raise Refusal(RefusalCode.NOT_FOUND, "The reservation does not belong to this Track")


def _require_not_removed(saved):
    if saved.state == "removed":
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The lane reservation was removed")


def _has_task(observed: LaneRead) -> bool:
    return observed.state in (LaneState.LIVE, LaneState.FINISHED)


def _require_recoverable_task(saved, observed):
    if _has_task(observed) and observed.start_marker != saved.task:
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The reserved lane identity is already in use")
    if observed.state == LaneState.UNREADABLE and not _before_state_creation(saved):
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The saved lane is unreadable")


def _recovery_actions(saved, observed, can_retry):
    actions = []
    if can_retry:
        try:
            _require_recoverable_task(saved, observed)
        except Refusal:
            pass
        else:
            actions.append("retry")
    if observed.state == LaneState.GONE:
        actions.append("remove")
    return tuple(actions)


def _before_state_creation(saved):
    task = store.TaskStore(Path(saved.project.folder), saved.task)
    try:
        return _verified_task_folder(task.root) and all(
            entry.name in {"lock", task.input.name} and stat.S_ISREG(entry.lstat().st_mode)
            for entry in task.root.iterdir()
        )
    except OSError:
        return False
