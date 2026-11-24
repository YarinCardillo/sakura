"""Durable resolution and lane reservations for Inbox proposals."""

import hashlib
import json
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path

from sakura import engine_home
from sakura.accounts import write
from sakura.cockpit import decided
from sakura.flow import config, exits, store, tasks, waits
from sakura.outside import store as outside_store
from sakura.outside import waits as outside_waits
from sakura.outside.records import Refused

START, DISMISS = "start", "dismiss"
STARTED, DISMISSED = "started", "dismissed"
DIRECTORY = Path("cockpit") / "proposals"


def operation_id(wait_id: str) -> str:
    """The shared operation and reserved lane id for one proposal revision."""
    return "proposal-" + hashlib.sha256(wait_id.encode()).hexdigest()[:32]


def project_for(wait: waits.Wait, projects: tuple[Path, ...], requested: str = "") -> Path | None:
    """Choose only a registered project, using the wait's project or the sole project by default."""
    if requested:
        return next((project for project in projects if str(project) == requested), None)
    path = Path(wait.repo)
    return path if path in projects else projects[0] if len(projects) == 1 else None


def defaults(wait: waits.Wait, projects: tuple[Path, ...], missing_projects: frozenset[Path] = frozenset()) -> dict:
    """Fields the two clients use to open the proposal's Start a lane form."""
    project = project_for(wait, projects)
    preset, account = None, None
    if wait.finding is not None:
        saved = wait.finding
        return {"op": operation_id(wait.id), "title": wait.title, **saved,
                "project": saved["project"] or (str(project) if project else None)}
    if project is not None:
        try:
            values = config.load(project, folder_missing=project in missing_projects).values
            preset, account = values["preset"], values["budget"]["account"] or None
        except config.FlowConfigError:
            pass
    return {"op": operation_id(wait.id), "project": str(project) if project else None,
            "preset": preset, "account": account, "title": wait.title}


def outcome(wait_id: str) -> dict:
    """Read the reservation or terminal result for this revision, if there is one."""
    return _read(_path(wait_id))


def replay_wait(wait_id: str) -> waits.Wait | None:
    """Recover an already started proposal so either surface can retry its action."""
    entry = outcome(wait_id)
    if entry.get("answer") != STARTED:
        return None
    row = entry["wait"]
    return waits.Wait(row["id"], row["kind"], entry["source_repo"], row["project"], row["task"], row["title"], "",
                      None, source=entry["source"], author=entry["author"])


def start(wait: waits.Wait, op: str, repo: Path, options: list[str], actor: str,
          spawn: Callable[[], tuple[int, str]], created_id: Callable[[str], str]) -> tuple[int, str]:
    """Reserve the launch before spawning and replay the outcome on either surface."""
    target = _path(wait.id)
    with write.locked(target):
        entry = _read(target)
        if entry.get("answer") == STARTED:
            return exits.OK, tasks.CREATED_LANE_PREFIX + entry["lane"]
        if entry.get("answer") == DISMISSED:
            return exits.USAGE, waits.NO_LONGER_WAITING
        if entry and (entry["repo"], entry["options"]) != (str(repo), options):
            return exits.USAGE, "Retry with the original project, title, preset and account."
        try:
            with _current_proposal(wait):
                if not entry:
                    entry = _reserve(target, wait, op, repo, options)
        except Refused:
            return exits.USAGE, waits.NO_LONGER_WAITING
        lane = operation_id(wait.id)
        created = store.TaskStore(repo, lane)
        if created.exists():
            state = created.read_state()
            if state.get(tasks.INBOX_START_KEY) == lane and not state.get(tasks.PENDING_START_KEY):
                _finish(target, entry, wait, STARTED, actor, lane)
                return exits.OK, tasks.CREATED_LANE_PREFIX + lane
        code, output = spawn()
        lane = created_id(output)
        if code == exits.OK and lane == operation_id(wait.id):
            _finish(target, entry, wait, STARTED, actor, lane)
            return code, output
        if code in (exits.USAGE, exits.BUSY) and not store.TaskStore(repo, operation_id(wait.id)).exists():
            target.unlink()
        return (code, output) if code != exits.OK else (exits.FAILED, "Could not verify the created lane. Retry start.")


def recover_start(wait_id: str, actor: str) -> bool:
    """Resolve a reserved start whose lane was durably created before the receipt."""
    path = _path(wait_id)
    with write.locked(path):
        entry = _read(path)
        if not entry or entry.get("answer"):
            return entry.get("answer") == STARTED
        lane = operation_id(wait_id)
        task = store.TaskStore(Path(entry["repo"]), lane)
        if not task.exists():
            return False
        state = task.read_state()
        if state.get(tasks.INBOX_START_KEY) != lane or state.get(tasks.PENDING_START_KEY):
            return False
        row = entry["wait"]
        wait = waits.Wait(wait_id, row["kind"], entry["source_repo"], row["project"], row["task"], row["title"], "",
                          None, source=entry["source"], author=entry["author"])
        _finish(path, entry, wait, STARTED, actor, lane)
        return True


def dismiss(wait: waits.Wait, actor: str) -> tuple[int, str]:
    """Resolve the proposal without starting a lane."""
    target = _path(wait.id)
    with write.locked(target):
        if _read(target):
            return exits.USAGE, "This proposal already has a start or dismissal."
        try:
            with _current_proposal(wait):
                _finish(target, {}, wait, DISMISSED, actor)
        except Refused:
            return exits.USAGE, waits.NO_LONGER_WAITING
    return exits.OK, ""


@contextmanager
def _current_proposal(wait: waits.Wait) -> Iterator[None]:
    if wait.source == "scheduled":
        from sakura.scheduled import findings

        if wait.finding is not None and not findings.is_current(wait):
            raise Refused(waits.NO_LONGER_WAITING)
        yield
        return
    root = outside_store.path()
    with outside_store.locked(root, wait.task):
        _require_open(root, wait)
        yield


def _require_open(root: Path, wait: waits.Wait) -> None:
    from sakura.cockpit import digest_reading

    lane = outside_store.read(root, wait.task)
    outside_store.require_whole_history(root, wait.task)
    current = next((item for item in outside_waits.of_lane(lane, outside_store.log(root, lane.id))
                    if item.id == wait.id), None)
    if current is None or not digest_reading.date_of_wait(current):
        raise Refused(waits.NO_LONGER_WAITING)


def _reserve(path: Path, wait: waits.Wait, op: str, repo: Path, options: list[str]) -> dict:
    entry = {"op": op, "repo": str(repo), "options": options, "wait": decided.wait_fields(wait),
             "source_repo": wait.repo, "source": wait.source, "author": wait.author}
    write.replace_atomically(path, json.dumps(entry))
    return entry


def _finish(path: Path, entry: dict, wait: waits.Wait, answer: str, actor: str, lane: str | None = None) -> None:
    write.replace_atomically(path, json.dumps({**entry, "wait": decided.wait_fields(wait), "answer": answer,
                                               "lane": lane, "actor": actor, "at": datetime.now(UTC).isoformat()}))
    if wait.finding:
        from sakura.scheduled import findings
        from sakura.scheduled import store as scheduled_store

        try:
            findings.resolve(wait)
        except scheduled_store.Refused as error:
            scheduled_store.note_problem("finding_resolution_deferred", error)


def decisions(state_dir: Path, cutoff: datetime) -> Iterator[dict]:
    """Project recent proposal decisions from their terminal reservations."""
    for path in (state_dir / DIRECTORY).glob("proposal-*.json"):
        try:
            entry = _read(path)
            if entry.get("answer") not in (STARTED, DISMISSED) or "at" not in entry:
                continue
            at = datetime.fromisoformat(entry["at"])
            if at >= cutoff:
                yield decided.decision_entry({**entry["wait"], "lane": entry["lane"]}, entry["answer"],
                                             None, None, entry["actor"], at)
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            continue


def _path(wait_id: str) -> Path:
    return engine_home.state_dir() / DIRECTORY / f"{operation_id(wait_id)}.json"


def _read(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return {}
