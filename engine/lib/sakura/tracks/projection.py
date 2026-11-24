"""Read the Track records and project them using the cockpit's authenticated lane facts."""

import json
import sqlite3
from dataclasses import asdict, dataclass, field
from datetime import datetime, timedelta
from pathlib import Path

from sakura import engine_home
from sakura.cockpit import decided, track_actions
from sakura.flow import store, waits
from sakura.flow.render import strip_controls
from sakura.tracks.database import Database, SchemaError
from sakura.tracks.domain import (
    ConversationKey,
    Decision,
    LaneState,
    ProjectLink,
    Track,
)
from sakura.tracks.service import (
    LaneRead,
    _read_tracks,
    _transaction,
    label,
    lane_from_state,
    read_lane,
)
from sakura.tracks.starts import (
    _RESERVATIONS,
    _attempt,
    _recovery_actions,
    _reservation,
)

READ_PROBLEM = "Tracks could not be read. Try again after refreshing."
SETTLEMENTS = """
SELECT track_id, at, actor, place, after FROM track_audit
WHERE operation = 'settle_decision' AND julianday(at) >= julianday(?)
ORDER BY julianday(at) DESC, seq DESC LIMIT ?
"""


@dataclass(frozen=True)
class View:
    """The wire rows and lookup indexes from one consistent Track read."""

    rows: list[dict] = field(default_factory=list)
    lane_refs: dict[tuple[str, str], dict] = field(default_factory=dict)
    conversation_refs: dict[ConversationKey, dict] = field(default_factory=dict)
    choices: list[dict] = field(default_factory=list)
    waits: list[waits.Wait] = field(default_factory=list)
    done: list[dict] = field(default_factory=list)
    problem: str | None = None

    def lane_ref(self, folder: Path, task: str) -> dict | None:
        """Find a lane's Track by its canonical project identity."""
        if not self.lane_refs:
            return None
        return self.lane_refs.get((engine_home.project_id_for_path(folder), task))


def read(now: datetime, tasks: dict, missing: frozenset, flow_items: list, read_state) -> View:
    """Collect Track data without creating a database or requiring an active project."""
    try:
        with _transaction(Database().connect, write=False) as db:
            if db is None:
                return View()
            tracks = _read_tracks(db)
            reservations = {row["op_id"]: _reservation(row) for row in db.execute(
                _RESERVATIONS + " WHERE s.state IN ('pending', 'interrupted')")}
            settlements = _settlements(db, now, tracks)
        return _project(tracks, reservations, settlements, tasks, missing, flow_items, read_state)
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error, SchemaError, engine_home.EngineHomeError):
        return View(problem=READ_PROBLEM)


def _project(tracks, reservations, settlements, tasks, missing, flow_items, read_state):
    view = View(done=settlements)
    ready = {path for path in tasks if path not in missing}
    saved_tasks = {(engine_home.project_id_for_path(folder), task.task_id): task
                   for folder, members in tasks.items() for task in members} if tracks else {}
    item_keys = {item.key for item in flow_items} if tracks else set()
    projects = {project.id: _project_link(project, ready) for track in tracks for project in track.projects}
    for track in tracks:
        ref = _ref(track)
        if track.status != "closed":
            view.choices.append(ref)
        decisions = [_decision(view, track, decision) for decision in track.decisions]
        keys = track_actions.record_keys(track.status, any(row["state"] == "open" for row in decisions))
        lanes = [_lane(link, projects[link.project_id], reservations, saved_tasks, item_keys, read_state, ready)
                 for link in track.lanes]
        view.lane_refs.update(((lane.project_id, lane.task), ref) for lane in track.lanes)
        view.conversation_refs.update((link.key, ref) for link in track.conversations)
        view.rows.append({**ref, "goal": track.goal, "status": track.status, "summary": track.summary,
                          "updated_at": track.updated_at, "projects": [projects[p.id] for p in track.projects],
                          "lanes": lanes, "conversations": [{"key": asdict(link.key), "title": link.title}
                                                            for link in track.conversations],
                          "decisions": decisions, "actions": _actions(keys)})
    return view


def _project_link(project, ready):
    return {**asdict(project), "folder_missing": Path(project.folder) not in ready and not Path(project.folder).is_dir()}


def _read_member(project, task, saved, read_state):
    if saved is None:
        return read_lane(project, task)
    try:
        return lane_from_state(read_state(saved))
    except store.StateError:
        return LaneRead(LaneState.UNREADABLE)


def _lane(link, project, reservations, saved_tasks, item_keys, read_state, ready):
    member = ProjectLink(project["id"], project["folder"], project["name"])
    key = ("flow", member.folder, link.task)
    observed = _read_member(member, link.task, saved_tasks.get((member.id, link.task)), read_state)
    reservation = reservations.get(link.start_op)
    if reservation and reservation.track_id != link.track_id:
        reservation = None
    return {"project_id": member.id, "task": link.task, "key": list(key) if key in item_keys else None,
            "title": observed.title or link.title, "state": reservation.state if reservation else observed.state,
            "folder_missing": project["folder_missing"], "gone": reservation is None and observed.state == LaneState.GONE,
            "did_not_start": reservation is not None, "startop": reservation.op_id if reservation else None,
            "actions": _reservation_actions(reservation, observed, ready) if reservation else []}


def _reservation_actions(saved, observed, ready):
    try:
        with _attempt(saved.project, saved.task, blocking=False, create=False) as held:
            if held:
                keys = _recovery_actions(saved, observed, Path(saved.project.folder) in ready)
                return _actions(track_actions.RECOVERY_KEYS[key] for key in keys)
    except OSError:
        return []
    return []


def _actions(keys):
    return [track_actions.SPECS[key].document(key) for key in keys]


def _ref(track: Track) -> dict:
    return {"id": track.id, "name": label(track.name, "Track")}


def _decision(view, track, decision):
    wait_id = None
    if decision.state == "open":
        wait = decision_wait(track, decision)
        view.waits.append(wait)
        wait_id = wait.id
    return {"id": decision.number, "text": strip_controls(decision.text), "state": decision.state,
            "answer": strip_controls(decision.answer) if decision.answer is not None else None,
            "settled_at": decision.settled_at, "wait_id": wait_id}


def decision_wait(track: Track, decision: Decision) -> waits.Wait:
    """Give the decision one Inbox identity across Track and project changes."""
    ref = _ref(track)
    key = ("wait", waits.TRACK_SOURCE, track.id, waits.TRACK_DECISION, str(decision.number))
    return waits.Wait(json.dumps(key, separators=(",", ":")), waits.TRACK_DECISION, "", ref["name"], "",
                      label(decision.text, "Track decision"), strip_controls(decision.text), decision.created_at,
                      source=waits.TRACK_SOURCE, track=ref)


def _settlements(db, now, tracks):
    cutoff = now - timedelta(days=decided.RETENTION_DAYS)
    subjects = {(track.id, decision.number): (track, decision) for track in tracks for decision in track.decisions
                if decision.state == "settled"}
    if not subjects:
        return []
    entries = []
    for row in db.execute(SETTLEMENTS, (cutoff.isoformat(), decided.MAX_LISTED)):
        track, decision = subjects[row["track_id"], json.loads(row["after"])["number"]]
        wait = decision_wait(track, decision)
        at = datetime.fromisoformat(row["at"])
        entry = decided.decision_entry(decided.wait_fields(wait), "reply", None, decision.answer, row["actor"], at)
        entries.append({**entry, "at": at.isoformat(), "place": row["place"]})
    return list(reversed(entries))
