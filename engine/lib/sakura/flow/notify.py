"""Notification sinks for engine events."""

from __future__ import annotations

import http.client
import json
import subprocess
import time
import urllib.parse
import urllib.request
import uuid
from datetime import datetime
from pathlib import Path
from typing import Callable, Protocol

from sakura import programs
from sakura.flow import gates
from sakura.flow.config import FlowConfig
from sakura.flow.render import strip_controls
from sakura.flow.store import TaskStore

HTTP_TIMEOUT_SECONDS = 10
KEYCHAIN_TIMEOUT_SECONDS = 10
ESCALATION_EVENT = "escalation"
TRANSPORT_FAILURES = (OSError, ValueError, http.client.HTTPException)
TITLE_PREFIX = "sakura flow"
PUSHED, DEFERRED, FAILED = "pushed", "deferred to the day (night mode)", "failed"
NO_TOPIC = "skipped (no ntfy topic configured)"


class Notifier(Protocol):
    """Receives engine events that a person may want to see."""

    def notify(self, event: str, task: str, message: str) -> None: ...


class TopicUnavailable(RuntimeError):
    """Raised when the ntfy topic cannot be read from the Keychain; never carries the topic."""


class LedgerNotifier:
    """Writes every event into the task ledger."""

    def __init__(self, store: TaskStore) -> None:
        self._store = store

    def notify(self, event: str, task: str, message: str) -> None:
        """Append `notify <event>: <message>` to the ledger, as one line without terminal controls."""
        self._store.append_ledger("notify", f"notify {event}: {' '.join(strip_controls(message).split())}")


class NtfyTransport:
    """Posts whatever it is given to an ntfy topic held in the Keychain; knows nothing of day or night."""

    def __init__(self, keychain_service: str, url: str) -> None:
        self._service = keychain_service
        self._url = url.rstrip("/")

    def post(self, event: str, task: str, message: str) -> None:
        """POST `message` titled `sakura flow: <event> <task>`; raises TopicUnavailable or a network error."""
        topic = urllib.parse.quote(_read_topic(self._service), safe="")
        request = urllib.request.Request(
            f"{self._url}/{topic}", data=message.encode(), method="POST",
            headers={"Title": f"{TITLE_PREFIX}: {event} {task}"})
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_SECONDS):
            pass


class NotificationPolicy:
    """Ledgers every event once, then pages it by day or stores it as a deferred event by night.

    The mode is asked for at notify time. A transport failure is recorded as
    `notify failed: <event>`; the topic and the underlying error text are never
    recorded, since either may contain it.
    """

    def __init__(self, store: TaskStore, transport: NtfyTransport | None, mode: Callable[[], str],
                 deferred_dir: Path, day_start: str = "the day window") -> None:
        self._store = store
        self._ledger = LedgerNotifier(store)
        self._transport = transport
        self._mode = mode
        self._deferred_dir = deferred_dir
        self._day_start = day_start

    def notify(self, event: str, task: str, message: str) -> None:
        """Ledger the event; when it is a paged one, post it by day or defer it by night."""
        self._ledger.notify(event, task, message)
        if event not in gates.PAGED_EVENTS:
            return
        clean = strip_controls(message)
        if self._mode() == gates.DAY:
            self._page(event, task, clean)
        else:
            self._defer(event, task, clean)

    def _page(self, event: str, task: str, message: str) -> None:
        if self._transport is None:
            return
        try:
            self._transport.post(event, task, message)
        except (TopicUnavailable, *TRANSPORT_FAILURES):
            self._store.append_ledger("notify", f"notify failed: {event}")

    def _defer(self, event: str, task: str, message: str) -> None:
        if write_deferred(self._deferred_dir, self._store.repo.name, task, event, message):
            self._store.append_ledger("notify", f"notify deferred (night): {event}; night mode sends no phone page, "
                                                f"this one is kept and the cockpit lists it once the day starts at {self._day_start}")
        else:
            self._store.append_ledger("notify", f"notify defer failed: {event}")


class AlertPusher:
    """Pages an alert that belongs to no task: by day through ntfy, by night as a deferred event.

    The same day and night policy as `NotificationPolicy`, without a task ledger (the usage archive run has
    no repository). `push` returns what happened as text and never raises; a failure carries no error text,
    which could hold the topic.
    """

    def __init__(self, transport: NtfyTransport | None, mode: Callable[[], str], deferred_dir: Path) -> None:
        self._transport = transport
        self._mode = mode
        self._deferred_dir = deferred_dir

    def push(self, event: str, title: str, message: str) -> str:
        """Post `message` under `title` by day, defer it by night, skip it when no ntfy topic is configured."""
        if self._transport is None:
            return NO_TOPIC
        clean = strip_controls(message)
        if self._mode() != gates.DAY:
            return DEFERRED if write_deferred(self._deferred_dir, "", title, event, clean) else FAILED
        try:
            self._transport.post(event, title, clean)
        except (TopicUnavailable, *TRANSPORT_FAILURES):
            return FAILED
        return PUSHED


def write_deferred(deferred_dir: Path, repo: str, task: str, event: str, message: str) -> bool:
    """Store an event that would have paged during the night; False when the directory cannot be written."""
    entry = {"at": datetime.now().astimezone().isoformat(timespec="seconds"), "repo": repo,
             "task": task, "event": event, "message": message}
    target = deferred_dir / f"{time.time_ns()}-{uuid.uuid4().hex}.json"
    try:
        gates.write_atomically(target, json.dumps(entry).encode())
    except OSError:
        return False
    return True


def build_notifier(store: TaskStore, config: FlowConfig, *, mode: Callable[[], str] | None = None,
                   deferred_dir: Path | None = None) -> NotificationPolicy:
    """Return the policy: ledger always, ntfy when `notify.ntfy_keychain` names a Keychain service.

    `mode` and `deferred_dir` default to the real gate mode and the state directory; tests inject both.
    """
    day_start = config.values["gates"]["day"].split("-")[0]
    return NotificationPolicy(store, _transport_of(config), mode or _current_mode(config),
                              deferred_dir or gates.default_deferred_dir(), day_start)


def build_alert_pusher(config: FlowConfig, *, mode: Callable[[], str] | None = None,
                       deferred_dir: Path | None = None) -> AlertPusher:
    """The pusher for alerts of the usage archive run, from the (personal) `notify` and `gates` settings."""
    return AlertPusher(_transport_of(config), mode or _current_mode(config), deferred_dir or gates.default_deferred_dir())


def _transport_of(config: FlowConfig) -> NtfyTransport | None:
    settings = config.values["notify"]
    return NtfyTransport(settings["ntfy_keychain"], settings["ntfy_url"]) if settings["ntfy_keychain"] else None


def _current_mode(config: FlowConfig) -> Callable[[], str]:
    return lambda: gates.effective_mode(config, datetime.now(), gates.default_mode_file())[0]


def _read_topic(service: str) -> str:
    """Read the ntfy topic from the macOS Keychain; stdout is captured and never logged."""
    try:
        security = programs.path("security")
        done = subprocess.run([security, "find-generic-password", "-s", service, "-w"],
                              capture_output=True, text=True, timeout=KEYCHAIN_TIMEOUT_SECONDS, check=False,
                              env=programs.environment("system", security), stdin=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError, programs.ProgramError) as error:
        raise TopicUnavailable(f"cannot run security for service {service}") from error
    topic = done.stdout.strip()
    if done.returncode != 0 or not topic:
        raise TopicUnavailable(f"no ntfy topic in the Keychain service {service}")
    return topic
