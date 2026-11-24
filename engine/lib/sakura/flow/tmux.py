"""Thin tmux client: argument lists without a shell, exact session targets, markers on what sakura creates."""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Mapping, Sequence

from sakura import programs

NAME_LIMIT = 60
HASH_LENGTH = 6
COMMAND_TIMEOUT_SECONDS = 10
TMUX = "tmux"
ENV_PROGRAM = "/usr/bin/env"
# The private server of the flow surface: the engine never starts or touches the person's default tmux server.
SURFACE_SOCKET = "sakura"
# The longest socket path the system takes (`sockaddr_un.sun_path` less its final NUL): tmux refuses a longer one with
# "File name too long". 104 bytes on macOS, 108 on Linux.
SOCKET_PATH_LIMIT = 103 if sys.platform == "darwin" else 107
# The private server starts with no config file: the person's own tmux.conf may restore their saved sessions
# (and the agents in them) into any new server and overwrite their saved layout from it.
NO_CONFIG_FILE = "/dev/null"
PANE_MARKER_KEY = "@sakura-pane"
WINDOW_LISTING_FORMAT = "#{window_id} #{window_activity}"
OPTION_PREFIX = "@sakura-"

_NAME_CHARS = re.compile(r"[^A-Za-z0-9._-]")
_SAFE_NAME = re.compile(rf"[A-Za-z0-9._-]{{1,{NAME_LIMIT + 1 + HASH_LENGTH}}}")
_USER_OPTION = re.compile(r"@[A-Za-z0-9_-]+")
_ENV_KEY = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_WINDOW_ID = re.compile(r"@[0-9]+")
_PANE_ID = re.compile(r"%[0-9]+")
_SAKURA_LISTING = re.compile(rf"({re.escape(OPTION_PREFIX)}[A-Za-z0-9_-]+)(?:\s|$)")
# How tmux words a target that went away; a window in teardown can still answer display-message.
_GONE_MARKER = "can't find"


class TmuxError(RuntimeError):
    """Raised when tmux is missing, a command fails or times out, or an argument is refused before it is sent."""


@dataclass(frozen=True)
class Pane:
    """One pane: its id, whether its process has exited, and its `@sakura-pane` marker ("" when unset)."""

    id: str
    dead: bool
    marker: str


@dataclass(frozen=True)
class Window:
    """One window: id, name, its `@sakura-*` user options, whether every pane is dead, and its panes.

    `activity` is tmux's `window_activity` (epoch seconds of the last output), None when tmux did not say.
    """

    id: str
    name: str
    options: dict[str, str]
    dead: bool
    panes: list[Pane]
    activity: int | None = None


def surface_client() -> "TmuxClient":
    """The client of the flow surface: the private socket `tmux -L sakura` with no config file, never the default server."""
    return TmuxClient(socket=SURFACE_SOCKET, config_file=NO_CONFIG_FILE)


def is_env_name(key: str) -> bool:
    """Return whether `key` can be passed to a tmux window as an environment variable name."""
    return _ENV_KEY.fullmatch(key) is not None


def sanitize_name(name: str) -> str:
    """Return `name` with every character outside [A-Za-z0-9._-] replaced by `-`, at most NAME_LIMIT long."""
    return _NAME_CHARS.sub("-", name)[:NAME_LIMIT]


def marked_name(base: str, key: str) -> str:
    """Return `<sanitized base>-<first 6 hex of SHA-256 of key>` so bases that collide after sanitizing stay apart."""
    digest = hashlib.sha256(key.encode("utf-8", errors="surrogatepass")).hexdigest()
    return f"{sanitize_name(base)}-{digest[:HASH_LENGTH]}"


class TmuxClient:
    """Runs tmux as argument lists, on the default server or on the private socket `-L socket`."""

    def __init__(self, socket: str | None = None, run: Callable | None = None, config_file: str | None = None):
        self._socket = socket
        self._run = run or _pinned_run
        self._config_file = config_file

    @property
    def socket(self) -> str | None:
        """The `-L` socket name, or None for the default server."""
        return self._socket

    def socket_path_problem(self) -> str:
        """Why tmux cannot make the socket of `-L <socket>` where it would (`$TMUX_TMPDIR/tmux-<uid>/<socket>`, the
        folder's real path, `/tmp` by default), or '' when the path fits the system's limit."""
        if not self._socket:
            return ""
        folder = os.path.realpath(os.environ.get("TMUX_TMPDIR") or "/tmp")
        path = os.path.join(folder, f"tmux-{os.getuid()}", self._socket)
        size = len(os.fsencode(path))
        if size <= SOCKET_PATH_LIMIT:
            return ""
        return (f"the tmux socket path {path} is {size} bytes, over the {SOCKET_PATH_LIMIT}-byte limit the system sets "
                "for a socket path, so tmux cannot start its server there; set TMUX_TMPDIR to a shorter folder (such "
                f"as /tmp, where the socket is /tmp/tmux-{os.getuid()}/{self._socket}) and run the command again")

    # Session lifecycle

    def available(self) -> bool:
        """Return whether a `tmux` executable is on PATH."""
        try:
            programs.path(TMUX)
        except programs.ProgramError:
            return False
        return True

    def has_server(self) -> bool:
        """Return whether a tmux server is already running; asking never starts one."""
        return self._call(["list-sessions"])[0] == 0

    def has_session(self, name: str) -> bool:
        """Return whether a session called `name` exists."""
        return self._call(["has-session", "-t", self._session_target(name)])[0] == 0

    def ensure_session(self, name: str, cwd: Path, first_window: str, argv: Sequence[str],
                       env: Mapping[str, str], marker: tuple[str, str]) -> bool:
        """Create the session with `first_window` running `argv` and set its marker option.

        Returns True when the session exists with this marker (created now or before), False for an existing
        session with another marker; that one is not touched. The first window is the command itself, so no
        shell window is left behind.
        """
        key, value = marker
        if not value:
            raise TmuxError("a session marker needs a non-empty value")
        if self.has_session(name):
            return self.option(name, key) == value
        _check_name(first_window)
        self._checked(["new-session", "-d", "-s", _tmux_session_name(name), "-n", first_window,
                       *_spawn_args(cwd, env), "--", *_command(argv, env)])
        self._mark_or_remove(name, key, value)
        return True

    def kill_session(self, name: str, marker: tuple[str, str]) -> bool:
        """Kill the session when it carries `marker`; False (nothing touched) when it is absent or foreign."""
        key, value = marker
        if not self.has_session(name) or self.option(name, key) != value:
            return False
        self._checked(["kill-session", "-t", self._session_target(name)])
        return True

    def socket_path(self) -> str:
        """Return the path of the socket of the server this client talks to; the server must be running."""
        return self._checked(["display-message", "-p", "#{socket_path}"]).removesuffix("\n")

    def switch_or_attach(self, session: str) -> list[str]:
        """Return the command that brings a terminal to `session`: switch-client inside tmux, else attach."""
        if self._inside_own_server():
            return [*self._base(), "switch-client", "-t", self._session_target(session)]
        # Another tmux server's $TMUX would make tmux refuse to nest the attach.
        unset = ["env", "-u", "TMUX"] if os.environ.get("TMUX") else []
        return [*unset, *self._base(), "attach-session", "-t", self._session_target(session)]

    def _inside_own_server(self) -> bool:
        """Whether this terminal is already inside the server this client talks to ($TMUX is `socket,pid,index`)."""
        inside = os.environ.get("TMUX")
        if not inside:
            return False
        return self._socket is None or Path(inside.rsplit(",", 2)[0]).name == self._socket

    # Options

    def option(self, target: str, key: str) -> str:
        """Return the user option `key` on a session name, window id (@N) or pane id (%N); "" when unset."""
        _check_user_option(key)
        flags, resolved = self._scope(target)
        return self._checked(["show-options", "-q", "-v", *flags, "-t", resolved, "--", key]).removesuffix("\n")

    def set_option(self, target: str, key: str, value: str) -> None:
        """Set the user option `key` (`@name` only; built-in options are refused) on a session, window or pane."""
        _check_user_option(key)
        self._set(target, key, value)

    # Windows and panes

    def new_window(self, session: str, name: str, argv: Sequence[str], cwd: Path, env: Mapping[str, str],
                   remain: bool = True) -> str:
        """Add a detached window running `argv` and return its id; `remain` keeps it visible after exit.

        Creation, `remain-on-exit` and the final name are one tmux invocation under a temporary name, so a
        command that exits at once is still kept (or dropped) as asked.
        """
        _check_name(name)
        target = self._session_target(session)
        temporary = f"sakura-new-{uuid.uuid4().hex[:HASH_LENGTH]}"
        by_name = f"{target}={temporary}"
        create = ["new-window", "-d", "-P", "-F", "#{window_id}", "-t", target, "-n", temporary,
                  *_spawn_args(cwd, env), "--", *_command(argv, env)]
        keep = ["set-option", "-w", "-t", by_name, "remain-on-exit", "on" if remain else "off"]
        return self._checked(create, keep, ["rename-window", "-t", by_name, "--", name]).strip()

    def split_window(self, target: str, argv: Sequence[str], cwd: Path, env: Mapping[str, str],
                     pane_marker: str) -> str:
        """Split `target` (window or pane id), run `argv` in the new pane, mark it, and return the pane id."""
        flags, resolved = self._scope(target)
        if flags == []:
            raise TmuxError(f"split_window needs a window or pane id, got {target!r}")
        pane = self._checked(["split-window", "-d", "-P", "-F", "#{pane_id}", "-t", resolved,
                              *_spawn_args(cwd, env), "--", *_command(argv, env)]).strip()
        self.set_option(pane, PANE_MARKER_KEY, pane_marker)
        return pane

    def windows(self, session: str) -> list[Window]:
        """Return the session's windows; a window that vanishes while it is read is left out."""
        listing = self._checked(["list-windows", "-t", self._session_target(session), "-F", WINDOW_LISTING_FORMAT])
        found = []
        for window_id, activity in filter(None, map(_listing_row, listing.splitlines())):
            try:
                found.append(self._read_window(window_id, activity))
            except TmuxError as error:
                if _GONE_MARKER not in str(error) and self._window_exists(window_id):
                    raise
        return found

    def pane_title(self, pane: str) -> str:
        """Return the pane title as the program set it; untrusted text, strip controls before printing."""
        _check_pane(pane)
        return self._checked(["display-message", "-p", "-t", pane, "#{pane_title}"]).removesuffix("\n")

    def set_remain_on_exit(self, window: str, remain: bool) -> None:
        """Keep (`remain`) or drop the window once its command exits."""
        _check_window(window)
        self._checked(["set-option", "-w", "-t", window, "remain-on-exit", "on" if remain else "off"])

    def server_start_time(self, pane: str) -> int:
        """Return the epoch second the tmux server holding this pane started; a pane id never outlives its server."""
        _check_pane(pane)
        return int(self._checked(["display-message", "-p", "-t", pane, "#{start_time}"]).strip())

    def select_window(self, window: str) -> None:
        """Make the window with this id (@N) the current one of its session."""
        _check_window(window)
        self._checked(["select-window", "-t", window])

    def kill_window(self, window: str) -> None:
        """Kill the window with this id (@N)."""
        _check_window(window)
        self._checked(["kill-window", "-t", window])

    # Input

    def paste(self, pane: str, text: str) -> None:
        """Paste `text` into the pane as one bracketed paste with newlines kept; it is not submitted."""
        _check_pane(pane)
        _check_paste_text(text)
        buffer = f"sakura-{uuid.uuid4().hex}"
        self._checked(["set-buffer", "-b", buffer, "--", text])
        try:
            self._checked(["paste-buffer", "-p", "-r", "-d", "-b", buffer, "-t", pane])
        except TmuxError:
            self._call(["delete-buffer", "-b", buffer])
            raise

    def send_enter(self, pane: str) -> None:
        """Press Enter in the pane."""
        _check_pane(pane)
        self._checked(["send-keys", "-t", pane, "Enter"])

    # Private: running tmux

    def _base(self) -> list[str]:
        return ["tmux", *(["-L", self._socket] if self._socket else []),
                *(["-f", self._config_file] if self._config_file else [])]

    def _call(self, *commands: Sequence[str]) -> tuple[int, str, str]:
        """Run one tmux invocation made of `commands` joined by `;`; every argument stays literal."""
        argv = self._base()
        for position, command in enumerate(commands):
            argv += ([";"] if position else []) + [_literal(arg) for arg in command]
        try:
            done = self._run(argv, capture_output=True, encoding="utf-8", errors="replace",
                             stdin=subprocess.DEVNULL, check=False, timeout=COMMAND_TIMEOUT_SECONDS)
        except FileNotFoundError as error:
            raise TmuxError("tmux is not installed") from error
        except subprocess.TimeoutExpired as error:
            raise TmuxError(f"tmux {commands[0][0]} timed out after {COMMAND_TIMEOUT_SECONDS} seconds") from error
        return done.returncode, done.stdout, done.stderr

    def _checked(self, *commands: Sequence[str]) -> str:
        code, out, err = self._call(*commands)
        if code:
            raise TmuxError(f"tmux {commands[0][0]} failed: {(err or out).strip()}")
        return out

    # Private: targets and options

    def _session_target(self, name: str) -> str:
        _check_name(name)
        return f"={_tmux_session_name(name)}:"

    def _scope(self, target: str) -> tuple[list[str], str]:
        if target.startswith("%"):
            _check_pane(target)
            return ["-p"], target
        if target.startswith("@"):
            _check_window(target)
            return ["-w"], target
        return [], self._session_target(target)

    def _set(self, target: str, key: str, value: str) -> None:
        flags, resolved = self._scope(target)
        self._checked(["set-option", *flags, "-t", resolved, "--", key, value])

    def _mark_or_remove(self, name: str, key: str, value: str) -> None:
        try:
            self.set_option(name, key, value)
        except TmuxError:
            self._call(["kill-session", "-t", self._session_target(name)])
            raise

    # Private: reading windows

    def _read_window(self, window_id: str, activity: int | None = None) -> Window:
        name = self._checked(["display-message", "-p", "-t", window_id, "#{window_name}"]).removesuffix("\n")
        panes = self._read_panes(window_id)
        return Window(window_id, name, self._sakura_options(window_id),
                      bool(panes) and all(pane.dead for pane in panes), panes, activity)

    def _read_panes(self, window_id: str) -> list[Pane]:
        rows = self._checked(["list-panes", "-t", window_id, "-F", "#{pane_id} #{pane_dead}"]).splitlines()
        return [Pane(pane_id, dead == "1", self.option(pane_id, PANE_MARKER_KEY))
                for pane_id, dead in (row.split(" ") for row in rows)]

    def _sakura_options(self, window_id: str) -> dict[str, str]:
        listing = self._checked(["show-options", "-w", "-t", window_id]).splitlines()
        keys = [match.group(1) for match in map(_SAKURA_LISTING.match, listing) if match]
        return {key: self.option(window_id, key) for key in keys}

    def _window_exists(self, window_id: str) -> bool:
        return self._call(["display-message", "-p", "-t", window_id, ""])[0] == 0


def _listing_row(row: str) -> tuple[str, int | None] | None:
    """Split one `window_id window_activity` row; the activity is None when it is absent or not a number."""
    fields = row.split()
    if not fields:
        return None
    return fields[0], int(fields[1]) if len(fields) > 1 and fields[1].isdigit() else None


def _tmux_session_name(name: str) -> str:
    """Mirror tmux, which stores `.` and `:` in session names as `_`."""
    return name.replace(".", "_").replace(":", "_")


def _pinned_run(argv: Sequence[str], **kwargs):
    """`subprocess.run` for tmux: the engine's pinned `tmux` and an environment made for it (the private tmux
    directory and the terminal settings, nothing else of the engine's)."""
    executable = programs.path(TMUX)
    env = programs.environment("tmux", executable)
    return subprocess.run([executable, *argv[1:]], env=env, **kwargs)


def _literal(arg: str) -> str:
    """Escape a final `;`: tmux reads an argument that ends with one as a command separator."""
    return arg[:-1] + "\\;" if arg.endswith(";") else arg


def _spawn_args(cwd: Path, env: Mapping[str, str]) -> list[str]:
    for key in env:
        if not _ENV_KEY.fullmatch(key):
            raise TmuxError(f"refused environment variable name {key!r}")
    args = ["-c", str(cwd).replace("#", "##")]
    for key, value in env.items():
        args += ["-e", f"{key}={value}"]
    return args


def _command(argv: Sequence[str], env: Mapping[str, str]) -> list[str]:
    """`argv`, behind `env PATH=...` when the caller gives a PATH.

    tmux ignores `-e PATH=`: a window gets the PATH of the client that created it, and the engine's tmux client has a
    fixed one (so its own lookups never use a repository's folders). Without this, a window's `python3` and every
    program a role or a test command looks up would be the system's, not the person's.
    """
    if not argv:
        raise TmuxError("refused an empty command")
    path = env.get("PATH")
    return [ENV_PROGRAM, f"PATH={path}", *argv] if path else list(argv)


def _check_name(name: str) -> None:
    if not _SAFE_NAME.fullmatch(name):
        raise TmuxError(f"refused tmux name {name!r}: use [A-Za-z0-9._-], at most {NAME_LIMIT + 1 + HASH_LENGTH} long")


def _check_user_option(key: str) -> None:
    if not _USER_OPTION.fullmatch(key):
        raise TmuxError(f"refused option {key!r}: only user options (@name) are allowed")


def _check_window(window: str) -> None:
    if not _WINDOW_ID.fullmatch(window):
        raise TmuxError(f"refused window id {window!r}: expected @<number>")


def _check_pane(pane: str) -> None:
    if not _PANE_ID.fullmatch(pane):
        raise TmuxError(f"refused pane id {pane!r}: expected %<number>")


_PASTE_CONTROL = re.compile(r"[\x01-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]")


def paste_refusal(text: str) -> str | None:
    """Return why `text` cannot be pasted, or None when it can."""
    if not text:
        return "nothing to paste"
    if "\x00" in text:
        return "refused text with a NUL character"
    if _PASTE_CONTROL.search(text):
        # ESC could end the bracketed paste early and turn the rest into keystrokes.
        return "refused text with a control character (only newline, carriage return and tab pass)"
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        return "refused text that is not valid Unicode"
    return None


def _check_paste_text(text: str) -> None:
    reason = paste_refusal(text)
    if reason:
        raise TmuxError(reason)
