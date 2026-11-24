"""Migration of the account keys of `usage.toml` (`sources`, `plans`, `extra_usage`) into `accounts.toml`.

One conversion serves both the explicit command and legacy mode: while no accounts.toml exists,
`registry.load` builds the registry in memory from the same keys, so a host keeps archiving under its own
ids until the owner migrates. Neither path opens the archive database.

Recovery: usage.toml is copied to `usage.toml.bak-<stamp>` first; accounts.toml is written before usage.toml
is rewritten, and if the rewrite fails the new accounts.toml is removed, so the state stays the old one.
A second run finds accounts.toml and changes nothing, except when usage.toml still holds account keys (the
crash window between the two writes): `--write` then backs up and rewrites usage.toml to finish the migration.
"""

from __future__ import annotations

import os
import shutil
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, replace
from datetime import UTC, datetime
from pathlib import Path

from sakura.accounts import registry, write
from sakura.accounts.providers import ADAPTERS, SUBSCRIPTION_PROVIDERS
from sakura.accounts.registry import LEGACY_ACCOUNT_KEYS, Account, AccountsConfigError

USAGE_FILE = "usage.toml"
LEGACY_DEFAULT_ID = "personal"
KEPT_KEYS = ("db", "claude_bin", "pr_repos", "pr_days", "plan_weights")
BACKUP_STAMP = "%Y%m%d-%H%M%S"
FILE_MODE = 0o600


@dataclass(frozen=True)
class _Migration:
    accounts_text: str
    usage_text: str
    kept_keys: tuple[str, ...]


def legacy_registry(usage_path: Path, env: Mapping[str, str]) -> registry.Registry | None:
    """The registry built in memory from the account keys of `usage_path`, None when it has none."""
    raw = _read_usage(usage_path)
    if raw is None or not LEGACY_ACCOUNT_KEYS & raw.keys():
        return None
    _, converted = _convert(raw, usage_path, _home(env))
    return replace(converted, legacy=True)


def has_account_keys(usage_path: Path) -> bool:
    """Whether `usage_path` still holds `sources`, `plans` or `extra_usage`."""
    raw = _read_usage(usage_path)
    return raw is not None and bool(LEGACY_ACCOUNT_KEYS & raw.keys())


def run_migrate(apply: bool, env: Mapping[str, str] = os.environ) -> int:
    """Print the accounts that `usage.toml` converts to (dry run), or with `apply` write them.

    Raises AccountsConfigError when usage.toml cannot be converted or written; nothing is changed then.
    """
    accounts_path = registry.default_path(env)
    usage_path = accounts_path.with_name(USAGE_FILE)
    if apply:
        with write.locked(accounts_path):
            return _migrate(accounts_path, usage_path, env, apply=True)
    return _migrate(accounts_path, usage_path, env, apply=False)


def _migrate(accounts_path: Path, usage_path: Path, env: Mapping[str, str], apply: bool) -> int:
    if accounts_path.exists():
        return _finish_interrupted(usage_path, _home(env), apply)
    migration = _prepare(usage_path, _home(env))
    if migration is None:
        print("nothing to migrate: usage.toml has no sources, plans or extra_usage")
    elif apply:
        _apply(migration, accounts_path, usage_path)
    else:
        print(migration.accounts_text, end="")
        print(f"usage.toml keeps: {', '.join(migration.kept_keys) or 'nothing'}")
    return 0


def _finish_interrupted(usage_path: Path, home: str, apply: bool) -> int:
    """accounts.toml exists: nothing to do, unless usage.toml still holds account keys (a crash between the writes)."""
    migration = _prepare(usage_path, home) if has_account_keys(usage_path) else None
    if migration is None:
        print("already migrated")
    elif apply:
        backup = _backup(usage_path)
        try:
            write.replace_atomically(usage_path, migration.usage_text)
        except OSError as error:
            raise AccountsConfigError(f"cannot rewrite {usage_path}: {error}; backup at {backup}") from error
        print(f"finished an interrupted migration: usage.toml rewritten without account keys; backup at {backup}")
    else:
        print("interrupted migration: accounts.toml exists but usage.toml still holds account keys; "
              "run with --write to finish it")
    return 0


def _prepare(usage_path: Path, home: str) -> _Migration | None:
    raw = _read_usage(usage_path)
    if raw is None:
        raise AccountsConfigError(f"{usage_path}: no usage config to migrate")
    if not LEGACY_ACCOUNT_KEYS & raw.keys():
        return None
    unknown = sorted(set(raw) - set(KEPT_KEYS) - LEGACY_ACCOUNT_KEYS)
    if unknown:
        raise AccountsConfigError(
            f"{usage_path}: cannot migrate, unknown keys {unknown} would be lost; move or remove them first")
    accounts_text, _ = _convert(raw, usage_path, home)
    kept = tuple(key for key in KEPT_KEYS if key in raw)
    return _Migration(accounts_text, _emit_kept(raw, kept, usage_path), kept)


def _apply(migration: _Migration, accounts_path: Path, usage_path: Path) -> None:
    backup = _backup(usage_path)
    try:
        write.replace_atomically(accounts_path, migration.accounts_text)
    except OSError as error:
        raise AccountsConfigError(f"cannot write {accounts_path}: {error}; nothing changed, backup at {backup}") from error
    try:
        write.replace_atomically(usage_path, migration.usage_text)
    except OSError as error:
        raise AccountsConfigError(
            f"cannot rewrite {usage_path}: {error}; {_remove_new(accounts_path)}, backup at {backup}") from error
    print(f"migrated to {accounts_path}; backup at {backup}")


def _backup(usage_path: Path) -> Path:
    stamp = datetime.now(UTC).strftime(BACKUP_STAMP)
    backup = usage_path.with_name(f"{usage_path.name}.bak-{stamp}")
    shutil.copy2(usage_path, backup)
    backup.chmod(FILE_MODE)
    return backup


def _remove_new(accounts_path: Path) -> str:
    try:
        accounts_path.unlink()
    except OSError as error:
        return f"could not remove the new {accounts_path} ({error}); delete it by hand"
    return "the new accounts.toml was removed, state unchanged"


def _read_usage(path: Path) -> dict | None:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except UnicodeDecodeError as error:
        raise AccountsConfigError(f"{path}: cannot read usage config: not valid UTF-8 ({error.reason})") from error
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise AccountsConfigError(f"{path}: cannot read usage config: {error}") from error


def _convert(raw: dict, usage_path: Path, home: str) -> tuple[str, registry.Registry]:
    """The accounts.toml text for the account keys of `raw` and the registry it loads back to."""
    try:
        accounts = _attach(_sources(raw.get("sources", []), home), raw)
    except AccountsConfigError as error:
        raise AccountsConfigError(f"{usage_path}: {error}") from error
    text = write.emit(accounts)
    return text, registry.loads(text, usage_path, {"HOME": home})


def _sources(raw: object, home: str) -> tuple[Account, ...]:
    if not isinstance(raw, list) or not all(isinstance(entry, dict) for entry in raw):
        raise AccountsConfigError("sources must be an array of tables")
    return tuple(_source(entry, home) for entry in raw)


def _source(entry: dict, home: str) -> Account:
    provider = entry.get("provider")
    if provider not in SUBSCRIPTION_PROVIDERS:
        raise AccountsConfigError(f"sources.provider must be one of {SUBSCRIPTION_PROVIDERS}, got {provider!r}")
    account_id = entry.get("account", LEGACY_DEFAULT_ID)
    if not isinstance(account_id, str):
        raise AccountsConfigError(f"sources.account must be a string, got {account_id!r}")
    root = registry.expand_home(_required(entry, "root", f"{provider}/{account_id}"), home)
    config_dir = _config_dir(entry, provider, account_id, root, home)
    return Account(provider, account_id, account_id, config_dir, True, None, (), ())


def _config_dir(entry: dict, provider: str, account_id: str, root: Path, home: str) -> Path:
    if "config_dir" in entry:
        return _explicit_config_dir(entry, provider, account_id, root, home)
    if root.name == ADAPTERS[provider].transcript_subdir:
        return root.parent
    raise AccountsConfigError(
        f"source {provider}/{account_id}: cannot derive its config directory from root {root} "
        f"(not a '{ADAPTERS[provider].transcript_subdir}' directory); add config_dir to the source")


def _explicit_config_dir(entry: dict, provider: str, account_id: str, root: Path, home: str) -> Path:
    config_dir = registry.expand_home(_required(entry, "config_dir", f"{provider}/{account_id}"), home)
    subdir = ADAPTERS[provider].transcript_subdir
    # The registry derives the transcript root from config_dir, so a different root would be scanned elsewhere.
    if root.resolve() != (config_dir / subdir).resolve():
        raise AccountsConfigError(
            f"source {provider}/{account_id}: root {root} is not {config_dir / subdir} "
            f"(config_dir plus '{subdir}'); the registry scans only there, so fix root or config_dir")
    return config_dir


def _required(entry: dict, key: str, owner: str) -> str:
    value = entry.get(key)
    if not isinstance(value, str) or not value:
        raise AccountsConfigError(f"source {owner}: {key} must be a non-empty string")
    return value


def _attach(accounts: tuple[Account, ...], raw: dict) -> tuple[Account, ...]:
    plans = registry.parse_plans(raw.get("plans", []))
    purchases = registry.parse_extra_usage(raw.get("extra_usage", []))
    claude_ids = {account.id for account in accounts if account.provider == "claude"}
    for owner, entries in (("plans", plans), ("extra_usage", purchases)):
        for entry in entries:
            if entry.account not in claude_ids:
                raise AccountsConfigError(f"{owner} name account {entry.account!r}, which has no claude source")
    return tuple(
        replace(account, plans=tuple(p for p in plans if p.account == account.id),
                extra_usage=tuple(e for e in purchases if e.account == account.id))
        if account.provider == "claude" else account for account in accounts)


def _emit_kept(raw: dict, kept: tuple[str, ...], usage_path: Path) -> str:
    lines = []
    for key in kept:
        try:
            lines += _kept_lines(key, raw[key])
        except (TypeError, ValueError) as error:
            raise AccountsConfigError(f"{usage_path}: {key}: {error}") from error
    return "\n".join(lines) + "\n"


def _kept_lines(key: str, value: object) -> list[str]:
    if key in ("db", "claude_bin"):
        return [f"{key} = {_string(value)}"]
    if key == "pr_repos":
        return [f"{key} = [{', '.join(_string(item) for item in _sequence(value))}]"]
    if key == "pr_days":
        return [f"{key} = {_number(value)}"]
    return ["", "[plan_weights]", *(f"{write.quote(name)} = {_number(weight)}" for name, weight in _table(value).items())]


def _string(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError("must be a string")
    return write.quote(value)


def _sequence(value: object) -> list:
    if not isinstance(value, list):
        raise TypeError("must be an array of strings")
    return value


def _table(value: object) -> dict:
    if not isinstance(value, dict):
        raise TypeError("must be a table of numbers")
    return value


def _number(value: object) -> str:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TypeError("must be a number")
    return repr(value)


def _home(env: Mapping[str, str]) -> str:
    return env.get("HOME") or os.path.expanduser("~")
