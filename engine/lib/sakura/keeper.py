"""The engine keeper: lanes outlive the app and a restart of the Mac.

A lane's runs are processes of this machine. After a restart or a crash their records are still open and nothing
reopens them. The keeper (a LaunchAgent, `keeper_launchd.py`) looks every minute for lanes whose last run record has no
end and whose process is gone, and resumes each one the way the budget wake does (`flow.cli.resume_detached`). It never
resumes a lane the person stopped or one that waits on the person, never starts more lanes than the preset's `lanes`
knob allows (the others are queued for the next look), and a lane it has resumed twice in a few minutes is not resumed a
third time: the keeper marks it blocked, which the person sees as any blocked lane, and says so in the ledger.

The keeper holds only what it did, in `<engine home>/keeper.json`: lane names and times, nothing from a lane. The
master's conversation is not its concern (the app restores it). The phone page can run under the keeper as well.
"""

from __future__ import annotations

import json
import sys
import threading
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

from sakura import engine_home, leeway_serve
from sakura.cockpit import attention, review_waits
from sakura.flow import gates, lane_ceiling, lane_questions, launchers, registry, run_records, store

KEEPER_NAME, PHONE_PAGE_NAME = "keeper", "phone-page"  # as the app lists the always-on services
STATE_FILE = "keeper.json"
FILE_MODE = 0o600
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
LOOP_SECONDS = 60
HEARTBEAT_FRESH = timedelta(minutes=3)
CRASH_WINDOW = timedelta(minutes=3)
CRASH_RESUMES = 2
RESTART_EPISODE = timedelta(minutes=10)
PHONE_PAGE_RETRY = timedelta(minutes=10)
KEEPER_ACTOR = "cli:engine-keeper"
RUNNING, STOPPED = "running", "stopped"
FINISHED = lane_ceiling.FINISHED_STATUSES
CRASH_LOOP_REASON = ("the engine keeper resumed this lane twice within a few minutes and it stopped again; "
                     "read its log, then resume it yourself")
PHONE_PAGE_STOP_SECONDS = 10.0
Resume = Callable[[Path, store.TaskStore], None]
PhonePage = Callable[[threading.Event], None]  # serves the phone page until the event is set


@dataclass(frozen=True)
class KeeperReport:
    """What one look did, as lane names (`<project>/<task>`): resumed, left for the next look because the lane ceiling is
    reached, waiting on the person, and asked about (resumed twice and stopped again)."""

    resumed: tuple[str, ...] = ()
    queued: tuple[str, ...] = ()
    waiting: tuple[str, ...] = ()
    asked: tuple[str, ...] = ()


@dataclass(frozen=True)
class Ports:
    """What the keeper reads and starts, injectable so the tests need no process and no tmux."""

    repos: Callable[[], list[Path]]
    is_alive: Callable[[dict], bool]
    resume: Resume
    lanes: Callable[[], int]
    leeway_serve: leeway_serve.LeewayServe | None = None
    review_refresh: Callable[[], None] | None = None


def real_ports() -> Ports:
    """The keeper's ports on this machine: the registry, the launchers, the detached resume and the `lanes` knob."""
    from sakura.flow import cli, config

    return Ports(repos=registry.repos, is_alive=launchers.entry_is_alive,
                 resume=lambda repo, task: cli.resume_detached(repo, task, KEEPER_ACTOR),
                 lanes=lambda: config.load(Path.home()).values["knobs"]["lanes"],
                 leeway_serve=leeway_serve.LeewayServe(leeway_serve.helper_command),
                 review_refresh=review_waits.Refresher(registry.repos).start)


def state_path() -> Path:
    """Where the keeper keeps what it did."""
    return engine_home.root() / STATE_FILE


# One look

def run_once(now: datetime, ports: Ports | None = None) -> KeeperReport:
    """Look at every lane once and resume what died with a restart; returns what was done and records it.

    Raises OSError or a registry error when the projects cannot be listed; one lane that cannot be read is skipped.
    """
    ports = ports or real_ports()
    held = _read_state()
    scan = _scan(ports)
    done = _act(now, ports, held, scan)
    report = KeeperReport(done.resumed, done.queued, tuple(sorted(scan.waiting)), done.asked)
    _remember(now, held, report, scan.needing)
    return report


def status(now: datetime) -> dict:
    """The keeper as a document: `on` (it looked within the last minutes), `since`, `last_restart` (`at`, `resumed`,
    `waiting`), `needs_person` (lanes it stopped trying) and `phone_page` (whether it runs the phone page)."""
    held = _read_state()
    beat = _parse(held.get("heartbeat_at"))
    return {"on": beat is not None and now - beat <= HEARTBEAT_FRESH, "since": held.get("since"),
            "last_restart": held.get("last_restart"), "needs_person": list(held.get("needs_person", [])),
            "phone_page": held.get("phone_page", True)}


def set_phone_page(is_on: bool) -> None:
    """Keep whether the keeper runs the phone page; the running keeper reads it on its next look."""
    held = _read_state()
    held["phone_page"] = is_on
    _write_state(held)


def serve(ports: Ports | None = None, *, now: Callable[[], datetime] | None = None,
          phone_page: PhonePage | None = None, wait: Callable[[float], bool] | None = None) -> None:
    """Look every minute until `wait` says to stop, keep the phone page running when it is on and the ports' Leeway serve
    (which keeps the usage readings fresh) running, and stop that serve when the keeper stops.

    A look that fails is printed to stderr (the agent's log) and the loop goes on. `wait(seconds)` returns True to stop.
    """
    ports = ports or real_ports()
    clock = now or (lambda: datetime.now(timezone.utc))
    wait = wait or threading.Event().wait
    _begin(clock())
    phone = _PhonePage(phone_page)
    try:
        while True:
            _look(clock, ports)
            phone.keep(clock(), set_on=_read_state().get("phone_page", True))
            _keep_leeway_serve(ports.leeway_serve)
            if ports.review_refresh is not None:
                ports.review_refresh()
            if wait(LOOP_SECONDS):
                return
    finally:
        if ports.leeway_serve is not None:
            ports.leeway_serve.stop()


def _keep_leeway_serve(serve_leeway: leeway_serve.LeewayServe | None) -> None:
    if serve_leeway is None:
        return
    try:
        serve_leeway.keep()
    except OSError as error:  # the next look tries again; the readings only go stale meanwhile
        print(f"keeper: leeway serve not started: {type(error).__name__}: {error}", file=sys.stderr)


class _PhonePage:
    """The phone page server in a thread of its own, started again at most every `PHONE_PAGE_RETRY` when it ends and
    stopped, with its thread joined, when the person turns it off."""

    def __init__(self, serve_page: PhonePage | None) -> None:
        self._serve, self._thread, self._started_at = serve_page, None, None
        self._stop = threading.Event()

    def keep(self, now: datetime, *, set_on: bool) -> None:
        """Start the server when it is on and not running (and not tried a moment ago); stop it when it is off."""
        if self._serve is None:
            return
        if not set_on:
            self._stop_running()
            return
        if self._thread and self._thread.is_alive():
            return
        if self._started_at is not None and now - self._started_at < PHONE_PAGE_RETRY:
            return
        self._started_at, self._stop = now, threading.Event()
        self._thread = threading.Thread(target=self._run, args=(self._stop,), name="phone-page", daemon=True)
        self._thread.start()

    def _stop_running(self) -> None:
        if not self._thread:
            return
        self._stop.set()
        self._thread.join(PHONE_PAGE_STOP_SECONDS)
        if self._thread.is_alive():
            print("keeper: the phone page did not stop in time", file=sys.stderr)
            return
        self._thread, self._started_at = None, None

    def _run(self, stop: threading.Event) -> None:
        try:
            self._serve(stop)
        except Exception as error:  # noqa: BLE001 (the keeper's work goes on; the reason is in the agent's log)
            print(f"keeper: the phone page stopped: {type(error).__name__}: {error}", file=sys.stderr)


def _look(clock: Callable[[], datetime], ports: Ports) -> None:
    try:
        now = clock()
        run_once(now, ports)
        attention.prune(now)
    except Exception as error:  # noqa: BLE001 (a failed look must not end the keeper; the next look tries again)
        print(f"keeper: look failed: {type(error).__name__}: {error}", file=sys.stderr)


# What to do with each lane

@dataclass
class _Scan:
    """What a look found: the lanes to resume, how many lanes still have a live run, the names of the lanes that wait on
    the person with a dead open run, and of those the keeper gave up on."""

    dead: list[tuple[Path, store.TaskStore]]
    alive: int = 0
    waiting: list[str] = field(default_factory=list)
    needing: list[str] = field(default_factory=list)


def _scan(ports: Ports) -> _Scan:
    scan = _Scan([])
    for repo in ports.repos():
        for task in store.task_stores(repo):
            verdict, name = _verdict(task, ports.is_alive), lane_ceiling.lane_name(repo, task.task_id)
            if verdict == "alive":
                scan.alive += 1
            elif verdict == "resume":
                scan.dead.append((repo, task))
            elif verdict == "waiting":
                scan.waiting.append(name)
            elif verdict == "asked":
                scan.needing.append(name)
    return scan


def _verdict(task: store.TaskStore, is_alive: Callable[[dict], bool]) -> str:
    """`alive`, `resume`, `waiting`, `asked` or `leave` for one lane: a lane with open run records, none of them alive,
    or a running lane with no open record and no engine process holding its lock, is resumed, and not when it was
    stopped or waits on the person; `asked` is a lane the keeper gave up on."""
    try:
        state = task.read_state()
    except (store.StateError, OSError):
        return "leave"
    if state.get("status") == "blocked" and state.get("reason") == CRASH_LOOP_REASON:
        return "asked"
    open_runs = [run for run in state.get("runs", {}).values() if run.get("state") in run_records.LIVE_STATES]
    if not open_runs:
        return "resume" if _is_running_without_driver(task, state) else "leave"
    if any(not run_records.is_dead(run, is_alive) for run in open_runs):
        return "alive"
    status_word = state.get("status")
    if status_word == STOPPED or task.stop_file.exists() or status_word in FINISHED:
        return "leave"
    return "resume" if status_word == RUNNING and not _asks_the_person(task) else "waiting"


def _is_running_without_driver(task: store.TaskStore, state: dict) -> bool:
    """A lane the engine calls running between two runs (or in a phase with no agent) whose engine process is gone: the
    driver holds the task lock for as long as it lives, so a free lock means nobody is driving."""
    if state.get("status") != RUNNING or task.stop_file.exists() or _asks_the_person(task):
        return False
    try:
        with task.lock():
            return True
    except store.EngineBusy:
        return False


def _asks_the_person(task: store.TaskStore) -> bool:
    try:
        return bool(lane_questions.open_questions(task))
    except lane_questions.QuestionsBroken:
        return True


def _act(now: datetime, ports: Ports, held: dict, scan: _Scan) -> KeeperReport:
    resumes = {name: [moment for moment in times if now - _parse(moment) < CRASH_WINDOW]
               for name, times in held.get("resumes", {}).items()}
    room = max(ports.lanes() - scan.alive, 0)
    resumed, queued, asked = [], [], []
    for repo, task in sorted(scan.dead, key=lambda pair: (str(pair[0]), pair[1].task_id)):
        name = lane_ceiling.lane_name(repo, task.task_id)
        if len(resumes.get(name, [])) >= CRASH_RESUMES:
            _ask_the_person(task, ports.is_alive)
            asked.append(name)
        elif len(resumed) >= room:
            queued.append(name)
        else:
            ports.resume(repo, task)
            resumes.setdefault(name, []).append(_format(now))
            resumed.append(name)
    held["resumes"] = {name: times for name, times in resumes.items() if times}
    return KeeperReport(tuple(resumed), tuple(queued), (), tuple(asked))


def _ask_the_person(task: store.TaskStore, is_alive: Callable[[dict], bool]) -> None:
    """Stop trying: close the dead records and block the lane with a reason, which the person sees as any blocked lane."""
    run_records.reconcile(task, is_alive)
    task.update_state(lambda state: state.update(status="blocked", reason=CRASH_LOOP_REASON))
    task.append_ledger("keeper", CRASH_LOOP_REASON)


# What it remembers

def _remember(now: datetime, held: dict, report: KeeperReport, needing: list[str]) -> None:
    held["heartbeat_at"] = _format(now)
    held["needs_person"] = sorted({*needing, *report.asked})
    if report.resumed or report.queued or report.waiting or report.asked:
        held["last_restart"] = _episode(now, held.get("last_restart"), report)
    _write_state(held)


def _episode(now: datetime, previous: dict | None, report: KeeperReport) -> dict:
    """The restart record: a look within `RESTART_EPISODE` of the last one adds to it, a later one starts a new record."""
    earlier = _parse((previous or {}).get("at"))
    if previous and earlier is not None and now - earlier < RESTART_EPISODE:
        return {"at": previous["at"], "resumed": sorted({*previous["resumed"], *report.resumed}),
                "waiting": sorted({*previous["waiting"], *report.waiting})}
    return {"at": _format(now), "resumed": list(report.resumed), "waiting": list(report.waiting)}


def _begin(now: datetime) -> None:
    held = _read_state()
    held["since"] = _format(now)
    _write_state(held)


def _read_state() -> dict:
    try:
        found = json.loads(state_path().read_text())
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as error:
        raise OSError(f"cannot read the keeper's state {state_path()}: {error}") from error
    return found if isinstance(found, dict) else {}


def _write_state(held: dict) -> None:
    gates.write_atomically(state_path(), json.dumps(held, sort_keys=True).encode())
    state_path().chmod(FILE_MODE)


def _format(moment: datetime) -> str:
    return moment.astimezone(timezone.utc).strftime(TIME_FORMAT)


def _parse(text: str | None) -> datetime | None:
    try:
        return datetime.strptime(text, TIME_FORMAT).replace(tzinfo=timezone.utc) if text else None
    except ValueError:
        return None
