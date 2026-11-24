"""Plan quota snapshots: one minimal headless Claude Code request per account, read for its rate limit event.

Claude Code reads its own credentials through `CLAUDE_CONFIG_DIR`; this module never opens them. From
`claude auth status --json` only `subscriptionType` is kept, every other field is dropped unread.
"""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import tempfile
from collections.abc import Callable, Mapping
from dataclasses import astuple, dataclass
from datetime import datetime, timezone
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.usage.archive import records
from sakura.usage.archive.settings import Source

PROBE_TIMEOUT_SECONDS = 90
MISSING_BINARY_EXIT = 127
TIMEOUT_EXIT = 124
PERCENT = 100
EVENT_TYPE = "rate_limit_event"
# Credentials and routing that would make the probe bill an API key or leave the account's subscription.
UNSET_ENV = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL",
             "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX")
PROBE_FLAGS = ["-p", "ok", "--output-format", "stream-json", "--verbose", "--model", "haiku", "--tools", "",
               "--system-prompt", "Reply ok.", "--no-session-persistence", "--setting-sources", "",
               "--strict-mcp-config", "--disable-slash-commands"]
# Columns are named so that a column added by a later schema version cannot break a writer that lags behind it.
INSERT_SNAPSHOT = ("INSERT OR IGNORE INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                   " five_hour_resets_at, seven_day_pct, seven_day_resets_at, overage_in_use)"
                   " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")

Runner = Callable[[list[str], Mapping[str, str], Path], subprocess.CompletedProcess]
Clock = Callable[[], datetime]


class QuotaProbeError(RuntimeError):
    """The probe process could not run or timed out; the message is fixed text, never process output."""


@dataclass(frozen=True)
class Snapshot:
    """Quota state of one account at one moment, in the column order of `quota_snapshots`."""

    account: str
    ts: str
    subscription_type: str | None
    status: str | None
    five_hour_pct: float | None
    five_hour_resets_at: str | None
    seven_day_pct: float | None
    seven_day_resets_at: str | None
    overage_in_use: int | None


def subprocess_runner(argv: list[str], env: Mapping[str, str], cwd: Path) -> subprocess.CompletedProcess:
    """Run `argv` with stdin closed; a launch failure or a timeout becomes a failed result, never an exception."""
    try:
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True, env=dict(env),
                              cwd=cwd, timeout=PROBE_TIMEOUT_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, TIMEOUT_EXIT, "", "")
    except OSError:
        return subprocess.CompletedProcess(argv, MISSING_BINARY_EXIT, "", "")


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


def probe(source: Source, claude_bin: str, run: Runner, clock: Clock = utc_now) -> Snapshot | None:
    """The quota snapshot of the source's account, None when there is no config dir or no rate limit event.

    The snapshot is stamped with `clock()` read after the probe returned, so turns written while it ran
    fall before it.

    Raises QuotaProbeError when the probe process could not run at all. A non-zero exit still counts: a
    rejected request (HTTP 429) prints its rate limit event all the same.
    """
    if source.config_dir is None:
        return None
    env = _probe_env(source.config_dir)
    with tempfile.TemporaryDirectory(prefix="sakura-quota-") as workdir:
        plan = _subscription_type(run([claude_bin, "auth", "status", "--json"], env, Path(workdir)))
        done = run([claude_bin, *PROBE_FLAGS], env, Path(workdir))
    info = _last_event(done.stdout)
    if info is None:
        _raise_if_not_run(done)
        return None
    return _snapshot(source.account, clock(), plan, info)


def record(conn: sqlite3.Connection, snapshot: Snapshot) -> None:
    """Store the snapshot; a second one with the same account and time is ignored."""
    with conn:
        conn.execute(INSERT_SNAPSHOT, astuple(snapshot))


def _raise_if_not_run(done: subprocess.CompletedProcess) -> None:
    if done.returncode == MISSING_BINARY_EXIT:
        raise QuotaProbeError("probe could not run")
    if done.returncode == TIMEOUT_EXIT:
        raise QuotaProbeError("probe timed out")


def _subscription_type(done: subprocess.CompletedProcess) -> str | None:
    try:
        status = json.loads(done.stdout)
    except (TypeError, ValueError):
        return None
    value = status.get("subscriptionType") if isinstance(status, dict) else None
    return value if isinstance(value, str) and value else None


def _probe_env(config_dir: Path) -> dict[str, str]:
    """The environment without API keys, pointing Claude Code at `config_dir`."""
    return claude_config_env({key: value for key, value in os.environ.items() if key not in UNSET_ENV}, config_dir)


def claude_config_env(env: Mapping[str, str], config_dir: Path, *, home: Path | None = None) -> dict[str, str]:
    """`env` pointing Claude Code at `config_dir`.

    Claude Code keys its keychain login by CLAUDE_CONFIG_DIR, so the default `~/.claude` must stay unset:
    naming it explicitly looks logged out.
    """
    adapter = ADAPTERS["claude"]
    result = {key: value for key, value in env.items() if key != adapter.env_var}
    default_dir = (home or Path.home()) / adapter.default_dir.removeprefix("~/")
    if config_dir.expanduser().resolve() != default_dir.resolve():
        result[adapter.env_var] = str(config_dir)
    return result


def _last_event(stdout: str | None) -> dict | None:
    found = None
    for line in (stdout or "").splitlines():
        if EVENT_TYPE not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        info = obj.get("rate_limit_info") if isinstance(obj, dict) and obj.get("type") == EVENT_TYPE else None
        found = info if isinstance(info, dict) else found
    return found


def _snapshot(account: str, now: datetime, plan: str | None, info: dict) -> Snapshot:
    windows = info.get("unifiedWindows")
    windows = windows if isinstance(windows, dict) else {}
    five, seven = _window(windows.get("five_hour")), _window(windows.get("seven_day"))
    status = info.get("status")
    return Snapshot(account, now.astimezone(timezone.utc).isoformat(), plan,
                    status if isinstance(status, str) else None, *five, *seven, _overage(info.get("isUsingOverage")))


def _overage(value: object) -> int | None:
    """1 or 0 for a boolean `isUsingOverage`, None (unknown) for anything else."""
    return int(value) if isinstance(value, bool) else None


def _window(raw: object) -> tuple[float | None, str | None]:
    if not isinstance(raw, dict):
        return None, None
    return _percent(raw.get("utilization")), records.epoch_iso(raw.get("resetsAt"))


def _percent(value: object) -> float | None:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    return round(value * PERCENT, 1)
