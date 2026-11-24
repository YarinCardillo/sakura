"""File-based task store under the engine home folder, outside the repository."""

from __future__ import annotations

import contextlib
import copy
import errno
import fcntl
import hashlib
import json
import os
import re
import shutil
import stat
import tempfile
import threading
from dataclasses import asdict, dataclass
from functools import cached_property
from pathlib import Path
from typing import Callable, Iterator, Literal, Sequence

from sakura import engine_home, repository
from sakura.flow import gitutil, ledger_times

MAX_TASK_ID_LENGTH = 48
MAX_LEDGER_TEXT_CHARS = 1000
MAX_AGENT_VALUE_CHARS = 300
LEDGER_CUT = "..."
SIMPLE_ESCAPES = {"\n": "\\n", "\t": "\\t"}
DEFAULT_TASK_ID = "task"
FILE_EXTENSION = re.compile(r"\.[a-z0-9]{1,5}$", re.IGNORECASE)
NON_SLUG = re.compile(r"[^a-z0-9]+")
TASK_ID = re.compile(rf"[a-z0-9][a-z0-9-]{{0,{MAX_TASK_ID_LENGTH - 1}}}")
DIGEST_DIR = Path(".local/state/sakura/flow-digests")
DIGEST_DIR_MODE = 0o700
DIGEST_FILE_MODE = 0o600
# The journal of state writes (`TaskStore._chain_file`): how many are kept, and the kinds a lane's own store takes in.
CHAIN_LENGTH = 256
SANCTIONED_KINDS = ("side", "touch")
INPUT_FILE_MODE = 0o600
MISSING_EXTERNAL_REGISTRY = "missing"
# A supervised external run also names its process group and the nonce in its environment (`flow/process.py`).
EXTERNAL_RUN_NONCE = re.compile(r"[0-9a-f]{32}")
EXTERNAL_RUN_KEYS = (frozenset({"pid", "command"}), frozenset({"pid", "command", "pgid", "nonce"}))
INBOX_DIR_NAME = "inbox"
HOOK_MARK_DIR = Path(".local/state/sakura/flow-hook-marks")
HOOK_MARK_SUFFIX = ".hook-mark"
# The state key of the task's own repository: `{"path": ..., "config_sha256": ...}` (`flow/task_repository.py`).
TASK_REPOSITORY_KEY = "task_repository"


@dataclass(frozen=True)
class Ruling:
    """One arbiter decision on a builder request; `path` is repo-relative (or absolute outside the repo)."""

    path: str
    kind: Literal["test_change", "timeout", "allowlist"]
    decision: Literal["grant", "deny"]
    why: str
    cost_if_wrong: str
    spec_change: bool


ReadState = Callable[["TaskStore"], dict]


class StateError(RuntimeError):
    """Raised when a task's state file cannot be read or created."""


class StateTampered(StateError):
    """Raised when state.json no longer matches what the engine last wrote."""

    def __init__(self, path: Path) -> None:
        super().__init__(f"state.json changed outside the engine: {path}")


class ExternalRunRegistryError(StateError):
    """Raised when the authenticated external child registry is invalid."""


class EngineBusy(RuntimeError):
    """Raised when another engine process holds the task lock."""

    def __init__(self, pid: int) -> None:
        super().__init__(f"engine already running (pid {pid})")
        self.pid = pid


def flow_root(repo: Path) -> Path:
    """Return the directory that holds every task of this repository."""
    return engine_home.flow_dir(repo)


def task_stores(repo: Path, *, folder_missing: bool = False) -> list["TaskStore"]:
    """Return the store of every task directory of `repo` that holds a state file.

    Reading a project's tasks works on that project, as opening one of its tasks does: its Git calls are verified
    against it from here on. A lane's engine process is started on one project, so without this it refused Git in
    every other registered project it lists (`others.md`, the lane ceiling) as a folder an agent can write.
    A missing project uses its saved state without opening Git in its folder.
    """
    if not folder_missing:
        gitutil.trust_repository(repo)
    root = _missing_project_flow_dir(repo) if folder_missing else flow_root(repo)
    if not root.is_dir():
        return []
    stores = (TaskStore(repo, path.name, folder_missing=folder_missing) for path in sorted(root.iterdir())
              if TASK_ID.fullmatch(path.name) and path.is_dir())
    return [task for task in stores if task.exists()]


def _missing_project_flow_dir(repo: Path) -> Path:
    return engine_home.root() / engine_home.project_id_for_path(repo) / engine_home.FLOW_DIR_NAME


def _only_stack_last_used_changed(before: dict | None, after: dict) -> bool:
    if before is None or {key: value for key, value in before.items() if key != "stacks"} != {
        key: value for key, value in after.items() if key != "stacks"
    }:
        return False
    old_stacks, new_stacks = before.get("stacks"), after.get("stacks")
    if not isinstance(old_stacks, dict) or not isinstance(new_stacks, dict) or old_stacks.keys() != new_stacks.keys():
        return False
    increased = False
    for stack_id, old_entry in old_stacks.items():
        new_entry = new_stacks[stack_id]
        if not isinstance(old_entry, dict) or not isinstance(new_entry, dict):
            return False
        old_used, new_used = old_entry.get("last_used"), new_entry.get("last_used")
        if {key: value for key, value in old_entry.items() if key != "last_used"} != {
            key: value for key, value in new_entry.items() if key != "last_used"
        }:
            return False
        if old_used != new_used:
            # Any move of the time: two writers touch one stack, and the one that took its time first may write last.
            if (isinstance(old_used, bool) or not isinstance(old_used, (int, float))
                    or isinstance(new_used, bool) or not isinstance(new_used, (int, float))):
                return False
            increased = True
    return increased


def task_id_for(ref: str, existing: dict[str, str], label: str | None = None) -> str:
    """Derive a unique task id from a source reference, or from `label` when the reference is not readable.

    `existing` maps task id to the ref it was created for. The same ref returns
    its id; a different ref that slugs to a taken id gets `-2`, `-3`, and so on.
    """
    for task_id, known_ref in existing.items():
        if known_ref == ref:
            return task_id
    base = _slug(label or ref)
    candidate, number = base, 1
    while candidate in existing:
        number += 1
        suffix = f"-{number}"
        candidate = base[: MAX_TASK_ID_LENGTH - len(suffix)] + suffix
    return candidate


def _slug(ref: str) -> str:
    name = ref.rstrip("/").rsplit("/", 1)[-1] or ref
    name = FILE_EXTENSION.sub("", name)
    slug = NON_SLUG.sub("-", name.lower()).strip("-")[:MAX_TASK_ID_LENGTH].strip("-")
    return slug or DEFAULT_TASK_ID


class TaskStore:
    """Files of one task: state, spec, contract, plan, ledger, runs and evidence."""

    def __init__(self, repo: Path, task_id: str, side_writer: bool = False, *, folder_missing: bool = False) -> None:
        """`side_writer`: this process writes the task's state beside the engine that drives it (a chat with one agent
        that continues after the lane's own runs ended, or runs next to them). Its writes go through the same locked
        writer as the engine's and are accepted by the engine's store (see `_is_accepted`); any other store that writes
        beside an engine is still refused as tampering. `folder_missing` opens saved state with no Git folder.
        """
        if not isinstance(task_id, str) or not TASK_ID.fullmatch(task_id):
            raise StateError("invalid task id")
        self.repo = repo
        self.task_id = task_id
        self.folder_missing = folder_missing
        self._is_side_writer = side_writer
        # Every engine process works on a task, so its Git calls verify workspaces against this repository.
        if not folder_missing:
            gitutil.trust_repository(repo)
        self._state_guard = threading.RLock()
        self._written_digest: str | None = None
        self._last_written: dict | None = None
        self._state_observer: Callable[[TaskStore], None] | None = None

    @property
    def is_side_writer(self) -> bool:
        """Whether this store declared itself a side writer."""
        return self._is_side_writer

    def become_side_writer(self) -> None:
        """Declare this store a side writer from now on (see `__init__`)."""
        self._is_side_writer = True

    @contextlib.contextmanager
    def as_side_writer(self) -> Iterator[None]:
        """Write as a side writer for the length of the block only: an operator's answer written under the task lock (an
        approval) beside a lane's own store that may still be alive must not read as tampering to it, and afterwards
        this store is the plain one it was."""
        before, self._is_side_writer = self._is_side_writer, True
        try:
            yield
        finally:
            self._is_side_writer = before

    # -- paths ------------------------------------------------------------

    @cached_property
    def _common_dir(self) -> Path:
        if self.folder_missing:
            from sakura.flow import registry

            if saved := registry.saved_common_dir(self.repo):
                return saved
            # Older bare registrations have no saved common directory.
            return self.repo if self._digest_path_for(self.repo).is_file() else self.repo / ".git"
        # Resolved once: the repository cannot move under a live store, and every path below used to spawn git. A
        # workspace of the task's own repository resolves to the person's, so its task folder, state digests and
        # hook marks are the engine's.
        return engine_home.project_common_dir(repository.common_dir(self.repo))

    @property
    def person_repository(self) -> Path:
        """The main worktree of the person's repository the task belongs to, also for a store opened on a workspace."""
        return engine_home.main_worktree(self._common_dir)

    @cached_property
    def _flow_dir(self) -> Path:
        if self.folder_missing:
            return _missing_project_flow_dir(self.repo)
        return engine_home.flow_dir_for(self._common_dir)

    @property
    def root(self) -> Path:
        return self._flow_dir / self.task_id

    @property
    def state_file(self) -> Path:
        return self.root / "state.json"

    @property
    def spec(self) -> Path:
        return self.root / "spec.md"

    @property
    def input(self) -> Path:
        return self.root / "input.md"

    @property
    def contract(self) -> Path:
        return self.root / "contract.json"

    @property
    def plan_md(self) -> Path:
        return self.root / "plan.md"

    @property
    def plan_json(self) -> Path:
        return self.root / "plan.json"

    @property
    def ledger(self) -> Path:
        return self.root / "ledger.md"

    @property
    def rulings(self) -> Path:
        return self.root / "rulings.json"

    @property
    def evidence_dir(self) -> Path:
        return self.root / "evidence"

    @property
    def report(self) -> Path:
        return self.root / "report.md"

    @property
    def local_report(self) -> Path:
        return self.root / "local-report.md"

    @property
    def stop_file(self) -> Path:
        return self.root / "STOP"

    def publish_stop(self, marker: str, require_state: Callable[[dict], None] | None = None) -> None:
        """Publish a private STOP marker, optionally checking persisted state in the same critical section."""
        with self._state_guard, self._digest_lock():
            if require_state is not None:
                require_state(self._read_state_locked())
            self._atomic_write(self.stop_file, marker.encode(), DIGEST_FILE_MODE)

    def clear_stop(self) -> bool:
        """Clear STOP under the same lock used to publish it. Return whether it existed."""
        with self._digest_lock():
            try:
                self.stop_file.unlink()
            except FileNotFoundError:
                return False
            return True

    @property
    def steer_file(self) -> Path:
        return self.root / "steer.txt"

    @property
    def inbox_dir(self) -> Path:
        """Return the mailbox of operator commands waiting for a safe boundary."""
        return self.root / INBOX_DIR_NAME

    @property
    def runs_dir(self) -> Path:
        return self.root / "runs"

    def hook_mark(self, run: str) -> Path:
        """Return the file the engine hook touches whenever it handles a call of run `run`.

        It sits in the operator state directory, in a directory keyed by a digest of the repository and task,
        so no directory a role may write (the task store included) can hold a forged mark and
        `prune_hook_marks` can drop every mark of the task at once.
        """
        run_key = hashlib.sha256(run.encode()).hexdigest()
        return self._hook_mark_dir() / f"{run_key}{HOOK_MARK_SUFFIX}"

    def prune_hook_marks(self) -> None:
        """Remove every hook mark of this task; other tasks' marks stay."""
        try:
            shutil.rmtree(self._hook_mark_dir())
        except FileNotFoundError:
            return

    def _hook_mark_dir(self) -> Path:
        identity = f"{self._common_dir}\0{self.task_id}"
        return Path.home() / HOOK_MARK_DIR / hashlib.sha256(identity.encode()).hexdigest()

    def hook_ran_since(self, run: str, started: float) -> bool:
        """True when the hook marked run `run` at or after `started` (epoch seconds); an older mark is stale."""
        try:
            return self.hook_mark(run).stat().st_mtime >= started
        except OSError:
            return False

    @property
    def operations_dir(self) -> Path:
        """Return the directory for non-agent command logs and raw output."""
        return self.runs_dir / "operations"

    @property
    def external_runs_file(self) -> Path:
        """Return the registry file for supervised children outside the engine process."""
        return self.root / "external-runs.json"

    # -- state ------------------------------------------------------------

    def exists(self) -> bool:
        """Return whether this task has a state file."""
        return self.state_file.exists()

    def create(self, initial: dict, input_data: bytes | None = None) -> None:
        """Create the task directory, its input (`input_data`, when given, before the state that records its
        digest), its state and an empty ledger; refuses an existing task."""
        if self.exists():
            raise StateError(f"task already exists: {self.state_file}")
        engine_home.ensure_private(self._flow_dir.parent)
        self.root.mkdir(parents=True, exist_ok=True)
        if input_data is not None:
            self._atomic_write(self.input, input_data, INPUT_FILE_MODE)
        self.write_state(initial)
        self.ledger.write_text(f"# Ledger: {self.task_id}\n")

    def read_state(self) -> dict:
        """Return the state.

        Raises StateError when the file is unreadable and StateTampered when this
        store's state changes beyond a registered stack's last-used timestamp.
        """
        with self._state_guard, self._digest_lock():
            return self._read_state_locked()

    def _read_state_locked(self) -> dict:
        """Read and validate state while the cross-process digest lock is held."""
        raw = self._read_regular_file()
        state = self._parse(raw)
        if not self._is_accepted(raw, state):
            raise StateTampered(self.state_file)
        self._trust_task_repository(state)
        return state

    @staticmethod
    def _trust_task_repository(state: dict) -> None:
        """The task's own repository (`flow/task_repository.py`), once the state names it, is trusted like the
        repository the store was opened on: Git calls in its workspaces are verified against it."""
        recorded = state.get(TASK_REPOSITORY_KEY)
        path = recorded.get("path") if isinstance(recorded, dict) else None
        if isinstance(path, str) and Path(path).is_dir():
            gitutil.trust_repository(Path(path))

    def _parse(self, raw: bytes) -> dict:
        try:
            return json.loads(raw)
        except ValueError as error:
            raise StateError(f"cannot read {self.state_file}: {error}") from error

    def _is_accepted(self, raw: bytes, state: object) -> bool:
        """Whether bytes are persisted for this task and differ only by stack last-used timestamps."""
        digest = hashlib.sha256(raw).hexdigest()
        task_ok = isinstance(state, dict) and state.get("task") == self.task_id
        if digest not in self._persistent_digests() or not task_ok:
            return False
        if self._is_side_writer or self._written_digest is None or digest == self._written_digest:
            return True
        if digest == self._read_side_digest() or self._only_sanctioned_writes_since(digest):
            self._written_digest, self._last_written = digest, copy.deepcopy(state)  # taken in: the engine's own record now
            return True
        return isinstance(state, dict) and _only_stack_last_used_changed(self._last_written, state)

    def _only_sanctioned_writes_since(self, digest: str) -> bool:
        """Whether every write between this store's own last one and the state on disk (`digest`) was a side writer's or a
        stack touch, however many there were and in whatever order: the journal of writes (`_chain_file`) links each
        digest to the one it replaced, and one write of anything else anywhere on the way is tampering."""
        links = {entry["digest"]: entry for entry in self._read_chain() if isinstance(entry, dict)}
        current = digest
        for _ in range(len(links)):
            entry = links.get(current)
            if entry is None or entry.get("kind") not in SANCTIONED_KINDS:
                return False
            current = entry.get("prev", "")
            if current == self._written_digest:
                return True
        return False

    def _read_regular_file(self) -> bytes:
        """Read state.json without following a symlink; anything but a regular file is tampering."""
        try:
            if not stat.S_ISREG(os.lstat(self.state_file).st_mode):
                raise StateTampered(self.state_file)
            descriptor = os.open(self.state_file, os.O_RDONLY | os.O_NOFOLLOW)
        except OSError as error:
            if error.errno == errno.ELOOP:
                raise StateTampered(self.state_file) from error
            raise StateError(f"cannot read {self.state_file}: {error}") from error
        with os.fdopen(descriptor, "rb") as stream:
            return stream.read()

    def last_written(self) -> dict | None:
        """Return a copy of the state this store last wrote, or None when it has not written one."""
        with self._state_guard:
            return copy.deepcopy(self._last_written)

    def write_state(self, state: dict) -> None:
        """Replace the state atomically in three crash-safe steps.

        The digest file first accepts the old and the new digest, then the state
        is replaced, then only the new digest stays accepted; when the replace
        fails the previous list is restored. A crash between steps leaves a state
        that read_state still accepts, and the next write closes the window.
        Everything runs under the cross-process digest lock; readers never write.
        """
        with self._state_guard, self._digest_lock():
            self._write_state_locked(state)
        self._notify_state_written()

    def set_state_observer(self, observer: Callable[[TaskStore], None] | None) -> None:
        """Call `observer(store)` after every accepted state write, outside the locks; it must not raise."""
        self._state_observer = observer

    def _notify_state_written(self) -> None:
        if self._state_observer is not None:
            self._state_observer(self)

    def _write_state_locked(self, state: dict, before: dict | None = None) -> None:
        """Persist state and its accepted digest while the cross-process lock is held; `before` is the state it replaces."""
        payload = json.dumps(state, indent=2, sort_keys=True).encode()
        new_digest = hashlib.sha256(payload).hexdigest()
        self.root.mkdir(parents=True, exist_ok=True)
        replaced = self._journal_entry(new_digest, state, before)
        previous = self._persistent_digests()
        self._write_persistent_digests([*previous, new_digest])
        try:
            self._atomic_write(self.state_file, payload, None)
        except BaseException:
            self._write_persistent_digests(previous)
            raise
        self._write_persistent_digests([new_digest])
        if self._is_side_writer:
            self._write_digest_list(self._side_file(), [new_digest])
        self._append_chain(replaced)
        self._written_digest = new_digest
        self._last_written = copy.deepcopy(state)

    @staticmethod
    def _atomic_write(target: Path, payload: bytes, mode: int | None) -> None:
        handle, temp_name = tempfile.mkstemp(dir=target.parent, prefix=f"{target.name}.", suffix=".tmp")
        try:
            with os.fdopen(handle, "wb") as stream:
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            if mode is not None:
                os.chmod(temp_name, mode)
            os.replace(temp_name, target)
        except BaseException:
            Path(temp_name).unlink(missing_ok=True)
            raise

    def _digest_file(self) -> Path:
        return self._digest_path_for(self._common_dir)

    def _digest_path_for(self, common_dir: Path) -> Path:
        identity = f"{common_dir}\0{self.task_id}"
        key = hashlib.sha256(identity.encode()).hexdigest()
        return Path.home() / DIGEST_DIR / f"{key}.sha256"

    def _chain_file(self) -> Path:
        return self._digest_file().with_suffix(".chain")

    def _read_chain(self) -> list:
        return self._read_digest_list(self._chain_file())

    def _journal_entry(self, digest: str, state: dict, before: dict | None) -> dict:
        """What this write is for the journal of writes: its digest, the digest of the file it replaces, and its kind: `side`
        for a declared side writer, `touch` when only the last-used times of stacks changed, `own` for any other."""
        try:
            replaced = self.state_file.read_bytes()
        except OSError:
            replaced = b""
        if before is None and replaced:
            try:
                before = json.loads(replaced)
            except ValueError:
                before = None
        kind = "side" if self._is_side_writer else "touch" if _only_stack_last_used_changed(before, state) else "own"
        return {"digest": digest, "prev": hashlib.sha256(replaced).hexdigest() if replaced else "", "kind": kind}

    def _append_chain(self, entry: dict) -> None:
        self._write_digest_list(self._chain_file(), [*self._read_chain(), entry][-CHAIN_LENGTH:])

    def _side_file(self) -> Path:
        return self._digest_file().with_suffix(".side")

    def _read_side_digest(self) -> str:
        """The digest of the state a side writer wrote last ("" when none did): kept with the accepted digests, where no
        role can write."""
        found = self._read_digest_list(self._side_file())
        return found[0] if found else ""

    def _persistent_digests(self) -> list[str]:
        """Accepted digests kept outside the git directory, which an agent cannot write; empty when missing."""
        return self._read_digest_list(self._digest_file())

    def _write_persistent_digests(self, digests: list[str]) -> None:
        self._write_digest_list(self._digest_file(), digests)

    @staticmethod
    def _read_digest_list(path: Path) -> list[str]:
        try:
            digests = json.loads(path.read_text())
        except (OSError, ValueError):
            return []
        return digests if isinstance(digests, list) else []

    def _write_digest_list(self, target: Path, digests: list[str]) -> None:
        self._ensure_digest_dir()
        self._atomic_write(target, json.dumps(digests).encode(), DIGEST_FILE_MODE)

    def register_command(self, digest: str) -> None:
        """Record the sha256 of an operator command file that the CLI is about to place in the inbox."""
        with self._digest_lock():
            known = self._read_digest_list(self._commands_file())
            if digest not in known:
                self._write_digest_list(self._commands_file(), [*known, digest])

    def command_registered(self, digest: str) -> bool:
        """Whether an operator command with exactly these bytes was registered and is still unconsumed."""
        with self._digest_lock():
            return digest in self._read_digest_list(self._commands_file())

    def unregister_command(self, digest: str) -> None:
        """Forget a registered command once it has been applied or rejected."""
        with self._digest_lock():
            known = self._read_digest_list(self._commands_file())
            if digest in known:
                self._write_digest_list(self._commands_file(), [item for item in known if item != digest])

    def _commands_file(self) -> Path:
        return self._digest_file().with_suffix(".commands")

    def _ensure_digest_dir(self) -> None:
        directory = self._digest_file().parent
        directory.mkdir(parents=True, exist_ok=True, mode=DIGEST_DIR_MODE)
        if directory.stat().st_mode & 0o777 != DIGEST_DIR_MODE:
            directory.chmod(DIGEST_DIR_MODE)

    @contextlib.contextmanager
    def _digest_lock(self) -> Iterator[None]:
        """Exclusive cross-process lock serializing every state write and every state read."""
        self._ensure_digest_dir()
        lock_path = self._digest_file().with_suffix(".lock")
        descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, DIGEST_FILE_MODE)
        try:
            os.fchmod(descriptor, DIGEST_FILE_MODE)
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    def update_state(self, change: Callable[[dict], None]) -> dict:
        """Apply `change` to the current state, write it and return the new state."""
        with self._state_guard, self._digest_lock():
            state = self._read_state_locked()
            before = copy.deepcopy(state)
            change(state)
            self._write_state_locked(state, before)
        self._notify_state_written()
        return state

    def register_external_run(self, pid: int, command: str, pgid: int = 0, nonce: str = "") -> None:
        """Register an external child by exact PID and command, without changing task state; a supervised one also by
        its process group and nonce, so a successor can end what is left of it."""
        self._validate_external_run(pid, command)
        record = {"pid": pid, "command": command}
        if nonce or pgid:
            self._validate_external_group(pgid, nonce)
            record.update(pgid=pgid, nonce=nonce)
        with self._digest_lock():
            records = self._read_external_runs_locked()
            existing = next((item for item in records if item["pid"] == pid), None)
            if existing:
                if existing["command"] != command:
                    raise ExternalRunRegistryError(f"external PID {pid} is registered to another command")
                return
            self._write_external_runs_locked([*records, record])

    def external_runs(self) -> list[dict]:
        """Return authenticated external child identities for parent recovery."""
        with self._digest_lock():
            return self._read_external_runs_locked()

    def remove_external_run(self, pid: int, command: str) -> bool:
        """Remove an external child only when both its PID and command match."""
        self._validate_external_run(pid, command)
        with self._digest_lock():
            records = self._read_external_runs_locked()
            if not any(item["pid"] == pid and item["command"] == command for item in records):
                return False
            remaining = [item for item in records if item["pid"] != pid]
            self._write_external_runs_locked(remaining)
            return True

    @staticmethod
    def _validate_external_run(pid: int, command: str) -> None:
        if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
            raise ValueError("external run pid must be a positive integer")
        if not isinstance(command, str) or not command:
            raise ValueError("external run command must be nonempty")

    @staticmethod
    def _validate_external_group(pgid: int, nonce: str) -> None:
        if isinstance(pgid, bool) or not isinstance(pgid, int) or pgid <= 1:
            raise ValueError("external run process group must be an integer above 1")
        if not isinstance(nonce, str) or not EXTERNAL_RUN_NONCE.fullmatch(nonce):
            raise ValueError("external run nonce must be 32 lowercase hex characters")

    def _read_external_runs_locked(self) -> list[dict]:
        registry, auth = self.external_runs_file, self._external_runs_auth_file()
        registry_exists, auth_exists = self._path_exists(registry), self._path_exists(auth)
        if not registry_exists and not auth_exists:
            return []
        if not registry_exists and auth_exists:
            digests = self._external_runs_digests(auth)
            if MISSING_EXTERNAL_REGISTRY in digests:
                return []
            raise ExternalRunRegistryError("external run registry authentication is incomplete")
        if not auth_exists:
            raise ExternalRunRegistryError("external run registry authentication is incomplete")
        try:
            payload = self._read_authenticated_file(registry)
            digests = json.loads(self._read_authenticated_file(auth))
            records = json.loads(payload)
        except (OSError, ValueError, UnicodeDecodeError) as error:
            raise ExternalRunRegistryError(f"cannot read external run registry authentication: {error}") from error
        digest = hashlib.sha256(payload).hexdigest()
        if not isinstance(digests, list) or not digests or any(not isinstance(item, str) for item in digests):
            raise ExternalRunRegistryError("external run registry authentication is malformed")
        if digest not in digests:
            raise ExternalRunRegistryError("external run registry authentication mismatch")
        if not isinstance(records, list):
            raise ExternalRunRegistryError("external run registry must be a list")
        validated = []
        seen = set()
        for item in records:
            if (not isinstance(item, dict) or set(item) not in EXTERNAL_RUN_KEYS
                    or isinstance(item["pid"], bool) or not isinstance(item["pid"], int) or item["pid"] <= 0
                    or not isinstance(item["command"], str) or not item["command"] or item["pid"] in seen):
                raise ExternalRunRegistryError("external run registry entry is malformed")
            if "nonce" in item:
                try:
                    self._validate_external_group(item["pgid"], item["nonce"])
                except ValueError as error:
                    raise ExternalRunRegistryError(f"external run registry entry is malformed: {error}") from None
            seen.add(item["pid"])
            validated.append(dict(item))
        return validated

    def _write_external_runs_locked(self, records: list[dict]) -> None:
        registry = self.external_runs_file
        registry.parent.mkdir(parents=True, exist_ok=True)
        payload = json.dumps(sorted(records, key=lambda item: item["pid"]), separators=(",", ":")).encode()
        digest = hashlib.sha256(payload).hexdigest()
        auth = self._external_runs_auth_file()
        old_digests = self._external_runs_digests(auth)
        old_payload = self._read_authenticated_file(registry) if self._path_exists(registry) else None
        old_digest = hashlib.sha256(old_payload).hexdigest() if old_payload is not None else None
        staged = ([old_digest] if old_digest else [MISSING_EXTERNAL_REGISTRY]) + [digest]
        self._atomic_write(auth, json.dumps(staged).encode(), DIGEST_FILE_MODE)
        try:
            self._atomic_write(registry, payload, DIGEST_FILE_MODE)
        except BaseException:
            if old_digests:
                self._atomic_write(auth, json.dumps(old_digests).encode(), DIGEST_FILE_MODE)
            else:
                auth.unlink(missing_ok=True)
            raise
        self._atomic_write(auth, json.dumps([digest]).encode(), DIGEST_FILE_MODE)

    def _external_runs_auth_file(self) -> Path:
        return self._digest_file().with_suffix(".external-runs.sha256")

    @staticmethod
    def _path_exists(path: Path) -> bool:
        try:
            os.lstat(path)
            return True
        except FileNotFoundError:
            return False

    @staticmethod
    def _read_authenticated_file(path: Path) -> bytes:
        if not stat.S_ISREG(os.lstat(path).st_mode):
            raise OSError(f"not a regular file: {path}")
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as stream:
            return stream.read()

    @staticmethod
    def _external_runs_digests(path: Path) -> list[str]:
        try:
            data = json.loads(TaskStore._read_authenticated_file(path))
        except FileNotFoundError:
            return []
        except (OSError, ValueError, UnicodeDecodeError) as error:
            raise ExternalRunRegistryError(f"cannot read external run registry authentication: {error}") from error
        if not isinstance(data, list) or any(not isinstance(item, str) for item in data):
            raise ExternalRunRegistryError("external run registry authentication is malformed")
        return data

    def append_ledger(self, phase: str, text: str, agent_values: Sequence[str] = ()) -> None:
        """Append one line to the ledger, tagged with its phase.

        The ledger is read as one entry per line, so nothing written to it may carry a line break: `text` and
        every value are made one line (line breaks and tabs become visible escapes, terminal control sequences are removed) and cut to a bounded length.
        `agent_values` are strings an agent wrote, such as review findings; each is shown as `[agent: ...]`
        after the engine's own text, so it cannot pass for an engine record.
        """
        line = f"- [{_ledger_value(phase, MAX_AGENT_VALUE_CHARS)}] {_ledger_value(text, MAX_LEDGER_TEXT_CHARS)}"
        tags = "".join(f" [agent: {_ledger_value(value, MAX_AGENT_VALUE_CHARS)}]" for value in agent_values)
        with self.ledger.open("a") as stream:
            offset = os.fstat(stream.fileno()).st_size
            stream.write(f"{line}{tags}\n")
        ledger_times.record(self.ledger, offset)

    # -- rulings ----------------------------------------------------------

    def add_ruling(self, ruling: Ruling) -> None:
        """Append a ruling to rulings.json and ledger it; written by the engine, never by an agent."""
        entries = self.ruling_entries() + [asdict(ruling)]
        payload = json.dumps({"rulings": entries}, indent=2).encode()
        self._atomic_write(self.rulings, payload, None)
        with self.ledger.open("a") as stream:
            stream.write(f"- Ruling: {ruling.decision} {ruling.kind} {ruling.path}, {ruling.why}, "
                         f"cost if wrong: {ruling.cost_if_wrong}\n")

    def granted(self, path: str, kind: str) -> bool:
        """Whether the latest ruling for `path` and `kind` is a grant; unreadable rulings grant nothing."""
        decisions = [entry.get("decision") for entry in self.ruling_entries()
                     if entry.get("path") == path and entry.get("kind") == kind]
        return bool(decisions) and decisions[-1] == "grant"

    def ruling_entries(self) -> list[dict]:
        """Return the stored rulings; empty when the file is missing or unreadable."""
        try:
            data = json.loads(self.rulings.read_text())
        except (OSError, ValueError):
            return []
        entries = data.get("rulings") if isinstance(data, dict) else None
        return [entry for entry in entries if isinstance(entry, dict)] if isinstance(entries, list) else []

    # -- exclusion --------------------------------------------------------

    @contextlib.contextmanager
    def lock(self) -> Iterator[None]:
        """Hold the task-wide engine lock; raises EngineBusy naming the holder's pid."""
        self.root.mkdir(parents=True, exist_ok=True)
        path = self.root / "lock"
        with path.open("a+") as stream:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                stream.seek(0)
                raise EngineBusy(_read_pid(stream.read())) from None
            stream.seek(0)
            stream.truncate()
            stream.write(str(os.getpid()))
            stream.flush()
            try:
                yield
            finally:
                fcntl.flock(stream, fcntl.LOCK_UN)


def _ledger_value(value: str, limit: int) -> str:
    """`value` as one line of at most `limit` characters.

    Line breaks and tabs become visible `\\n` and `\\t`; terminal escape sequences and the other control
    characters are removed, as everywhere an agent's text reaches an operator's terminal.
    """
    from sakura.flow.render import strip_controls  # render imports this module, so the import cannot be at the top
    text = str(value)
    for character, escape in SIMPLE_ESCAPES.items():
        text = text.replace(character, escape)
    text = strip_controls(text)
    return text if len(text) <= limit else text[:limit - len(LEDGER_CUT)] + LEDGER_CUT


def _read_pid(text: str) -> int:
    return int(text) if text.strip().isdigit() else 0
