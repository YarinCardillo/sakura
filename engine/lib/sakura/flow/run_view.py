"""What the lane page says about one run beyond its record: what it is doing now, and how a message reaches it.

Both are read-only views for `sakura flow sessions`: `doing` from the end of the run's log (never the whole log), and
`listening` from the run's state, its live channel and whether its conversation can still be continued.
"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path

from sakura import text_limits
from sakura.flow import conversations, feed, inbox, launchers, logread, roles, run_log, run_records, run_times
from sakura.flow.store import TaskStore

TAIL_BYTES = 128 * 1024
WIDE_TAIL_BYTES = 2 * 1024 * 1024  # a log whose end holds one huge tool result and nothing else
STARTING_GRACE_SECONDS = 120
LIVE, QUEUE, RESUME, NO = "live", "queue", "resume", "no"
MESSAGE_PROVIDERS = ("claude", "codex", "goose")


def effective(store: TaskStore, record: dict, now: datetime | None = None) -> dict:
    """`record` as the run stands: one recorded as running whose process is gone is `orphaned`, with the session id its
    log holds, so it can be continued. Nothing is written; the engine settles the record when it next starts.

    A run still `starting` is given `STARTING_GRACE_SECONDS` to be launched before it counts as gone."""
    if record["state"] not in run_records.LIVE_STATES or not _is_gone(record, now or datetime.now(timezone.utc)):
        return record
    return {**record, "state": "orphaned", "session_id": record.get("session_id") or run_records.session_id_of_run(store, record)}


def doing(store: TaskStore, record: dict) -> dict | None:
    """What the run is doing now: the tool it started and has not finished (only while the run is alive), else the first
    line of its last text block, else None when its log holds neither."""
    if record.get("acp_permission") and record["state"] in run_records.LIVE_STATES:
        return {"kind": "tool", "tool": "ACP permission", "detail": "Checking the tool grant",
                "since": record.get("started_at")}
    log = run_records.log_path(store, record["name"])
    scanned = _tail_scan(log, record)
    if scanned is None:
        return None
    if record["state"] in run_records.LIVE_STATES:
        running = [entry for entry in scanned.entries
                   if entry.kind == run_log.TOOL and entry.tool_id and entry.tool_id not in scanned.closed]
        if running:
            return _tool_doing(log, record, running[-1])
    texts = [entry for entry in scanned.entries if entry.kind == run_log.TEXT]
    line = feed.first_line(texts[-1].text) if texts else ""
    return {"kind": "text", "text": line} if line else None


def listening(store: TaskStore, state: dict, record: dict, common_dir: Path) -> tuple[str, str | None]:
    """How a message reaches the run now, and the words that say why none can: `live` (a running Claude run takes it
    now), `queue` (a running run takes it as a resume when it ends), `resume` (an ended run is continued with it), or
    `no` with a sentence."""
    if record["provider"] not in MESSAGE_PROVIDERS:
        return NO, conversations.NOT_AN_AGENT
    if record["state"] in run_records.LIVE_STATES:
        takes_live = record["provider"] == "claude" and (record["state"] == "starting" or inbox.is_live(store, record["name"]))
        return (LIVE, None) if takes_live else (QUEUE, None)
    problem = conversations.resume_problem(record, store, state, common_dir)
    return (NO, problem) if problem else (RESUME, None)


def _tail_scan(log: Path, record: dict) -> run_log.Scan | None:
    """The entries of the end of the log, read from its first whole line on; None when it cannot be read."""
    spec = roles.RoleSpec(record["provider"])
    try:
        size = logread.size_of(log)
        for window in (TAIL_BYTES, WIDE_TAIL_BYTES):
            start = max(0, size - window)
            data = logread.read_range(log, start, window)
            cut = data.find(b"\n") + 1 if start else 0
            scanned = run_log.scan(spec, data[cut:], base=start + cut)
            if scanned.entries or start == 0:
                return scanned
    except (OSError, logread.UnreadableLog):
        return None
    return scanned


def _tool_doing(log: Path, record: dict, entry: run_log.LogEntry) -> dict:
    shown = feed.shown_entry(record, entry, "")
    detail = (shown.extra.get("command") or shown.path) if shown else ""
    return {"kind": "tool", "tool": shown.tool if shown else entry.tool, "detail": detail, "since": _since(log, entry)}


def _since(log: Path, entry: run_log.LogEntry) -> str:
    """When the tool started: its mark, else the time the log was last written (a tool that is still running wrote it)."""
    moment = run_times.time_at(log, entry.end)
    if moment:
        return moment
    try:
        return text_limits.utc_text(datetime.fromtimestamp(log.stat().st_mtime, timezone.utc))
    except OSError:
        return ""


def _is_gone(record: dict, now: datetime) -> bool:
    if record["state"] == "running":
        return run_records.is_dead(record, launchers.entry_is_alive)
    started = datetime.fromisoformat(record["started_at"])
    return not record["identity"] and (now - started).total_seconds() > STARTING_GRACE_SECONDS
