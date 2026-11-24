"""Turn rows from single transcript lines. Only numbers, ids, model, time, cwd and branch survive."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from typing import NamedTuple

from sakura.usage import claude, codex, jsonl
from sakura.usage.archive import legacy_limits

CLAUDE_MARKERS = (b'"usage"', b'"type":"assistant"')
LIMIT_MARKERS = (b'"quotaLimits"', b'"error":"rate_limit"')
UNKNOWN_WINDOW = "unknown"
CODEX_MARKERS = (b'"session_meta"', b'"turn_context"', b'"token_count"')
SYNTHETIC_MODEL = "<synthetic>"
DETACHED_BRANCH = "HEAD"
EPOCH_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
ZERO_TOTALS = (0, 0, 0)
RATE_LIMITS_MARKER = b'"rate_limits"'
CODEX_FIVE_HOUR_MINUTES, CODEX_WEEK_MINUTES = 300, 10080
CODEX_ALLOWED = "allowed"


class Turn(NamedTuple):
    """One assistant turn, in the column order of the `turns` table."""

    provider: str
    event_key: str
    account: str
    session_id: str
    agent_id: str | None
    ts: str
    cwd: str | None
    branch: str | None
    model: str | None
    input_tokens: int
    cache_write_tokens: int
    cache_read_tokens: int
    output_tokens: int
    cost_usd: float | None


class LimitEvent(NamedTuple):
    """One "limit reached" message, in the column order of the `limit_events` table."""

    account: str
    event_key: str
    ts: str
    window: str
    resets_at: str | None


class RateLimits(NamedTuple):
    """The plan limits on a Codex line, in the `quota_snapshots` column order after the account."""

    ts: str
    subscription_type: str | None
    status: str | None
    five_hour_pct: float | None
    five_hour_resets_at: str | None
    seven_day_pct: float | None
    seven_day_resets_at: str | None


def claude_turn(line: bytes, account: str, fallback_session: str) -> Turn | None:
    """The turn of a Claude transcript line, or None when it is not a countable assistant message."""
    if not all(marker in line for marker in CLAUDE_MARKERS):
        return None
    obj = _load(line)
    message = obj.get("message") if obj else None
    if not obj or obj.get("type") != "assistant" or not isinstance(message, dict) or _is_limit(obj):
        return None
    usage, at = claude.usage_of(message.get("usage"), None), jsonl.from_iso(obj.get("timestamp"))
    model = message.get("model")
    if not usage or not at or not isinstance(message.get("id"), str) or model == SYNTHETIC_MODEL:
        return None
    return Turn("claude", message["id"], account, _text(obj.get("sessionId")) or fallback_session,
                _text(obj.get("agentId")), _iso(at), _text(obj.get("cwd")), _branch(obj.get("gitBranch")),
                _text(model), usage.uncached_input, usage.cache_write, usage.cache_read, usage.output, None)


def limit_event(line: bytes, account: str) -> LimitEvent | None:
    """The limit event of a Claude transcript line (a rejected-request message), else None."""
    if not any(marker in line for marker in LIMIT_MARKERS):
        return None
    obj = _load(line)
    if not obj or not _is_limit(obj):
        return None
    key, at = _text(obj.get("uuid")) or _text(obj.get("requestId")), jsonl.from_iso(obj.get("timestamp"))
    if not key or not at:
        return None
    limits = obj.get("quotaLimits")
    if isinstance(limits, dict):
        return LimitEvent(account, key, _iso(at), _text(limits.get("rateLimitType")) or UNKNOWN_WINDOW,
                          epoch_iso(limits.get("resetsAt")))
    legacy = legacy_limits.legacy_window_and_reset(obj.get("message"), at)
    return LimitEvent(account, key, _iso(at), *legacy) if legacy else None


def codex_turn(line: bytes, offset: int, cursor: dict, account: str, fallback_session: str) -> Turn | None:
    """The turn of a Codex rollout line; `cursor` carries session facts and totals between lines and runs."""
    if not any(marker in line for marker in CODEX_MARKERS):
        return None
    obj = _load(line)
    payload = obj.get("payload") if obj else None
    if not isinstance(payload, dict):
        return None
    if obj.get("type") == "session_meta":
        _remember_session(cursor, payload)
    elif obj.get("type") == "turn_context":
        _remember_context(cursor, payload)
    elif payload.get("type") == "token_count":
        return _token_turn(obj, payload, offset, cursor, account, fallback_session)
    return None


def codex_rate_limits(line: bytes) -> RateLimits | None:
    """The plan limits of a Codex `token_count` line, None without a 5-hour or weekly window.

    Windows are told apart by `window_minutes`, whatever their position; other lengths are dropped. Only
    plan, status, the two percentages and their reset times are read, nothing else of the object.
    """
    if RATE_LIMITS_MARKER not in line:
        return None
    obj = _load(line)
    payload = obj.get("payload") if obj else None
    limits = payload.get("rate_limits") if isinstance(payload, dict) and payload.get("type") == "token_count" else None
    at = jsonl.from_iso(obj.get("timestamp")) if obj else None
    if not isinstance(limits, dict) or at is None:
        return None
    windows = {_window_minutes(raw): raw for raw in (limits.get("primary"), limits.get("secondary"))
               if isinstance(raw, dict)}
    five, week = _codex_window(windows.get(CODEX_FIVE_HOUR_MINUTES)), _codex_window(windows.get(CODEX_WEEK_MINUTES))
    if five == week == (None, None):
        return None
    reached = limits.get("rate_limit_reached_type")
    status = CODEX_ALLOWED if reached is None else _text(reached)
    return RateLimits(_iso(at), _text(limits.get("plan_type")), status, *five, *week)


def merge(first: Turn, other: Turn) -> Turn:
    """The first turn's identity with the per-field maximum of the token counts."""
    return first._replace(
        input_tokens=max(first.input_tokens, other.input_tokens),
        cache_write_tokens=max(first.cache_write_tokens, other.cache_write_tokens),
        cache_read_tokens=max(first.cache_read_tokens, other.cache_read_tokens),
        output_tokens=max(first.output_tokens, other.output_tokens))


def _window_minutes(window: dict) -> object:
    minutes = window.get("window_minutes")
    return minutes if isinstance(minutes, (int, float)) and not isinstance(minutes, bool) else None


def _codex_window(window: dict | None) -> tuple[float | None, str | None]:
    """Percent used and ISO reset time of one Codex window; an unusable percentage drops both."""
    used = window.get("used_percent") if window else None
    if not isinstance(used, (int, float)) or isinstance(used, bool):
        return None, None
    return round(used, 1), epoch_iso(window.get("resets_at"))


def _token_turn(obj: dict, payload: dict, offset: int, cursor: dict, account: str, fallback: str) -> Turn | None:
    totals, at = codex.cumulative_totals(payload.get("info")), jsonl.from_iso(obj.get("timestamp"))
    previous = tuple(cursor.get("totals", ZERO_TOTALS))
    if totals is None or at is None or totals == previous:
        return None
    if any(now < before for now, before in zip(totals, previous)):
        previous = ZERO_TOTALS
    cursor["totals"] = list(totals)
    usage = codex.delta_usage(totals, previous)
    session = cursor.get("session_id") or fallback
    return Turn("codex", f"{session}@{offset}", account, session, None, _iso(at), cursor.get("cwd"),
                cursor.get("branch"), cursor.get("model"), usage.uncached_input, 0, usage.cache_read,
                usage.output, None)


def _remember_session(cursor: dict, payload: dict) -> None:
    cursor["session_id"] = _text(payload.get("id")) or _text(payload.get("session_id")) or cursor.get("session_id")
    cursor["cwd"] = _text(payload.get("cwd")) or cursor.get("cwd")
    git = payload.get("git")
    cursor["branch"] = _branch(git.get("branch")) if isinstance(git, dict) else cursor.get("branch")


def _remember_context(cursor: dict, payload: dict) -> None:
    cursor["model"] = _text(payload.get("model")) or cursor.get("model")
    cursor["cwd"] = _text(payload.get("cwd")) or cursor.get("cwd")


def _is_limit(obj: dict) -> bool:
    return (obj.get("type") == "assistant" and obj.get("error") == "rate_limit"
            and obj.get("isApiErrorMessage") is True)


def epoch_iso(value: object) -> str | None:
    """An epoch-seconds number as ISO UTC with a Z suffix, None for anything else."""
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    try:
        return datetime.fromtimestamp(value, timezone.utc).strftime(EPOCH_FORMAT)
    except (OverflowError, OSError, ValueError):
        return None


def _load(line: bytes) -> dict | None:
    try:
        obj = json.loads(line)
    except (ValueError, RecursionError):
        return None
    return obj if isinstance(obj, dict) else None


def _text(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _branch(value: object) -> str | None:
    text = _text(value)
    return None if text in (None, DETACHED_BRANCH) else text


def _iso(moment) -> str:
    return moment.astimezone(timezone.utc).isoformat()
