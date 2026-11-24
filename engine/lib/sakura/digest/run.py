"""One digest run: the order of the steps, the lock, the done-today mark and the status.

The steps are: run the sources, have the model condense what they found, write the files and the lane, and only then
move the cursors and set the mark. A day counts as done only when at least one source ran well and the outputs were
published: a run that fails in the middle, or finds every source failing, leaves the state as it was, so the next
scheduled trigger (wake or login) tries again and looks at the same things. Each attempt overwrites the day's file,
so the failed sources show; the lane is written only by a run that succeeds. One run at a time holds the lock; a scheduled run does nothing before the configured time or
when today is already done, and an on-demand run always runs.
"""

from __future__ import annotations

import fcntl
import os
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone, tzinfo

from sakura.digest import config, intake, publish, schema, state, summarize

LANE_PREFIX = "digest-"
_WRITE_FILES = publish.write_files  # the field named `publish` hides the module inside Steps


class RunRefused(RuntimeError):
    """Raised, with a one-line reason, when a run cannot be made: no sources, or the model or the writers failed."""


@dataclass(frozen=True)
class Steps:
    """The three steps of a run; tests replace them, the commands use these."""

    collect: Callable = intake.collect
    summarize: Callable = summarize.summarize
    publish: Callable = publish.publish
    write_files: Callable = _WRITE_FILES


@dataclass(frozen=True)
class Outcome:
    """What `run` did: `ran` is False for a skip, which says why; a run that ran says what it wrote."""

    ran: bool
    reason: str = ""
    published: publish.Published | None = None
    reports: tuple[schema.SourceReport, ...] = field(default_factory=tuple)


@dataclass(frozen=True)
class Status:
    """What `status` shows: the last finished run, the next scheduled one and each source's last good run."""

    at: str
    last_run_at: str
    last_date: str
    next_at: datetime
    is_pending_today: bool
    sources: tuple[tuple[str, str], ...]


def run(settings: config.DigestConfig, now: datetime, *, scheduled: bool, steps: Steps = Steps(),
        tz: tzinfo | None = None, pusher=None) -> Outcome:
    """Make one run at `now` (an aware time); raises RunRefused, ConfigError-free: the caller loaded the settings."""
    from sakura.connectors import migration, registry

    local = now.astimezone(tz)
    with _lock() as is_free:
        if not is_free:
            return Outcome(False, "another digest run is in progress")
        try:
            if migration.ready():
                return Outcome(False, "Morning summary owns this schedule")
        except registry.ConnectorError as error:
            raise RunRefused("Morning summary handoff cannot be read") from error
        from sakura.scheduled import migrate, store

        try:
            if migrate.owns_digest():
                raise RunRefused(migrate.MIGRATED)
        except store.Refused as error:
            raise RunRefused(str(error)) from error
        memory = state.load()
        if scheduled and (skip := _skip_reason(settings, memory, local)):
            return Outcome(False, skip)
        return _make(settings, memory, now.astimezone(timezone.utc), local, steps, pusher)


def status(settings: config.DigestConfig, now: datetime, *, tz: tzinfo | None = None) -> Status:
    """The last run, when the next scheduled one is due, and what each source remembers."""
    memory, local = state.load(), now.astimezone(tz)
    today = local.date().isoformat()
    due = datetime.combine(local.date(), datetime.min.time(), local.tzinfo) + timedelta(minutes=settings.minute_of_day)
    is_pending = memory.done_date != today and local >= due
    next_at = due + timedelta(days=1) if memory.done_date == today else due
    return Status(settings.at, memory.last_run_at, memory.done_date, next_at, is_pending,
                  tuple((source.id, memory.of(source.id).at) for source in settings.sources))


def is_running() -> bool:
    """Whether a digest run holds the lock now."""
    with _lock() as is_free:
        return not is_free


# The run

def _skip_reason(settings: config.DigestConfig, memory: state.State, local: datetime) -> str:
    if memory.done_date == local.date().isoformat():
        return f"already done today ({memory.done_date})"
    if local.hour * 60 + local.minute < settings.minute_of_day:
        return f"before {settings.at}"
    return ""


def _make(settings: config.DigestConfig, memory: state.State, now: datetime, local: datetime, steps: Steps,
          pusher) -> Outcome:
    if not settings.active_sources:
        raise RunRefused("no sources: add a [[source]] to the digest file")
    sinces = intake.sinces(settings, memory, now)
    results = summarize.fit(steps.collect(settings, memory, now))
    try:
        return _publish(settings, memory, now, local, steps, pusher, results, sinces)
    except RunRefused:
        _remember_windows(memory, sinces)
        raise


def _publish(settings: config.DigestConfig, memory: state.State, now: datetime, local: datetime, steps: Steps, pusher,
             results: list[intake.SourceResult], sinces: dict[str, str]) -> Outcome:
    window = schema.Window(min(sinces.values()), now.isoformat(timespec="seconds"))
    reports = [result.report for result in results]
    if not any(report.ok for report in reports):
        _record_failed_attempt(settings, local, window, results, steps)
    try:
        output = steps.summarize(results, settings, window)
    except summarize.SummarizeError as error:
        raise RunRefused(f"nothing was published: {error}") from error
    digest = _digest(settings, local, window, results, output)
    try:
        published = steps.publish(digest, settings.language, now, pusher=pusher)
    except publish.PublishError as error:
        raise RunRefused(str(error)) from error
    state.save(state.committed(memory, reports, {result.report.id: result.cursor for result in results}, now,
                               digest.date, sinces))
    return Outcome(True, "", published, digest.sources)


def _remember_windows(memory: state.State, sinces: dict[str, str]) -> None:
    """After a run that published nothing: sources with nothing saved keep the window they used."""
    kept = state.with_windows(memory, sinces)
    if kept != memory:
        state.save(kept)


def _record_failed_attempt(settings: config.DigestConfig, local: datetime, window: schema.Window,
                           results: list[intake.SourceResult], steps: Steps) -> None:
    """Write the day's file with the failed sources, and refuse: no model run, no lane, the day stays open."""
    digest = _digest(settings, local, window, results, schema.ModelOutput())
    try:
        steps.write_files(digest)
    except publish.PublishError as error:
        raise RunRefused(str(error)) from error
    failed = ", ".join(report.id for report in digest.sources)
    raise RunRefused(f"every source failed ({failed}); nothing was published and the day stays open, see "
                     f"{config.digest_folder() / config.LATEST_FILE}")


def _digest(settings: config.DigestConfig, local: datetime, window: schema.Window, results: list[intake.SourceResult],
            output: schema.ModelOutput) -> schema.Digest:
    date = local.date().isoformat()
    return schema.Digest(date, window.until, window, settings.model, f"{LANE_PREFIX}{date}",
                         tuple(result.report for result in results), output)


@contextmanager
def _lock() -> Iterator[bool]:
    """Hold the digest lock without waiting; yields False when another run holds it."""
    folder = state.ensure_folder()
    descriptor = os.open(folder / config.LOCK_FILE, os.O_RDWR | os.O_CREAT, config.FILE_MODE)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        yield True
    finally:
        os.close(descriptor)
