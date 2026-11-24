"""The cockpit's proof secret and its used nonces, kept on disk so one-time use holds across processes.

The server and `sakura cockpit act` are separate processes of the same user, so neither can keep this in
memory. Both files are 0600 in a 0700 directory under the HOME the process sees, and every change happens
under an exclusive lock.
"""

from __future__ import annotations

import fcntl
import json
import os
import secrets
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

STATE_DIRECTORY = ".local/state/sakura/cockpit"
SECRET_FILE = "proof-secret"
NONCE_FILE = "proof-nonces.json"
LOCK_FILE = "proofs.lock"
SECRET_BYTES = 32
FILE_MODE, DIRECTORY_MODE = 0o600, 0o700


class ProofStoreError(OSError):
    """The secret or the used nonces cannot be trusted or written; nothing is confirmed until it is fixed."""


class ProofStore:
    """Secret and used nonces under one directory; `directory` defaults to the cockpit state under HOME."""

    def __init__(self, directory: Path | None = None) -> None:
        self._directory = Path(directory) if directory else Path.home() / STATE_DIRECTORY

    def secret(self) -> bytes:
        """Return the signing secret, creating it on first use. Raises ProofStoreError when it is damaged."""
        with self._locked():
            path = self._directory / SECRET_FILE
            if not path.exists():
                self._write(path, secrets.token_bytes(SECRET_BYTES))
            value = path.read_bytes()
        if len(value) != SECRET_BYTES:
            raise ProofStoreError(f"proof secret {path} is damaged; delete it to create a new one")
        return value

    def use_once(self, nonce: str, expiry: int, now: float) -> bool:
        """True the first time `nonce` is used; nonces are forgotten once their proof could no longer be shown."""
        with self._locked():
            used = {name: until for name, until in self._read_used().items() if until >= now}
            if nonce in used:
                return False
            used[nonce] = expiry
            self._write(self._directory / NONCE_FILE, json.dumps(used).encode())
            return True

    def count(self) -> int:
        """How many used nonces are remembered (for tests)."""
        with self._locked():
            return len(self._read_used())

    def _read_used(self) -> dict[str, int]:
        path = self._directory / NONCE_FILE
        if not path.exists():
            return {}
        try:
            used = json.loads(path.read_text())
            if isinstance(used, dict) and all(isinstance(value, int) for value in used.values()):
                return used
        except ValueError:
            pass
        raise ProofStoreError(f"used proofs file {path} is unreadable; delete it after checking the action log")

    @contextmanager
    def _locked(self) -> Iterator[None]:
        self._directory.mkdir(mode=DIRECTORY_MODE, parents=True, exist_ok=True)
        descriptor = os.open(self._directory / LOCK_FILE, os.O_RDWR | os.O_CREAT, FILE_MODE)
        with os.fdopen(descriptor, "r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield

    def _write(self, path: Path, payload: bytes) -> None:
        """Replace `path` atomically so a reader never sees half a file."""
        temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, FILE_MODE)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
