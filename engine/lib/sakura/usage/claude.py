"""Claude readers: flow run logs (stream-json) and interactive session transcripts."""

from datetime import datetime
from pathlib import Path

from sakura.usage import jsonl
from sakura.usage.model import Sample, Usage, UsageEvent

PROVIDER = "claude"
WINDOWS = ("five_hour", "seven_day")


def run_usage(log: Path, ended_at: datetime) -> UsageEvent | None:
    """Usage of the last `result` event of a flow run log, stamped with the run's end."""
    found = None
    for obj in jsonl.read_objects(log):
        if obj.get("type") == "result":
            usage = usage_of(obj.get("usage"), jsonl.number(obj.get("total_cost_usd")))
            found = usage or found
    return UsageEvent(ended_at, found) if found else None


def run_samples(log: Path, at: datetime) -> list[Sample]:
    """Utilization of both windows from the last `rate_limit_event` of a flow run log."""
    windows: dict = {}
    for obj in jsonl.read_objects(log):
        if obj.get("type") == "rate_limit_event":
            info = obj.get("rate_limit_info")
            found = info.get("unifiedWindows") if isinstance(info, dict) else None
            windows = found if isinstance(found, dict) else windows
    samples = (_sample(at, name, windows.get(name)) for name in WINDOWS)
    return [sample for sample in samples if sample]


def transcript_events(path: Path) -> list[UsageEvent]:
    """One event per assistant message of a transcript, deduplicated by message id."""
    seen: set[str] = set()
    events = []
    for obj in jsonl.read_objects(path):
        message = obj.get("message")
        if not isinstance(message, dict) or message.get("id") in seen:
            continue
        usage, at = usage_of(message.get("usage"), None), jsonl.from_iso(obj.get("timestamp"))
        if usage and at and isinstance(message.get("id"), str):
            seen.add(message["id"])
            events.append(UsageEvent(at, usage))
    return events


def usage_of(raw: object, cost_usd: float | None) -> Usage | None:
    """Usage of a Claude `usage` object, or None when the input or output count is missing."""
    if not isinstance(raw, dict):
        return None
    uncached = jsonl.count(raw.get("input_tokens"))
    output = jsonl.count(raw.get("output_tokens"))
    if uncached is None or output is None:
        return None
    return Usage(uncached, jsonl.count(raw.get("cache_creation_input_tokens")) or 0,
                 jsonl.count(raw.get("cache_read_input_tokens")) or 0, output, cost_usd)


def _sample(at: datetime, window: str, raw: object) -> Sample | None:
    if not isinstance(raw, dict):
        return None
    utilization = jsonl.number(raw.get("utilization"))
    resets_at = jsonl.from_epoch(raw.get("resetsAt"))
    if utilization is None or resets_at is None:
        return None
    return Sample(at, PROVIDER, window, utilization, resets_at)
