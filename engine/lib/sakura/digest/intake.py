"""The intake: run each source, bound what it prints, and report what happened per source.

A `run` source is the person's own program. It runs outside the cage, as the person, with its arguments exactly as
configured, stdin closed, a working folder of its own, a timeout and a cap on its output; it prints one JSON object
per line (an item, or `{"cursor": "..."}`). A `feed` source is fetched and read by the engine (`feed.py`). Whatever the
source returns is untrusted text: lines that are not valid items are counted and skipped, and a failing source is
reported while the others still run.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime, timedelta

from sakura import programs, text_limits
from sakura.connectors import kinds, registry
from sakura.digest import config, feed, installation, schema, state as digest_state
from sakura.digest.supervise import Execution, run_program
from sakura.digest import supervise

SINCE_VARIABLE = "SAKURA_DIGEST_SINCE"
CURSOR_VARIABLE = "SAKURA_DIGEST_CURSOR"
# A program's own diagnostics may name what it was talking to (an address with a token in it): they are never published.
DIAGNOSTICS_NOTE = "see the program's own output for the reason"
STDERR_TAIL_BYTES = supervise.STDERR_TAIL_BYTES
TOO_MUCH_NEW = "too much new: narrow this source"


@dataclass(frozen=True)
class SourceResult:
    """What one source gave: its report, its items and the cursor it returned ("" when it returned none)."""

    report: schema.SourceReport
    items: tuple[schema.Item, ...] = ()
    cursor: str = ""


Runner = Callable[[tuple[str, ...], Mapping[str, str], int], Execution]
# Reads a github, notion or linear connector: (connector, since, cursor, timeout) -> kinds.Fetched
Connect = Callable[[registry.Connector, str, str, int], kinds.Fetched]
Lookup = Callable[[str], registry.Connector | None]


def collect(settings: config.DigestConfig, memory: digest_state.State, now: datetime, *, run: Runner | None = None,
            fetch: feed.Fetcher = feed.fetch, connect: Connect | None = None, record_use: bool = True,
            lookup: Lookup = registry.find) -> list[SourceResult]:
    """Run every source that is switched on, in order; each result is independent, so one failure never stops the others."""
    return [run_source(source, _since(settings, memory, source, now), memory.of(source.id).cursor, run=run,
                       fetch=fetch, connect=connect, now=now if record_use else None, lookup=lookup) for source in settings.active_sources]


def sinces(settings: config.DigestConfig, memory: digest_state.State, now: datetime) -> dict[str, str]:
    """The start of the window each source that is switched on looks at in a run at `now`."""
    return {source.id: _since(settings, memory, source, now) for source in settings.active_sources}


def run_source(source: config.Source, since: str, cursor: str, *, run: Runner | None = None,
               fetch: feed.Fetcher = feed.fetch, connect: Connect | None = None,
               now: datetime | None = None, lookup: Lookup = registry.find) -> SourceResult:
    """Run one source; never raises for what the source does."""
    if source.connector:
        return _from_connector(source, since, cursor, run, fetch, connect, now, lookup)
    if source.feed:
        return _from_feed(source, since, cursor, fetch)
    return _from_program(source, since, cursor, run or run_program)


def parse_output(output: bytes) -> tuple[list[schema.Item], str, int]:
    """The items, the last cursor and the number of skipped lines of a program's output (one JSON object per line).

    At most `schema.MAX_ITEMS_PER_SOURCE` items are kept; see `_parse_lines` for the ones beyond."""
    items, cursor, skipped, _overflow = _parse_lines(output)
    return items, cursor, skipped


# Programs

def _from_program(source: config.Source, since: str, cursor: str, run: Runner,
                  approved_identity: str | None = None) -> SourceResult:
    try:
        program = installation.inspect(source.run)
        if approved_identity is not None and program.identity() != approved_identity:
            return _failed(source, installation.CHANGED)
        environment = {**programs.environment("system", program.argv[0]), SINCE_VARIABLE: since, CURSOR_VARIABLE: cursor}
        program.verify()
    except installation.InstallationError as error:
        return _failed(source, str(error))
    outcome = run(program.argv, environment, source.timeout_seconds)
    if outcome.start_error:
        return _failed(source, outcome.start_error)
    if outcome.timed_out:
        return _failed(source, f"timed out after {source.timeout_seconds} seconds")
    if outcome.over_limit:
        return _failed(source, "printed more than its allowed bytes on standard output or error")
    if outcome.exit_code != 0:
        return _failed(source, _exit_text(outcome))
    items, new_cursor, skipped, overflow = _parse_lines(outcome.stdout)
    return _delivered(source, items, new_cursor, skipped, overflow)


def _parse_lines(output: bytes) -> tuple[list[schema.Item], str, int, int]:
    """The items kept, the last cursor, the lines skipped as invalid or repeated, and the valid items beyond the limit."""
    items: list[schema.Item] = []
    seen: set[str] = set()
    cursor, skipped, overflow = "", 0, 0
    for raw in output.splitlines():
        if not raw.strip():
            continue
        found = _line(raw)
        if isinstance(found, str):
            cursor = found
        elif found is None or found.id in seen:
            skipped += 1
        elif len(items) >= schema.MAX_ITEMS_PER_SOURCE:
            overflow += 1
        else:
            seen.add(found.id)
            items.append(found)
    return items, cursor, skipped, overflow


def _delivered(source: config.Source, items: list[schema.Item], cursor: str, skipped: int, overflow: int) -> SourceResult:
    """The source's result. A source that gave more than it may delivers nothing this run (a partial batch would
    either repeat or lose items, whichever way its cursor moved): its report says so and its state does not move."""
    if overflow:
        return too_much_new(source.id, skipped, len(items) + overflow, schema.MAX_ITEMS_PER_SOURCE)
    return SourceResult(schema.SourceReport(source.id, True, len(items), skipped), tuple(items), cursor)


def too_much_new(source_id: str, skipped: int, count: int, limit: int) -> SourceResult:
    """The result of a source withheld whole for being over a limit of `limit` items (or bytes of input)."""
    reason = f"{TOO_MUCH_NEW} ({count} items, the limit is {limit})"
    return SourceResult(schema.SourceReport(source_id, False, 0, skipped, reason))


def _line(raw: bytes) -> schema.Item | str | None:
    """An item, a cursor (as text), or None for a line that is neither."""
    try:
        value = json.loads(raw.decode("utf-8"))
    except (ValueError, RecursionError):
        return None
    if isinstance(value, dict) and set(value) == {"cursor"}:
        text = value["cursor"]
        is_valid = isinstance(text, str) and not text_limits.one_line_problem(text, digest_state.MAX_CURSOR)
        return text if is_valid else None
    try:
        return schema.parse_item(value)
    except schema.SchemaError:
        return None


def _exit_text(outcome: Execution) -> str:
    return f"exited with status {outcome.exit_code}; {DIAGNOSTICS_NOTE}"


# Connectors

def _from_connector(source: config.Source, since: str, cursor: str, run: Runner | None, fetch: feed.Fetcher,
                    connect: Connect | None, now: datetime | None, lookup: Lookup) -> SourceResult:
    found = lookup(source.connector)
    if found is None or found.revoked:
        return _failed(source, f"connector {source.connector} is not connected")
    if found.kind in kinds.READ_BY_DIGEST and not found.selectors:
        as_source = config.Source(source.id, feed=found.target if found.kind == kinds.FEED else "",
                                  run=(found.target, *found.arguments) if found.kind == kinds.PROGRAM else (),
                                  timeout_seconds=source.timeout_seconds)
        if found.kind == kinds.PROGRAM:
            result = _from_program(as_source, since, cursor, run or run_program, found.program_identity)
        else:
            result = _from_feed(as_source, since, cursor, fetch)
        _note_use(found, now, result.report.ok, result.report.error)
        return result
    from sakura.connectors import replacement

    memory = replacement.checkpoint(found, source.id, digest_state.SourceState(cursor, since))
    try:
        if connect is not None:
            fetched = connect(found, since, memory.cursor, source.timeout_seconds)
        else:
            ports = kinds.Ports(run=run or run_program, fetch_feed=fetch)
            fetched = kinds.read(found, since, memory.cursor, source.timeout_seconds, ports)
    except kinds.ConnectorError as error:
        _note_use(found, now, False, str(error))
        return _failed(source, str(error))
    _note_use(found, now, True, "")
    return _delivered(source, list(fetched.items[:schema.MAX_ITEMS_PER_SOURCE]), fetched.cursor, fetched.skipped,
                      max(0, len(fetched.items) - schema.MAX_ITEMS_PER_SOURCE))


def _note_use(found: registry.Connector, now: datetime | None, is_ok: bool, detail: str) -> None:
    if now is not None:
        registry.mark_used(found.id, now, revision=found.revision, is_ok=is_ok, detail=detail)


# Feeds

def _from_feed(source: config.Source, since: str, cursor: str, fetch: feed.Fetcher) -> SourceResult:
    try:
        found, skipped, newest = feed.entries(fetch(source.feed, source.timeout_seconds), since, cursor)
    except feed.FeedError as error:
        return _failed(source, str(error))
    items = [schema.parse_item(document) for document in found]
    return _delivered(source, items[:schema.MAX_ITEMS_PER_SOURCE], newest, skipped,
                      max(0, len(items) - schema.MAX_ITEMS_PER_SOURCE))


def _failed(source: config.Source, reason: str) -> SourceResult:
    return SourceResult(schema.SourceReport(source.id, False, 0, 0, reason[:schema.MAX_ERROR]))


# Time

def _since(settings: config.DigestConfig, memory: digest_state.State, source: config.Source, now: datetime) -> str:
    return memory.of(source.id).at or _lookback(settings, now)


def _lookback(settings: config.DigestConfig, now: datetime) -> str:
    return text_limits.utc_text(now - timedelta(hours=settings.lookback_hours))
