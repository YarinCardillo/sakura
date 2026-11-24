"""Archive settings from the optional host config `~/.config/sakura/usage.toml`."""

from __future__ import annotations

import os
import tomllib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from types import MappingProxyType

from sakura.accounts import registry
from sakura.accounts.providers import (
    ADAPTERS,
    CODEX_ACCOUNT_PREFIX,
)
from sakura.accounts.registry import (
    ACCOUNTS_FILE,
    LEGACY_ACCOUNT_KEYS,
    AccountsConfigError,
    ExtraUsage,
    Plan,
)

# SQL condition on an `account` column that keeps Claude accounts only (calibration and reports are Claude-only).
CLAUDE_ACCOUNT_ONLY = f"account NOT LIKE '{CODEX_ACCOUNT_PREFIX}%'"
DEFAULT_PR_DAYS = 60
DEFAULT_CLAUDE_BIN = "claude"
DEFAULT_CONFIG = "~/.config/sakura/usage.toml"
DEFAULT_DB = "~/.local/state/sakura/usage.db"


class UsageConfigError(ValueError):
    """The usage config is malformed; the message names the offending key."""


@dataclass(frozen=True)
class Source:
    """A transcript root; `config_dir` is the account's config directory (the Claude quota probe runs under it)."""

    provider: str
    account: str
    root: Path
    config_dir: Path | None = None


@dataclass(frozen=True)
class Settings:
    db: Path
    sources: tuple[Source, ...]
    pr_repos: tuple[str, ...]
    pr_days: int
    plans: tuple[Plan, ...] = ()
    plan_weights: Mapping[str, float] = MappingProxyType({})
    claude_bin: str = DEFAULT_CLAUDE_BIN
    extra_usage: tuple[ExtraUsage, ...] = ()
    disabled: tuple[Source, ...] = ()
    reserves: Mapping[str, float] = MappingProxyType({})  # own weekly reserve by snapshot key, enabled accounts that set one


def period_at(plans: tuple[Plan, ...], account: str, at: datetime) -> Plan | None:
    """The plan period of the account at `at`: the latest one that began at or before it."""
    started = [plan for plan in plans if plan.account == account and plan.since <= at]
    return max(started, key=lambda plan: plan.since) if started else None


def plan_at(plans: tuple[Plan, ...], account: str, at: datetime) -> str | None:
    """The plan name of the account at `at`, None before its first period."""
    period = period_at(plans, account, at)
    return period.plan if period else None


def config_exists(env: Mapping[str, str] = os.environ) -> bool:
    """Whether accounts were configured under the HOME of `env`: an accounts.toml, or account keys still in usage.toml.

    A host with neither runs the archive on its detected directories but has no budget. A registry that
    cannot be read counts as configured, so the error surfaces where it is loaded.
    """
    try:
        return not registry.load_subscriptions(env=env).implicit
    except AccountsConfigError:
        return True


def db_path(path: Path | None = None, env: Mapping[str, str] = os.environ) -> Path:
    """Where the archive lives, from the host config alone: no registry is loaded."""
    home = env.get("HOME") or os.path.expanduser("~")
    raw = _read(path if path is not None else _expand(DEFAULT_CONFIG, home))
    return _expand(_text(raw, "db", DEFAULT_DB), home)


def load(path: Path | None = None, env: Mapping[str, str] = os.environ) -> Settings:
    """Settings from `path` (default config file) with defaults for anything missing."""
    home = env.get("HOME") or os.path.expanduser("~")
    config_path = path if path is not None else _expand(DEFAULT_CONFIG, home)
    raw = _read(config_path)
    _refuse_migrated_keys(raw, config_path)
    accounts = _registry(config_path, env).accounts
    return Settings(
        db=_expand(_text(raw, "db", DEFAULT_DB), home),
        sources=_sources(account for account in accounts if account.enabled),
        pr_repos=_repos(raw.get("pr_repos", [])),
        pr_days=_days(raw.get("pr_days", DEFAULT_PR_DAYS)),
        plans=tuple(plan for account in accounts for plan in account.plans),
        plan_weights=_weights(raw.get("plan_weights", {})),
        claude_bin=str(_expand(_text(raw, "claude_bin", DEFAULT_CLAUDE_BIN), home)),
        extra_usage=tuple(purchase for account in accounts for purchase in account.extra_usage),
        disabled=_sources(account for account in accounts if not account.enabled),
        reserves=_reserves(account for account in accounts if account.enabled),
    )


def _registry(config_path: Path, env: Mapping[str, str]) -> registry.Registry:
    try:
        return registry.load_subscriptions(config_path.with_name(ACCOUNTS_FILE), env)
    except AccountsConfigError as error:
        raise UsageConfigError(str(error)) from error


def _sources(accounts: Iterable[registry.Account]) -> tuple[Source, ...]:
    return tuple(Source(account.provider, account.id, account.config_dir / ADAPTERS[account.provider].transcript_subdir,
                        account.config_dir) for account in accounts)


def _reserves(accounts: Iterable[registry.Account]) -> Mapping[str, float]:
    return MappingProxyType({ADAPTERS[account.provider].snapshot_key(account.id): account.reserve_week_pct
                             for account in accounts if account.reserve_week_pct is not None})


def _read(path: Path) -> dict:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except UnicodeDecodeError as error:
        raise UsageConfigError(f"{path}: cannot read usage config: not valid UTF-8 ({error.reason})") from error
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise UsageConfigError(f"{path}: cannot read usage config: {error}") from error


def _refuse_migrated_keys(raw: dict, config_path: Path) -> None:
    if LEGACY_ACCOUNT_KEYS & raw.keys() and config_path.with_name(ACCOUNTS_FILE).exists():
        raise UsageConfigError(
            f"{config_path}: sources, plans and extra_usage now live in accounts.toml; run sakura accounts migrate")


def _weights(raw: object) -> Mapping[str, float]:
    numbers = (int, float)
    if not isinstance(raw, dict) or not all(
            isinstance(value, numbers) and not isinstance(value, bool) and value > 0 for value in raw.values()):
        raise UsageConfigError("plan_weights must map plan names to positive numbers")
    return MappingProxyType({name: float(value) for name, value in raw.items()})


def _repos(raw: object) -> tuple[str, ...]:
    if not isinstance(raw, list) or not all(isinstance(item, str) and "/" in item for item in raw):
        raise UsageConfigError("pr_repos must be a list of OWNER/NAME strings")
    return tuple(raw)


def _days(raw: object) -> int:
    if not isinstance(raw, int) or isinstance(raw, bool) or raw < 1:
        raise UsageConfigError("pr_days must be a positive integer")
    return raw


def _text(table: dict, key: str, default: str | None) -> str:
    value = table.get(key, default)
    if not isinstance(value, str) or not value:
        raise UsageConfigError(f"{key} must be a non-empty string")
    return value


def _expand(value: str, home: str) -> Path:
    if value == "~" or value.startswith("~/"):
        return Path(home) / value[2:]
    return Path(value)
