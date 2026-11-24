"""The tmux surface of a flow: a session per task, a live window per run; it never changes a run's outcome."""

from __future__ import annotations

import contextlib
import fcntl
import os
import sys
import threading
from pathlib import Path
from typing import Callable, Iterator, Mapping, Protocol, Sequence

from sakura import engine_home, interpreter
from sakura.flow import run_records
from sakura.flow.config import FlowConfig
from sakura.flow.render import strip_controls
from sakura.flow.store import StateError, TaskStore, flow_root
from sakura.flow.tmux import TmuxClient, TmuxError, is_env_name, marked_name, sanitize_name, surface_client

KIT_BIN = Path(__file__).resolve().parents[3] / "bin" / "sakura"
EVENTS_WINDOW = "events"
ENGINE_WINDOW = "engine"
TASK_MARKER_KEY = "@sakura-task"
RUN_MARKER_KEY = "@sakura-run"
ENGINE_MARKER_KEY = "@sakura-engine"
OPEN_MARKER_KEY = "@sakura-open"
NEEDS_TMUX = "needs tmux (surface.tmux = true and tmux on PATH)"
UNFORWARDED_VARIABLES = ("TMUX", "TMUX_PANE")
# The engine home too: a helper window (events, watch, cockpit) that lost it would register its repository in the default home.
WINDOW_ENV_KEYS = ("HOME", "PATH", engine_home.ENV_VARIABLE)
WARNED_KEY = "surface_warned"
LAUNCH_LOCK_SUFFIX = ".launch.lock"
LEDGER_TAG = "surface"
REASON_LIMIT = 200


class SurfaceUnavailable(RuntimeError):
    """Raised by commands that need tmux when the surface is off or tmux is missing."""


class DriverRunning(RuntimeError):
    """Raised when a detached driver of the task is still running in its session."""


class Surface(Protocol):
    """Where a task is shown to its owner. Every method is best effort and never raises for a display failure."""

    def task_opened(self, store: TaskStore) -> None:
        """The task is being driven: make sure its session and ledger window exist."""

    def run_started(self, store: TaskStore, run: str, role: str) -> None:
        """A run record has begun: show its log live."""

    def task_closed(self, store: TaskStore) -> None:
        """The task is cleaned: end its session, without killing the caller's own window first."""

    def task_settled(self, store: TaskStore) -> None:
        """The task reached a final state (report ready, failed): close its ledger window; the rest stays."""


class NullSurface:
    """The headless surface: does nothing."""

    def task_opened(self, store: TaskStore) -> None:
        return None

    def run_started(self, store: TaskStore, run: str, role: str) -> None:
        return None

    def task_closed(self, store: TaskStore) -> None:
        return None

    def task_settled(self, store: TaskStore) -> None:
        return None


class TmuxSurface:
    """Shows a task in tmux. A failure is ledgered once per task (`surface_warned` in state) and then ignored.

    Every call retries the session, so a killed session or a restarted server is rebuilt by the next event.
    """

    def __init__(self, client: TmuxClient, prefix: str, environ: Mapping[str, str] | None = None) -> None:
        self._client = client
        self._prefix = prefix
        self._environ = os.environ if environ is None else environ
        self._lock = threading.Lock()

    def task_opened(self, store: TaskStore) -> None:
        self._guarded(store, lambda: self._ensure_session(store))

    def run_started(self, store: TaskStore, run: str, role: str) -> None:
        self._guarded(store, lambda: self._open_run_window(store, run, role))

    def task_closed(self, store: TaskStore) -> None:
        """Inside the task session, keep the caller's window and let it end the session; outside, kill the session.

        Only a session that carries this task's marker is touched.
        """
        self._guarded(store, lambda: self._close(store))

    def task_settled(self, store: TaskStore) -> None:
        """Close the task's ledger window, which follows the ledger forever otherwise; only in a session that carries
        this task's marker, on this surface's own server, and never the window this process runs in."""
        self._guarded(store, lambda: self._close_events(store))

    # Operator commands: these report a tmux failure instead of ledgering it

    def session_for(self, store: TaskStore) -> str:
        """Return the name of the task's session, whether or not it exists."""
        return session_name(self._prefix, store.repo, store.task_id)

    def start_engine(self, store: TaskStore, argv: Sequence[str], env: Mapping[str, str]) -> str:
        """Run `argv` in window `engine` of the task session (created if needed) and return the session name.

        Raises DriverRunning while an earlier engine window is alive; one that has ended is replaced.
        Raises TmuxError when the session cannot be used. Check and creation are atomic across processes.
        """
        with self._lock, self._launch_lock(store):
            session = self._ensure_session(store)
            self._retire_ended_engine(session)
            window = self._client.new_window(session, ENGINE_WINDOW, argv, store.repo, env)
            self._client.set_option(window, ENGINE_MARKER_KEY, store.task_id)
        return session

    def rebuild(self, store: TaskStore) -> str:
        """Make sure the session, its ledger window and a watch window for every unended run exist."""
        with self._lock:
            session = self._ensure_session(store)
            if not any(window.name == EVENTS_WINDOW for window in self._client.windows(session)):
                self._client.new_window(session, EVENTS_WINDOW, self._events_argv(store), store.repo, self._window_env())
            for record in _unended_runs(store):
                self._open_run_window(store, record["name"], record["role"])
        return session

    def attach_command(self, store: TaskStore) -> list[str]:
        """Return the command that brings the terminal to the task session."""
        return self._client.switch_or_attach(self.session_for(store))

    def open_conversation(self, store: TaskStore, run: str, argv: Sequence[str], cwd: Path,
                          env: Mapping[str, str]) -> bool:
        """Open window `<run>-open` running `argv`; return False when that run is already open in the session.

        Check and creation are atomic across processes: of two simultaneous callers, one opens and one gets False.
        """
        with self._lock, self._launch_lock(store):
            session = self._ensure_session(store)
            if any(window.options.get(OPEN_MARKER_KEY) == run for window in self._client.windows(session)):
                return False
            window = self._client.new_window(session, sanitize_name(f"{run}-open"), argv, cwd,
                                             {**self._window_env(), **env}, remain=False)
            self._client.set_option(window, OPEN_MARKER_KEY, run)
        return True

    # Private: guard

    @contextlib.contextmanager
    def _launch_lock(self, store: TaskStore) -> Iterator[None]:
        """Serialize launches of one task across processes, from the busy check through the marked window.

        The lock file sits beside the task directory, not in it: a task directory without a state file would be
        listed as a task of its own by a detached `start` that has not created the task yet.
        """
        path = flow_root(store.repo) / f"{store.task_id}{LAUNCH_LOCK_SUFFIX}"
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(stream, fcntl.LOCK_UN)

    def _guarded(self, store: TaskStore, action: Callable[[], object]) -> None:
        with self._lock:
            try:
                action()
            except (TmuxError, OSError) as error:
                _warn_once_quietly(store, error)

    # Private: opening

    def _ensure_session(self, store: TaskStore) -> str:
        name = self.session_for(store)
        if not self._client.ensure_session(name, store.repo, EVENTS_WINDOW, self._events_argv(store), self._window_env(),
                                           _marker(store)):
            raise TmuxError(f"session {name} exists but belongs to something else")
        return name

    @staticmethod
    def _events_argv(store: TaskStore) -> list[str]:
        """`--wait` keeps the window alive while a detached `start` has not created the task yet."""
        return [str(KIT_BIN), "flow", "events", "--wait", store.task_id]

    def _retire_ended_engine(self, session: str) -> None:
        for window in self._client.windows(session):
            if ENGINE_MARKER_KEY not in window.options and window.name != ENGINE_WINDOW:
                continue
            if not window.dead:
                raise DriverRunning(f"a detached driver is already running in {session}")
            self._client.kill_window(window.id)

    def _open_run_window(self, store: TaskStore, run: str, role: str) -> None:
        name = self._ensure_session(store)
        if any(window.options.get(RUN_MARKER_KEY) == run for window in self._client.windows(name)):
            return
        watch = [str(KIT_BIN), "flow", "watch", store.task_id, run]
        window = self._client.new_window(name, sanitize_name(f"{run}-{role}"), watch, store.repo, self._window_env())
        self._client.set_option(window, RUN_MARKER_KEY, run)

    def _window_env(self) -> dict[str, str]:
        forwarded = {key: self._environ[key] for key in WINDOW_ENV_KEYS if key in self._environ}
        return {**forwarded, **interpreter.environment()}

    # Private: closing

    def _close(self, store: TaskStore) -> None:
        name = session_name(self._prefix, store.repo, store.task_id)
        key, value = _marker(store)
        if not self._client.has_session(name) or self._client.option(name, key) != value:
            return
        own = self._own_window(name)
        if own is None:
            self._client.kill_session(name, (key, value))
            return
        self._client.set_remain_on_exit(own, False)
        for window in self._client.windows(name):
            if window.id != own:
                self._client.kill_window(window.id)

    def _close_events(self, store: TaskStore) -> None:
        name = self.session_for(store)
        key, value = _marker(store)
        if not self._client.has_session(name) or self._client.option(name, key) != value:
            return
        own = self._own_window(name)
        for window in self._client.windows(name):
            if window.name == EVENTS_WINDOW and window.id != own:
                self._client.kill_window(window.id)

    def _own_window(self, session: str) -> str | None:
        """The window of `session` this process runs in, or None when it runs elsewhere."""
        pane, server = self._environ.get("TMUX_PANE"), self._environ.get("TMUX")
        if not pane or not server or not self._is_own_server(server):
            return None
        for window in self._client.windows(session):
            if any(candidate.id == pane for candidate in window.panes):
                return window.id
        return None

    def _is_own_server(self, tmux_variable: str) -> bool:
        """Pane ids repeat across servers, so $TMUX (`socket,pid,index`) must name this client's server."""
        return os.path.realpath(tmux_variable.rsplit(",", 2)[0]) == os.path.realpath(self._client.socket_path())


def session_name(prefix: str, repo: Path, task: str) -> str:
    """Return the task's session name; the hash keeps repositories and tasks that sanitize alike apart."""
    return marked_name(f"{prefix}{repo.name}-{task}", f"{repo}:{task}")


def build_surface(config: FlowConfig, client: TmuxClient | None = None) -> Surface:
    """Return the tmux surface when `surface.tmux` is on and tmux is on PATH, else the null surface."""
    try:
        return require_tmux_surface(config, client)
    except SurfaceUnavailable:
        return NullSurface()


def require_tmux_surface(config: FlowConfig, client: TmuxClient | None = None) -> TmuxSurface:
    """Return the tmux surface; raise SurfaceUnavailable when `surface.tmux` is off, tmux is not on PATH or its
    socket path would be over the system's limit (named, with the way to shorten it)."""
    settings = config.values["surface"]
    if not settings["tmux"]:
        raise SurfaceUnavailable(NEEDS_TMUX)
    client = client or surface_client()
    if not client.available():
        raise SurfaceUnavailable(NEEDS_TMUX)
    problem = client.socket_path_problem()
    if problem:
        raise SurfaceUnavailable(problem)
    return TmuxSurface(client, settings["session_prefix"])


def driver_environment(environ: Mapping[str, str]) -> dict[str, str]:
    """Return `environ` as a detached driver should get it: all of it, but never the caller's tmux identity."""
    return {key: value for key, value in environ.items()
            if key not in UNFORWARDED_VARIABLES and is_env_name(key)}


def _unended_runs(store: TaskStore) -> list[dict]:
    runs = store.read_state().get("runs", {}).values()
    live = [record for record in runs if record["state"] in run_records.LIVE_STATES]
    return sorted(live, key=lambda record: (record["started_at"], record["name"]))


def _marker(store: TaskStore) -> tuple[str, str]:
    return TASK_MARKER_KEY, f"{store.repo}:{store.task_id}"


def _warn_once_quietly(store: TaskStore, error: Exception) -> None:
    """Warn once; if the store cannot take the warning, say so on stderr: display never fails the run."""
    try:
        _warn_once(store, error)
    except (StateError, OSError) as write_error:
        print(f"surface warning not recorded: {' '.join(strip_controls(str(write_error)).split())[:REASON_LIMIT]}",
              file=sys.stderr)


def _warn_once(store: TaskStore, error: Exception) -> None:
    """Ledger `surface unavailable` the first time only, across restarts of the engine."""
    if store.read_state().get(WARNED_KEY):
        return
    store.update_state(lambda state: state.update({WARNED_KEY: True}))
    reason = " ".join(strip_controls(str(error)).split())[:REASON_LIMIT]
    store.append_ledger(LEDGER_TAG, f"surface unavailable: {reason}")
