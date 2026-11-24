"""Codex readers: session rollouts under the sessions directory."""

import re
from datetime import datetime
from pathlib import Path

from sakura.usage import jsonl
from sakura.usage.model import Sample, Usage, UsageEvent

PROVIDER = "codex"
WEEK_MINUTES = 10080
SESSION_ID = re.compile(r"[0-9A-Za-z][0-9A-Za-z-]*")


def session_events(rollout: Path) -> list[UsageEvent]:
    """Per-line usage as differences between consecutive cumulative `token_count` totals."""
    previous = (0, 0, 0)
    events = []
    for payload, at in _token_counts(rollout):
        totals = cumulative_totals(payload.get("info"))
        if totals is None or totals == previous:
            continue
        if any(now < before for now, before in zip(totals, previous)):
            previous = (0, 0, 0)
        events.append(UsageEvent(at, delta_usage(totals, previous)))
        previous = totals
    return events


def samples(rollout: Path) -> list[Sample]:
    """Weekly utilization (0..1) from the primary rate limit of each `token_count` line."""
    found = (_sample(payload.get("rate_limits"), at) for payload, at in _token_counts(rollout))
    return [sample for sample in found if sample]


def rollout_for(sessions_dir: Path, session_id: str) -> Path | None:
    """The rollout file of a session id, or None; ids that are not plain tokens match nothing."""
    if not SESSION_ID.fullmatch(session_id):
        return None
    try:
        return next(iter(sorted(sessions_dir.glob(f"**/rollout-*-{session_id}.jsonl"))), None)
    except OSError:
        return None


def _token_counts(rollout: Path):
    for obj in jsonl.read_objects(rollout):
        payload, at = obj.get("payload"), jsonl.from_iso(obj.get("timestamp"))
        if isinstance(payload, dict) and payload.get("type") == "token_count" and at:
            yield payload, at


def cumulative_totals(info: object) -> tuple[int, int, int] | None:
    """(input, cached input, output) cumulative totals of a `token_count` info object, or None."""
    raw = info.get("total_token_usage") if isinstance(info, dict) else None
    if not isinstance(raw, dict):
        return None
    counts = [jsonl.count(raw.get(key)) for key in ("input_tokens", "cached_input_tokens", "output_tokens")]
    return None if None in counts else tuple(counts)


def delta_usage(totals: tuple[int, int, int], previous: tuple[int, int, int]) -> Usage:
    """Usage added between two cumulative totals."""
    added_input, added_cached, added_output = (now - before for now, before in zip(totals, previous))
    return Usage(max(added_input - added_cached, 0), 0, added_cached, added_output, None)


def _sample(limits: object, at: datetime) -> Sample | None:
    primary = limits.get("primary") if isinstance(limits, dict) else None
    if not isinstance(primary, dict) or primary.get("window_minutes") != WEEK_MINUTES:
        return None
    percent, resets_at = jsonl.number(primary.get("used_percent")), jsonl.from_epoch(primary.get("resets_at"))
    if percent is None or resets_at is None:
        return None
    return Sample(at, PROVIDER, "seven_day", percent / 100, resets_at)
