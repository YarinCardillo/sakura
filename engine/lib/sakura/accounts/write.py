"""Atomic writes of `accounts.toml`: re-read under an exclusive lock, validate, write a temp file, replace.

The file is emitted whole from the model (stdlib has no TOML writer), so comments are not kept.
Recovery: a crash before `os.replace` leaves the old file untouched; a writer that waits on the lock
re-reads the registry once it gets it, so concurrent writers never lose each other's change.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import tempfile
from collections.abc import Callable, Iterator, Mapping
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path
from uuid import uuid4

from sakura.accounts import registry
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.accounts.registry import Account, AccountsConfigError

LOCK_SUFFIX = ".lock"
DIR_MODE = 0o700
TOML_ESCAPES = {"\\": "\\\\", '"': '\\"'}
FIRST_PRINTABLE = 0x20
DELETE = 0x7F


def add(path: Path, account: Account, env: Mapping[str, str] = os.environ) -> None:
    """Append `account`; refused when its `(provider, id)` or directory is already registered.

    Every write starts from the file at `path` (empty when there is none), never from detected or legacy
    accounts; a host whose usage.toml still holds account keys must run `accounts migrate` first.
    """
    _update(path, lambda accounts: (*accounts, account), env)


def register_goose(folder: Path, label: str, provider: str | None = None) -> Account:
    """Attach a configuration once, recovering the same account after an interrupted setup."""
    path = registry.default_path().with_name(registry.GOOSE_ACCOUNTS_FILE)
    folder = folder.resolve(strict=True)
    with locked(path):
        accounts = _stored_accounts(path, os.environ)
        for account in accounts:
            if account.provider != GOOSE or account.config_dir.resolve() != folder:
                continue
            if provider is not None and account.goose_provider not in (None, provider):
                raise AccountsConfigError("this Goose folder already names a different provider")
            if provider is None or account.goose_provider == provider:
                return account
            configured = replace(account, goose_provider=provider)
            _save(path, tuple(configured if row == account else row for row in accounts), os.environ)
            return configured
        account = Account(GOOSE, f"goose-{uuid4().hex}", label, folder, True, None, (), (), provider)
        _save(path, (*accounts, account), os.environ)
        return account


def remove(path: Path, provider: str, account_id: str) -> None:
    """Detach the registry reference, leaving the configuration directory and usage records intact."""
    def without(accounts: tuple[Account, ...]) -> tuple[Account, ...]:
        remaining = tuple(row for row in accounts if (row.provider, row.id) != (provider, account_id))
        if remaining == accounts:
            raise AccountsConfigError(f"no account {provider}/{account_id}")
        return remaining

    _update(path, without, os.environ)


def relabel(path: Path, provider: str, account_id: str, label: str, env: Mapping[str, str] = os.environ) -> None:
    """Change the display label of an account; its id never changes."""
    _update(path, _changing(provider, account_id, label=label), env)


def set_enabled(path: Path, provider: str, account_id: str, enabled: bool, env: Mapping[str, str] = os.environ) -> None:
    """Enable or disable an account; history and labels stay."""
    _update(path, _changing(provider, account_id, enabled=enabled), env)


def set_ceiling(path: Path, provider: str, account_id: str, reserve_week_pct: float | None,
                env: Mapping[str, str] = os.environ) -> None:
    """Set the weekly reserve ceiling of an account (0 to 100), or clear it with None."""
    _update(path, _changing(provider, account_id, reserve_week_pct=reserve_week_pct), env)


def add_plan(path: Path, provider: str, account_id: str, plan: str, since: str, estimated: bool,
             env: Mapping[str, str] = os.environ) -> None:
    """Add a plan period (`since` is an ISO date or datetime, read by the loader's rules) to an account.

    Refused when the account already has a period of the same plan starting at the same moment.
    """
    key = _snapshot_key(provider, account_id)
    [period] = registry.parse_plans([{"plan": plan, "since": since, "estimated": estimated}], key)

    def change(account: Account) -> Account:
        choices = ADAPTERS[provider].plan_choices
        if period.plan not in choices:
            raise AccountsConfigError(f"plan must be one of {', '.join(choices)} for {provider}, got {period.plan!r}")
        if any((p.plan, p.since) == (period.plan, period.since) for p in account.plans):
            raise AccountsConfigError(f"duplicate plan {period.plan!r} since {since} on {provider}/{account_id}")
        return replace(account, plans=(*account.plans, period))
    _update(path, _changing_account(provider, account_id, change), env)


def add_extra_usage(path: Path, provider: str, account_id: str, on: str, amount: float, currency: str, note: str,
                    env: Mapping[str, str] = os.environ) -> None:
    """Add an extra-usage purchase (`on` is an ISO date) to an account."""
    key = _snapshot_key(provider, account_id)
    [purchase] = registry.parse_extra_usage(
        [{"on": on, "amount": amount, "currency": currency, "note": note}], key)
    _update(path, _changing_account(
        provider, account_id, lambda account: replace(account, extra_usage=(*account.extra_usage, purchase))), env)


def emit(accounts: tuple[Account, ...]) -> str:
    """The TOML text for `accounts`, in a stable order; `registry.loads` reads it back to an equal model."""
    return "".join(_account_block(account) for account in accounts)


def _snapshot_key(provider: str, account_id: str) -> str:
    if provider not in ADAPTERS:
        raise AccountsConfigError(f"provider must be one of {tuple(ADAPTERS)}, got {provider!r}")
    return ADAPTERS[provider].snapshot_key(account_id)


def _changing(provider: str, account_id: str, **fields) -> Callable[[tuple[Account, ...]], tuple[Account, ...]]:
    return _changing_account(provider, account_id, lambda account: replace(account, **fields))


def _changing_account(provider: str, account_id: str,
                      change_one: Callable[[Account], Account]) -> Callable[[tuple[Account, ...]], tuple[Account, ...]]:
    def change(accounts: tuple[Account, ...]) -> tuple[Account, ...]:
        if not any((a.provider, a.id) == (provider, account_id) for a in accounts):
            raise AccountsConfigError(f"no account {provider}/{account_id}")
        return tuple(change_one(a) if (a.provider, a.id) == (provider, account_id) else a for a in accounts)
    return change


def _update(path: Path, change: Callable[[tuple[Account, ...]], tuple[Account, ...]], env: Mapping[str, str]) -> None:
    with locked(path):
        accounts = change(_stored_accounts(path, env))
        _save(path, accounts, env)


def _save(path: Path, accounts: tuple[Account, ...], env: Mapping[str, str]) -> None:
    text = emit(accounts)
    registry.loads(text, path, env)
    replace_atomically(path, text)


def _stored_accounts(path: Path, env: Mapping[str, str]) -> tuple[Account, ...]:
    stored = registry.load_stored(path, env)
    if stored is not None:
        return stored.accounts
    if path.name == registry.GOOSE_ACCOUNTS_FILE:
        return ()
    # Imported here: migrate builds on this module.
    from sakura.accounts import migrate

    usage_path = path.with_name(migrate.USAGE_FILE)
    if migrate.has_account_keys(usage_path):
        raise AccountsConfigError(f"{usage_path} still holds the accounts; run sakura accounts migrate first")
    return ()


@contextlib.contextmanager
def locked(target: Path) -> Iterator[None]:
    target.parent.parent.mkdir(parents=True, exist_ok=True)
    target.parent.mkdir(mode=DIR_MODE, exist_ok=True)
    with target.with_name(target.name + LOCK_SUFFIX).open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def replace_atomically(target: Path, text: str) -> None:
    descriptor, temp_name = tempfile.mkstemp(dir=target.parent, prefix=f"{target.name}.tmp-")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_name, target)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(temp_name)
        raise


def _account_block(account: Account) -> str:
    lines = ["[[accounts]]", f"provider = {quote(account.provider)}", f"id = {quote(account.id)}",
             f"label = {quote(account.label)}", f"config_dir = {quote(str(account.config_dir))}",
             f"enabled = {_boolean(account.enabled)}"]
    if account.reserve_week_pct is not None:
        lines.append(f"reserve_week_pct = {account.reserve_week_pct!r}")
    if account.goose_provider is not None:
        lines.append(f"provider_config = {{ provider = {quote(account.goose_provider)} }}")
    for plan in account.plans:
        lines += ["", "[[accounts.plans]]", f"plan = {quote(plan.plan)}", f"since = {_moment(plan.since)}",
                  f"estimated = {_boolean(plan.estimated)}"]
    for purchase in account.extra_usage:
        lines += ["", "[[accounts.extra_usage]]", f"on = {purchase.on.isoformat()}", f"amount = {purchase.amount!r}",
                  f"currency = {quote(purchase.currency)}", f"note = {quote(purchase.note)}"]
    return "\n".join(lines) + "\n\n"


def quote(value: str) -> str:
    return '"' + "".join(_escape(char) for char in value) + '"'


def _escape(char: str) -> str:
    if char in TOML_ESCAPES:
        return TOML_ESCAPES[char]
    if ord(char) < FIRST_PRINTABLE or ord(char) == DELETE:
        return f"\\u{ord(char):04X}"
    return char


def _boolean(value: bool) -> str:
    return "true" if value else "false"


def _moment(value: datetime) -> str:
    return value.astimezone(UTC).isoformat().replace("+00:00", "Z")
