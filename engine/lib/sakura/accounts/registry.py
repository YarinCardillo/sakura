"""The account registry: every provider account the engine knows, read from `~/.config/sakura/accounts.toml`.

An account is keyed by `(provider, id)`; the same id under two providers names one person or workspace.
Plans and extra usage live inside the account entry (`[[accounts.plans]]`, `[[accounts.extra_usage]]`) and are
keyed by the account's snapshot key (the bare id for Claude, `codex:<id>` for Codex), so a same-id pair never mixes.
"""

from __future__ import annotations

import os
import re
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, date, datetime
from pathlib import Path

from sakura.accounts.providers import ADAPTERS, GOOSE, GOOSE_LOCAL_PROVIDER

ACCOUNTS_FILE = "accounts.toml"
GOOSE_ACCOUNTS_FILE = "goose-accounts.toml"
DEFAULT_CONFIG = f"~/.config/sakura/{ACCOUNTS_FILE}"
# Keys that lived in usage.toml before the registry existed; `sakura accounts migrate` moves them.
LEGACY_ACCOUNT_KEYS = frozenset({"sources", "plans", "extra_usage"})
ID_PATTERN = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")
ACCOUNT_KEYS = frozenset({"provider", "id", "label", "config_dir", "enabled", "reserve_week_pct", "plans", "extra_usage",
                          "provider_config"})
PROVIDER_SLUG = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")
MAX_PERCENT = 100


class AccountsConfigError(ValueError):
    """The accounts registry is malformed; the message names the offending key."""


@dataclass(frozen=True)
class Plan:
    """The account was on `plan` from `since` (UTC) until the next period of the same account.

    `account` is the snapshot key of the owning account. `estimated` marks a period whose start date is a guess
    rather than a known switch date.
    """

    account: str
    plan: str
    since: datetime
    estimated: bool = False


@dataclass(frozen=True)
class ExtraUsage:
    """A purchase of extra usage on `on` (a UTC date), recorded in the config as the only source.

    `account` is the snapshot key of the owning account.
    """

    account: str
    on: date
    amount: float
    currency: str
    note: str = ""


@dataclass(frozen=True)
class Account:
    provider: str
    id: str
    label: str
    config_dir: Path
    enabled: bool
    reserve_week_pct: float | None
    plans: tuple[Plan, ...]
    extra_usage: tuple[ExtraUsage, ...]
    goose_provider: str | None = None


@dataclass(frozen=True)
class Registry:
    """The accounts in file order.

    `implicit` is True when nothing was chosen (no accounts.toml and no legacy keys): the accounts are then the
    directories found by the env var and the default dir of each provider. `legacy` is True when the accounts
    were built in memory from usage.toml because no accounts.toml exists yet.
    """

    accounts: tuple[Account, ...]
    implicit: bool
    legacy: bool = False


def state_of(account: Account) -> str:
    """`disabled`, else `ok` when the config directory exists and `missing` when it does not."""
    if not account.enabled:
        return "disabled"
    return "ok" if account.config_dir.is_dir() else "missing"


def default_path(env: Mapping[str, str] = os.environ) -> Path:
    """`~/.config/sakura/accounts.toml` under the HOME of `env`."""
    return expand_home(DEFAULT_CONFIG, env.get("HOME") or os.path.expanduser("~"))


def load(path: Path | None = None, env: Mapping[str, str] = os.environ) -> Registry:
    """The registry from `path` (default `~/.config/sakura/accounts.toml` under the HOME of `env`).

    Raises AccountsConfigError for an unreadable file, a bad entry, a duplicate `(provider, id)`
    or two accounts of one provider on the same directory. Without a file, the `sources`, `plans` and
    `extra_usage` of the usage.toml next to it build the registry in memory (legacy mode, ids kept); without
    those either, the registry is implicit and holds the env var and default directories that exist.
    """
    config_path = path if path is not None else default_path(env)
    native = load_subscriptions(config_path, env)
    goose = load_stored(config_path.with_name(GOOSE_ACCOUNTS_FILE), env)
    if goose is None:
        return native
    if any(account.provider != GOOSE for account in goose.accounts):
        raise AccountsConfigError("goose-accounts.toml accepts only Goose accounts")
    combined = (*native.accounts, *goose.accounts)
    _refuse_duplicates(combined)
    return Registry(combined, native.implicit, native.legacy)


def load_subscriptions(path: Path | None = None, env: Mapping[str, str] = os.environ) -> Registry:
    """Read Claude and Codex with their historical registry and fallback rules."""
    config_path = path if path is not None else default_path(env)
    stored = load_stored(config_path, env)
    if stored is None:
        return _without_file(config_path, env)
    if any(account.provider == GOOSE for account in stored.accounts):
        raise AccountsConfigError("store Goose accounts in goose-accounts.toml")
    return stored


def load_stored(config_path: Path, env: Mapping[str, str] = os.environ) -> Registry | None:
    """The registry written in the file at `config_path` (same errors as `load`), None when there is no file."""
    try:
        text = config_path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except UnicodeDecodeError as error:
        raise AccountsConfigError(f"{config_path}: cannot read accounts registry: not valid UTF-8 ({error.reason})") from error
    except OSError as error:
        raise AccountsConfigError(f"{config_path}: cannot read accounts registry: {error}") from error
    return loads(text, config_path, env)


def _without_file(config_path: Path, env: Mapping[str, str]) -> Registry:
    # Imported here: migrate and detect build accounts from this module's types.
    from sakura.accounts import detect, migrate

    legacy = migrate.legacy_registry(config_path.with_name(migrate.USAGE_FILE), env)
    return legacy if legacy is not None else Registry(accounts=detect.implicit_accounts(env), implicit=True)


def loads(text: str, source: Path, env: Mapping[str, str] = os.environ) -> Registry:
    """The registry in `text`, validated by the same rules as `load`; `source` names the file in errors."""
    home = env.get("HOME") or os.path.expanduser("~")
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise AccountsConfigError(f"{source}: cannot read accounts registry: {error}") from error
    try:
        accounts = _accounts(raw.get("accounts", []), home)
    except AccountsConfigError as error:
        raise AccountsConfigError(f"{source}: {error}") from error
    return Registry(accounts=accounts, implicit=False)


def parse_plans(raw: object, account: str | None = None) -> tuple[Plan, ...]:
    """Plans from an array of tables; each belongs to `account`, or names its own in an `account` key."""
    if not isinstance(raw, list) or not all(isinstance(entry, dict) for entry in raw):
        raise AccountsConfigError("plans must be an array of tables")
    return tuple(Plan(account or _text(entry, "account", None), _text(entry, "plan", None),
                      _moment(entry.get("since")), _flag(entry, "estimated")) for entry in raw)


def parse_extra_usage(raw: object, account: str | None = None) -> tuple[ExtraUsage, ...]:
    """Extra usage purchases from an array of tables; owned like `parse_plans`."""
    if not isinstance(raw, list) or not all(isinstance(entry, dict) for entry in raw):
        raise AccountsConfigError("extra_usage must be an array of tables")
    return tuple(ExtraUsage(account or _text(entry, "account", None), _day(entry.get("on")),
                            _amount(entry.get("amount")), _text(entry, "currency", None),
                            _note(entry.get("note", ""))) for entry in raw)


def _accounts(raw: object, home: str) -> tuple[Account, ...]:
    if not isinstance(raw, list) or not all(isinstance(entry, dict) for entry in raw):
        raise AccountsConfigError("accounts must be an array of tables")
    accounts = tuple(_account(entry, home) for entry in raw)
    _refuse_duplicates(accounts)
    return accounts


def _account(entry: dict, home: str) -> Account:
    unknown = sorted(set(entry) - ACCOUNT_KEYS)
    if unknown:
        raise AccountsConfigError(f"accounts has unknown keys {unknown}")
    provider = _text(entry, "provider", None)
    if provider not in ADAPTERS:
        raise AccountsConfigError(f"accounts.provider must be one of {tuple(ADAPTERS)}, got {provider!r}")
    account_id = _account_id(entry)
    upstream = _upstream(entry, provider)
    snapshot_key = ADAPTERS[provider].snapshot_key(account_id)
    try:
        return Account(
            provider=provider,
            id=account_id,
            label=_text(entry, "label", account_id),
            config_dir=expand_home(_text(entry, "config_dir", ADAPTERS[provider].default_dir), home),
            enabled=_boolean(entry, "enabled", True),
            reserve_week_pct=_percent(entry.get("reserve_week_pct")),
            plans=tuple(sorted(parse_plans(entry.get("plans", []), snapshot_key), key=lambda plan: plan.since)),
            extra_usage=parse_extra_usage(entry.get("extra_usage", []), snapshot_key),
            goose_provider=upstream,
        )
    except AccountsConfigError as error:
        raise AccountsConfigError(f"account {provider}/{account_id}: {error}") from error


def provider_config(value: object) -> str:
    """Validate a Goose provider reference that contains only the upstream slug."""
    if (not isinstance(value, dict) or value.keys() != {"provider"} or not isinstance(value["provider"], str)
            or not PROVIDER_SLUG.fullmatch(value["provider"])):
        raise AccountsConfigError("provider_config must contain only a provider slug")
    return value["provider"]


def _upstream(entry: dict, provider: str) -> str | None:
    if provider != GOOSE:
        if "provider_config" in entry:
            raise AccountsConfigError("provider_config is available for Goose accounts")
        return None
    if "reserve_week_pct" in entry or entry.get("plans") or entry.get("extra_usage"):
        raise AccountsConfigError("Goose accounts have no subscription usage settings")
    return provider_config(entry["provider_config"]) if "provider_config" in entry else None


def _account_id(entry: dict) -> str:
    account_id = entry.get("id")
    if not isinstance(account_id, str) or not ID_PATTERN.fullmatch(account_id):
        raise AccountsConfigError(f"accounts.id must be a slug ([a-z0-9][a-z0-9-]{{0,39}}), got {account_id!r}")
    return account_id


def _refuse_duplicates(accounts: tuple[Account, ...]) -> None:
    seen_keys: dict[tuple[str, str], Account] = {}
    seen_dirs: dict[tuple[str, Path], Account] = {}
    goose_slots: set[str] = set()
    for account in accounts:
        key = (account.provider, account.id)
        if key in seen_keys:
            raise AccountsConfigError(f"duplicate account {account.provider}/{account.id}")
        seen_keys[key] = account
        directory = (account.provider, account.config_dir.resolve())
        if directory in seen_dirs:
            raise AccountsConfigError(
                f"{account.provider} accounts {seen_dirs[directory].id!r} and {account.id!r} "
                f"share the config directory {account.config_dir}")
        seen_dirs[directory] = account
        if account.provider == GOOSE:
            # Goose v1.60.2 uses one Keychain service across all config roots.
            slot = "local Ollama endpoint" if account.goose_provider == GOOSE_LOCAL_PROVIDER else "Keychain slot"
            if slot in goose_slots:
                raise AccountsConfigError(f"Goose accounts share the same {slot}")
            goose_slots.add(slot)


def _percent(raw: object) -> float | None:
    if raw is None:
        return None
    if not isinstance(raw, (int, float)) or isinstance(raw, bool) or not 0 <= raw <= MAX_PERCENT:
        raise AccountsConfigError(f"accounts.reserve_week_pct must be a number from 0 to {MAX_PERCENT}")
    return float(raw)


def _boolean(table: dict, key: str, default: bool) -> bool:
    value = table.get(key, default)
    if not isinstance(value, bool):
        raise AccountsConfigError(f"accounts.{key} must be true or false")
    return value


def _flag(table: dict, key: str) -> bool:
    value = table.get(key, False)
    if not isinstance(value, bool):
        raise AccountsConfigError(f"plans.{key} must be true or false")
    return value


def _moment(raw: object) -> datetime:
    """A plan start: a TOML date or datetime, or an ISO string; naive values are UTC."""
    try:
        moment = datetime(raw.year, raw.month, raw.day, tzinfo=UTC) if type(raw) is date else (
            raw if isinstance(raw, datetime) else datetime.fromisoformat(raw))
    except (TypeError, ValueError) as error:
        raise AccountsConfigError(f"plans.since must be an ISO date or datetime, got {raw!r}") from error
    return moment.replace(tzinfo=UTC) if moment.tzinfo is None else moment.astimezone(UTC)


def _day(raw: object) -> date:
    try:
        if isinstance(raw, datetime):
            return raw.astimezone(UTC).date() if raw.tzinfo else raw.date()
        return raw if isinstance(raw, date) else datetime.fromisoformat(raw).date()
    except (TypeError, ValueError) as error:
        raise AccountsConfigError(f"extra_usage.on must be an ISO date, got {raw!r}") from error


def _amount(raw: object) -> float:
    if not isinstance(raw, (int, float)) or isinstance(raw, bool) or raw < 0:
        raise AccountsConfigError("extra_usage.amount must be a non-negative number")
    return float(raw)


def _note(raw: object) -> str:
    if not isinstance(raw, str):
        raise AccountsConfigError("extra_usage.note must be a string")
    return raw


def _text(table: dict, key: str, default: str | None) -> str:
    value = table.get(key, default)
    if not isinstance(value, str) or not value:
        raise AccountsConfigError(f"{key} must be a non-empty string")
    return value


def expand_home(value: str, home: str) -> Path:
    if value == "~" or value.startswith("~/"):
        return Path(home) / value[2:]
    return Path(value)
