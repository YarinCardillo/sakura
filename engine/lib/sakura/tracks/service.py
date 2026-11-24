"""Track authorization and transactions shared by the engine's entry points."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import sqlite3
import stat
from collections import defaultdict
from collections.abc import Callable, Iterator
from contextlib import closing, contextmanager, nullcontext
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING
from uuid import uuid4

from sakura import actors, engine_home, repository
from sakura.flow import lane_ceiling, registry, store, tasks
from sakura.flow.render import strip_controls
from sakura.tracks.database import PRIVATE_FILE_MODE, Database, name_key
from sakura.tracks.domain import (
    ANSWER_LIMIT,
    CONVERSATION_PART_LIMIT,
    DECISION_LIMIT,
    GOAL_LIMIT,
    NAME_LIMIT,
    OPERATION_LIMIT,
    SUMMARY_LIMIT,
    TITLE_LIMIT,
    AuditEntry,
    ConversationKey,
    ConversationLink,
    Decision,
    LaneLink,
    LaneState,
    ProjectLink,
    Refusal,
    RefusalCode,
    Track,
    TrackStatus,
)

if TYPE_CHECKING:
    from sakura.tracks.starts import LaunchInputs, Reservation

PLACES = {"app": "desktop", "overlay": "phone", "cli": "cli"}
ConnectionFactory = Callable[[bool], sqlite3.Connection | None]


@dataclass(frozen=True)
class LaneRead:
    """Verified lifecycle, optional live title and engine opener for one lane."""

    state: LaneState
    title: str | None = None
    opener: actors.Actor | None = None
    start_marker: str | None = None
    is_pending: bool = False


LaneReader = Callable[[ProjectLink, str], LaneRead]


def label(text: str, fallback: str) -> str:
    """Return one safe display line for a project, lane or conversation."""
    clean = strip_controls(text).strip().splitlines() if isinstance(text, str) else []
    return clean[0][:TITLE_LIMIT] if clean else fallback


def resolve_project(folder: Path) -> ProjectLink:
    """Canonicalize a repository or recover the identity of its saved missing path."""
    try:
        folder = folder.resolve()
        info = folder.stat()
    except FileNotFoundError:
        return ProjectLink(engine_home.project_id_for_path(folder), str(folder), label(folder.name, "Project"))
    except OSError:
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The project folder cannot be read") from None
    try:
        if not stat.S_ISDIR(info.st_mode):
            raise repository.RepositoryError("Not a directory")
        common = repository.common_dir(folder)
        canonical = engine_home.main_worktree(common)
        return ProjectLink(engine_home.project_id_for(common), str(canonical), label(canonical.name, "Project"))
    except repository.RepositoryError:
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The project folder cannot be verified") from None


def read_lane(project: ProjectLink, task: str) -> LaneRead:
    """Inspect saved task state even when the canonical Git folder has disappeared."""
    try:
        folder = Path(project.folder)
        if resolve_project(folder) != project:
            return LaneRead(LaneState.UNREADABLE)
        task_store = store.TaskStore(folder, task, folder_missing=_folder_missing(folder))
        expected = engine_home.root() / project.id / engine_home.FLOW_DIR_NAME / task
        if task_store.root != expected or not _verified_task_folder(expected):
            return LaneRead(LaneState.GONE if task_store.root == expected else LaneState.UNREADABLE)
        state = task_store.read_state()
        lane = lane_from_state(state)
        if lane.state == LaneState.UNREADABLE:
            return lane
        try:
            opener = lane_ceiling.opener(task_store)
        except UnicodeError:
            opener = None
        return LaneRead(lane.state, lane.title, opener, lane.start_marker, lane.is_pending)
    except (OSError, ValueError, store.StateError, registry.RegistryError, repository.RepositoryError,
            engine_home.EngineHomeError):
        return LaneRead(LaneState.UNREADABLE)



def lane_from_state(state: dict) -> LaneRead:
    """Interpret authenticated task state without reopening the task."""
    status = state.get("status")
    if not isinstance(status, str) or not status:
        return LaneRead(LaneState.UNREADABLE)
    lifecycle = LaneState.FINISHED if status in lane_ceiling.FINISHED_STATUSES else LaneState.LIVE
    return LaneRead(lifecycle, label(state.get(tasks.TITLE_KEY), "") or None,
                    start_marker=state.get(tasks.TRACK_START_KEY), is_pending=bool(state.get(tasks.PENDING_START_KEY)))

def _folder_missing(folder: Path) -> bool:
    try:
        folder.stat()
    except FileNotFoundError:
        return True
    return False


def _verified_task_folder(path: Path) -> bool:
    for directory in (path.parents[1], path.parent, path):
        try:
            info = directory.lstat()
        except FileNotFoundError:
            return False
        if not stat.S_ISDIR(info.st_mode):
            raise OSError("Saved lane path is ambiguous")
    return True


@contextmanager
def lifecycle_guard() -> Iterator[None]:
    """Serialize close with a finished lane's transition back to live."""
    path = engine_home.root() / "tracks-lifecycle.lock"
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, PRIVATE_FILE_MODE)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        os.close(fd)


class TrackService:
    """Manage Track records, with all changes and replay results committed together."""

    def __init__(self, connect: ConnectionFactory | None = None, clock: Callable[[], datetime] | None = None,
                 lane_reader: LaneReader = read_lane) -> None:
        self._connect = connect if connect is not None else Database().connect
        self._clock = clock if clock is not None else lambda: datetime.now(UTC)
        self._lane_reader = lane_reader

    def create(self, name: str, goal: str, *, actor: actors.Actor, op_id: str,
               project: Path | None = None) -> Track:
        """Create an open Track, optionally with an initial project and no lanes."""
        given_name = _text("name", name, NAME_LIMIT)
        name, goal = given_name.strip(), _text("goal", goal, GOAL_LIMIT)
        folder = project.absolute() if project is not None else None

        def apply(change):
            initial = resolve_project(folder) if folder is not None else None
            if initial is not None and _folder_missing(Path(initial.folder)):
                raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The project folder is missing")
            track = Track("t-" + uuid4().hex[:8], name, goal, "open", "", change.at, change.at)
            _require_name(change.db, track.name)
            change.db.execute("INSERT INTO tracks VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                              (track.id, name, name_key(name), goal, track.status, "", change.at, change.at))
            if initial is not None:
                _add_project(change.db, track.id, initial)
            result = {**asdict(track), "projects": [asdict(initial)] if initial else []}
            change.audit(track.id, {}, result)
            return result

        result = self._mutate("create", (given_name, goal, str(folder) if folder is not None else None), actor, op_id, apply,
                              master=True)
        return _track(result)

    def update(self, track_id: str, *, actor: actors.Actor, op_id: str, name: str | None = None,
               goal: str | None = None, summary: str | None = None, status: TrackStatus | None = None) -> Track:
        """Apply only supplied fields. Close checks the current membership inside the transaction."""
        edits = _edits(name, goal, summary, status)
        master = name is None and status != "closed"

        def apply(change):
            track = _get_track(change.db, track_id)
            next_status = edits.get("status", track.status)
            if change.actor_class == "master" and track.status == "closed":
                raise Refusal(RefusalCode.FORBIDDEN, "Master cannot edit a closed Track")
            if next_status == "closed" and track.status != "closed":
                self._check_close(change.db, track_id)
            if next_status != "closed":
                _require_name(change.db, edits.get("name", track.name), track_id)
            current = asdict(track)
            changed = {key: value for key, value in edits.items() if current[key] != value}
            if changed:
                _write_track(change, track_id, current, changed)
            return current

        guard = lifecycle_guard() if status == "closed" else nullcontext()
        with guard:
            result = self._mutate("update", (track_id, name, goal, summary, status), actor, op_id, apply, master=master)
        return _track(result)

    def add_decision(self, track_id: str, text: str, *, actor: actors.Actor, op_id: str) -> Decision:
        """Append an open decision with the next number in this Track."""
        text = _text("decision text", text, DECISION_LIMIT)

        def apply(change):
            _require_open(_get_track(change.db, track_id))
            number = change.db.execute("SELECT COALESCE(MAX(number), 0) + 1 FROM track_decisions WHERE track_id = ?",
                                       (track_id,)).fetchone()[0]
            new = asdict(Decision(track_id, number, text, "open", None, change.at, None))
            change.db.execute("INSERT INTO track_decisions VALUES (:track_id, :number, :text, :state, :answer, "
                              ":created_at, :settled_at)", new)
            change.audit(track_id, {}, new)
            return new

        return Decision(**self._mutate("add_decision", (track_id, text), actor, op_id, apply, master=True))

    def settle_decision(self, track_id: str, number: int, answer: str, *, actor: actors.Actor, op_id: str,
                        replay_only: bool = False) -> Decision:
        """Record the person's first answer, including on a closed Track."""
        answer = _text("answer", answer, ANSWER_LIMIT)
        if type(number) is not int or not 0 < number < 2**63:
            raise Refusal(RefusalCode.INVALID_INPUT, "A decision number must be a positive integer")

        def apply(change):
            row = change.db.execute("SELECT * FROM track_decisions WHERE track_id = ? AND number = ?",
                                    (track_id, number)).fetchone()
            if row is None:
                raise Refusal(RefusalCode.NOT_FOUND, "The decision was not found")
            if row["state"] != "open":
                raise Refusal(RefusalCode.DECISION_SETTLED, "The decision is already settled")
            if replay_only:
                raise Refusal(RefusalCode.NOT_FOUND, "The decision wait is unavailable")
            change.db.execute("UPDATE track_decisions SET state = 'settled', answer = ?, settled_at = ? "
                              "WHERE track_id = ? AND number = ?", (answer, change.at, track_id, number))
            fields = {"state": "settled", "answer": answer, "settled_at": change.at}
            change.audit(track_id, {"number": number, **{key: row[key] for key in fields}}, {"number": number, **fields})
            return {**dict(row), **fields}

        return Decision(**self._mutate("settle_decision", (track_id, number, answer), actor, op_id, apply))

    def link_lane(self, track_id: str, folder: Path, task: str, *, actor: actors.Actor, op_id: str) -> LaneLink:
        """Attach or move a saved lane. Master must be its recorded opener."""
        folder = folder.absolute()
        _task_id(task)

        def apply(change):
            _require_open(_get_track(change.db, track_id))
            project = resolve_project(folder)
            observed = self._lane_reader(project, task)
            if change.actor_class == "master" and observed.opener != actor:
                raise Refusal(RefusalCode.FORBIDDEN, "Master can link only a lane opened in this session")
            if observed.state not in (LaneState.LIVE, LaneState.FINISHED):
                raise Refusal(RefusalCode.LANE_UNAVAILABLE, "The lane cannot be verified")
            return _link_lane(change, track_id, project, task, observed.title or "Lane")

        return LaneLink(**self._mutate("link_lane", (track_id, str(folder), task), actor, op_id, apply, master=True))

    def unlink_lane(self, track_id: str, folder: Path, task: str, *, actor: actors.Actor, op_id: str) -> LaneLink:
        """Remove membership while retaining the Track's project."""
        folder = folder.absolute()
        _task_id(task)

        def apply(change):
            project_id = _saved_project_id(change.db, track_id, folder)
            row = _lane_row(change.db, project_id, task)
            _require_membership(row, track_id)
            _require_no_pending_start(change.db, row["start_op"])
            change.db.execute("DELETE FROM track_lanes WHERE project_id = ? AND task = ?", (project_id, task))
            change.audit(track_id, dict(row), {})
            return dict(row)

        return LaneLink(**self._mutate("unlink_lane", (track_id, str(folder), task), actor, op_id, apply))

    def link_conversation(self, track_id: str, key: ConversationKey, title: str, *,
                          actor: actors.Actor, op_id: str) -> ConversationLink:
        """Attach or move a started Master conversation by its persisted compound key."""
        identity = _conversation_id(key)
        given_title = _text("title", title, TITLE_LIMIT)
        title = label(given_title, "Conversation")

        def apply(change):
            _require_open(_get_track(change.db, track_id))
            old = _conversation_row(change.db, identity)
            new = dict(zip(("provider", "account", "session", "track_id", "title"), (*identity, track_id, title)))
            change.db.execute("INSERT INTO track_conversations VALUES (?, ?, ?, ?, ?) "
                              "ON CONFLICT(provider, account, session) DO UPDATE "
                              "SET track_id = excluded.track_id, title = excluded.title", (*identity, track_id, title))
            change.audit_move(track_id, old, new)
            return new

        return _conversation(self._mutate("link_conversation", (track_id, identity, given_title), actor, op_id, apply))

    def unlink_conversation(self, track_id: str, key: ConversationKey, *,
                            actor: actors.Actor, op_id: str) -> ConversationLink:
        """Remove the conversation's link from this Track."""
        identity = _conversation_id(key)

        def apply(change):
            row = _conversation_row(change.db, identity)
            _require_membership(row, track_id)
            change.db.execute("DELETE FROM track_conversations WHERE provider = ? AND account = ? AND session = ?",
                              identity)
            change.audit(track_id, dict(row), {})
            return dict(row)

        return _conversation(self._mutate("unlink_conversation", (track_id, identity), actor, op_id, apply))

    def read_all(self) -> tuple[Track, ...]:
        """Read all Tracks and membership from one database snapshot."""
        with _transaction(self._connect, write=False) as db:
            return () if db is None else _read_tracks(db)

    def reserve_start(self, track_id: str, project_id: str, launch_inputs: LaunchInputs,
                      actor: actors.Actor, op_id: str) -> Reservation:
        """Commit the reserved lane identity and its membership before launch."""
        from sakura.tracks.starts import TrackStarts

        return TrackStarts(self).reserve(track_id, project_id, launch_inputs, actor, op_id)

    def read_audit(self, track_id: str) -> tuple[AuditEntry, ...]:
        """Return the Track's attributed changes in commit order."""
        with _transaction(self._connect, write=False) as db:
            if db is None:
                return ()
            return tuple(AuditEntry(**{**dict(row), "before": json.loads(row["before"]), "after": json.loads(row["after"])})
                         for row in db.execute("SELECT * FROM track_audit WHERE track_id = ? ORDER BY seq", (track_id,)))

    def _check_close(self, db, track_id):
        rows = db.execute("SELECT l.*, p.folder, p.name, s.state AS start_state FROM track_lanes l JOIN track_projects p "
                          "USING (track_id, project_id) LEFT JOIN track_starts s ON s.op_id = l.start_op "
                          "WHERE l.track_id = ? ORDER BY l.project_id, l.task", (track_id,))
        blocked = []
        for row in rows:
            project = ProjectLink(row["project_id"], row["folder"], row["name"])
            if row["start_state"] in ("pending", "interrupted"):
                blocked.append(row["title"])
                continue
            lane = self._lane_reader(project, row["task"])
            if lane.state in (LaneState.LIVE, LaneState.UNREADABLE):
                blocked.append(lane.title or row["title"])
        if blocked:
            raise Refusal(RefusalCode.OPEN_LANES, "Finish or remove these linked lanes before closing: " +
                          ", ".join(blocked), tuple(blocked))

    def _mutate(self, operation, inputs, actor, op_id, apply, *, master=False):
        actor_class, scope = _actor_scope(actor, master)
        _text("operation id", op_id, OPERATION_LIMIT)
        fingerprint = hashlib.sha256(_json((operation, inputs)).encode()).hexdigest()
        with _transaction(self._connect, write=True) as db:
            result = _replay(db, op_id, scope, fingerprint)
            if result is not None:
                return result
            at = self._clock().isoformat()
            result = apply(_Change(db, actor, actor_class, operation, op_id, at))
            db.execute("INSERT INTO track_operations VALUES (?, ?, ?, ?, ?)",
                       (op_id, scope, fingerprint, _json(result), at))
            return result


@dataclass(frozen=True)
class _Change:
    db: sqlite3.Connection
    actor: actors.Actor
    actor_class: str
    operation: str
    op_id: str
    at: str

    def audit(self, track_id, before, after):
        self.db.execute("UPDATE tracks SET updated_at = ? WHERE id = ?", (self.at, track_id))
        self.db.execute("INSERT INTO track_audit (track_id, at, actor, actor_class, actor_session, place, operation, "
                        "op_id, before, after) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        (track_id, self.at, actors.format(self.actor), self.actor_class,
                         actors.format(self.actor) if self.actor_class == "master" else None,
                         PLACES.get(self.actor.source), self.operation, self.op_id, _json(before), _json(after)))

    def audit_move(self, track_id, prior, new):
        if prior is not None and prior["track_id"] != track_id:
            self.audit(prior["track_id"], dict(prior), {})
            prior = None
        before = dict(prior) if prior is not None else {}
        if before != new:
            self.audit(track_id, before, new)


@contextmanager
def _transaction(connect: ConnectionFactory, *, write: bool) -> Iterator[sqlite3.Connection | None]:
    db = connect(write)
    if db is None:
        yield None
        return
    with closing(db), db:
        db.execute("BEGIN IMMEDIATE" if write else "BEGIN")
        yield db


def _actor_scope(actor, master):
    if actor.source in actors.PERSON_SOURCES:
        return "person", "person"
    if master and actor.source == actors.MASTER_SOURCE and actor.login:
        return "master", actors.format(actor)
    raise Refusal(RefusalCode.FORBIDDEN, "This actor cannot perform the Track action")


def _replay(db, op_id, scope, fingerprint):
    replay = db.execute("SELECT * FROM track_operations WHERE op_id = ?", (op_id,)).fetchone()
    if replay is None:
        return None
    if replay["actor_scope"] != scope:
        raise Refusal(RefusalCode.FORBIDDEN, "This actor cannot replay the Track action")
    if replay["fingerprint"] != fingerprint:
        raise Refusal(RefusalCode.OPERATION_CONFLICT, "This operation already has different inputs")
    return json.loads(replay["result"])


def _json(data):
    return json.dumps(data, ensure_ascii=True, sort_keys=True, separators=(",", ":"))


def _text(field, text, limit, *, blank=False):
    if (not isinstance(text, str) or len(text) > limit or "\0" in text
            or any(0xD800 <= ord(char) <= 0xDFFF for char in text) or (not blank and not text.strip())):
        raise Refusal(RefusalCode.INVALID_INPUT, f"Invalid {field}. Use up to {limit} characters")
    return text


def _task_id(task):
    if not isinstance(task, str) or not store.TASK_ID.fullmatch(task):
        raise Refusal(RefusalCode.INVALID_INPUT, "Invalid lane identity")


def _conversation_id(key):
    return tuple(_text(field, getattr(key, field), CONVERSATION_PART_LIMIT) for field in ("provider", "account", "session"))


def _edits(name, goal, summary, status):
    edits = {}
    if name is not None:
        edits["name"] = _text("name", name, NAME_LIMIT).strip()
    if goal is not None:
        edits["goal"] = _text("goal", goal, GOAL_LIMIT)
    if summary is not None:
        edits["summary"] = _text("summary", summary, SUMMARY_LIMIT, blank=True)
    if status is not None:
        if status not in ("open", "paused", "closed"):
            raise Refusal(RefusalCode.INVALID_INPUT, "Choose open, paused or closed")
        edits["status"] = status
    return edits


def _require_name(db, name, except_id=""):
    if db.execute("SELECT 1 FROM tracks WHERE name_key = ? AND status <> 'closed' AND id <> ?",
                  (name_key(name), except_id)).fetchone():
        raise Refusal(RefusalCode.NAME_IN_USE, "An open or paused Track already uses this name")


def _get_track(db, track_id):
    row = db.execute("SELECT * FROM tracks WHERE id = ?", (track_id,)).fetchone()
    if row is None:
        raise Refusal(RefusalCode.NOT_FOUND, "The Track was not found")
    return _track(dict(row))


def _require_open(track):
    if track.status == "closed":
        raise Refusal(RefusalCode.CLOSED, "Reopen this Track first")


def _write_track(change, track_id, current, changed):
    before = {key: current[key] for key in changed}
    current.update(changed, updated_at=change.at)
    change.db.execute("UPDATE tracks SET name = :name, name_key = :name_key, goal = :goal, status = :status, "
                      "summary = :summary WHERE id = :id", {**current, "name_key": name_key(current["name"])})
    change.audit(track_id, before, changed)


def _add_project(db, track_id, project):
    db.execute("INSERT INTO track_projects VALUES (?, ?, ?, ?) ON CONFLICT(track_id, project_id) DO NOTHING",
               (track_id, project.id, project.folder, project.name))


def _saved_project_id(db, track_id, folder):
    row = db.execute("SELECT project_id FROM track_projects WHERE track_id = ? AND folder = ?",
                     (track_id, str(folder))).fetchone()
    return row["project_id"] if row is not None else resolve_project(folder).id


def _lane_row(db, project_id, task):
    return db.execute("SELECT * FROM track_lanes WHERE project_id = ? AND task = ?", (project_id, task)).fetchone()


def _link_lane(change, track_id, project, task, title):
    old = _lane_row(change.db, project.id, task)
    start_op = old["start_op"] if old else None
    _require_no_pending_start(change.db, start_op)
    _add_project(change.db, track_id, project)
    link = LaneLink(track_id, project.id, task, title, start_op)
    change.db.execute("INSERT INTO track_lanes VALUES (:project_id, :task, :track_id, :title, :start_op) "
                      "ON CONFLICT(project_id, task) DO UPDATE SET track_id = excluded.track_id, title = excluded.title",
                      asdict(link))
    change.audit_move(track_id, old, asdict(link))
    return asdict(link)


def _require_no_pending_start(db, start_op):
    if start_op is not None and db.execute("SELECT 1 FROM track_starts WHERE op_id = ? "
                                           "AND state IN ('pending', 'interrupted')", (start_op,)).fetchone():
        raise Refusal(RefusalCode.LANE_UNAVAILABLE, "Resolve the lane start before changing its link")


def _require_membership(row, track_id):
    if row is None or row["track_id"] != track_id:
        raise Refusal(RefusalCode.NOT_FOUND, "This link no longer belongs to the Track")


def _conversation_row(db, identity):
    return db.execute("SELECT * FROM track_conversations WHERE provider = ? AND account = ? AND session = ?",
                      identity).fetchone()


def _track(row):
    fields = {key: row[key] for key in ("id", "name", "goal", "status", "summary", "created_at", "updated_at")}
    projects = tuple(ProjectLink(**project) for project in dict(row).get("projects", ()))
    return Track(**fields, projects=projects)


def _conversation(row):
    return ConversationLink(row["track_id"], ConversationKey(row["provider"], row["account"], row["session"]), row["title"])


def _read_tracks(db):
    projects, lanes, conversations, decisions = (defaultdict(list) for _ in range(4))
    for row in db.execute("SELECT * FROM track_projects ORDER BY project_id"):
        projects[row["track_id"]].append(ProjectLink(row["project_id"], row["folder"], row["name"]))
    for row in db.execute("SELECT * FROM track_lanes ORDER BY project_id, task"):
        lanes[row["track_id"]].append(LaneLink(**dict(row)))
    for row in db.execute("SELECT * FROM track_conversations ORDER BY provider, account, session"):
        conversations[row["track_id"]].append(_conversation(row))
    for row in db.execute("SELECT * FROM track_decisions ORDER BY number"):
        decisions[row["track_id"]].append(Decision(**dict(row)))
    return tuple(Track(**{**dict(row), "projects": tuple(projects[row["id"]]), "lanes": tuple(lanes[row["id"]]),
                          "conversations": tuple(conversations[row["id"]]), "decisions": tuple(decisions[row["id"]])})
                 for row in db.execute("SELECT id, name, goal, status, summary, created_at, updated_at FROM tracks "
                                       "ORDER BY created_at, id"))
