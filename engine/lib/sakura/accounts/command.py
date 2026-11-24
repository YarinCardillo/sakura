"""The `sakura accounts` subcommands."""

from __future__ import annotations

import json
import os
from pathlib import Path

from sakura.accounts import detect, registry, write
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.accounts.registry import Account


def run_detect(env=os.environ) -> int:
    """Print every candidate directory with its proposed id and owner; writes nothing."""
    home = Path(env.get("HOME") or os.path.expanduser("~"))
    proposals = detect.propose(detect.detect(home, env), registry.load(env=env), home)
    if not proposals:
        print("no candidates")
    for proposal in proposals:
        candidate = proposal.candidate
        line = [candidate.provider, proposal.registered_as or proposal.proposed_id, str(candidate.config_dir), candidate.reason]
        if proposal.registered_as:
            line.append(f"[registered as {proposal.registered_as}]")
        print("  ".join(line))
    return 0


def run_list(env=os.environ, as_json: bool = False) -> int:
    """Print one line per account: provider, id, label, directory and state (ok, missing, disabled).

    With `as_json`, print one JSON array instead (see `_json_rows`); the registry is loaded first, so a broken
    accounts.toml raises before anything reaches stdout.
    """
    loaded = registry.load(env=env)
    if as_json:
        print(json.dumps(_json_rows(loaded), ensure_ascii=False))
        return 0
    if loaded.implicit:
        print("no accounts.toml (run sakura accounts detect)")
    for account in loaded.accounts:
        print("  ".join([account.provider, account.id, account.label, str(account.config_dir), registry.state_of(account)]))
    return 0


def _json_rows(loaded: registry.Registry) -> list[dict]:
    """Every loaded account as {provider, id, key, label, enabled, state}, by provider then id; no paths.

    `key` is the snapshot key, the value `budget_alerts.account` holds.
    """
    rows = [{"provider": account.provider, "id": account.id, "key": ADAPTERS[account.provider].snapshot_key(account.id),
             "label": account.label, "enabled": account.enabled, "state": registry.state_of(account)}
            for account in loaded.accounts]
    return sorted(rows, key=lambda row: (row["provider"], row["id"]))


def run_add(provider: str, account_id: str, config_dir: str, label: str | None, env=os.environ) -> int:
    """Register an account on an existing directory; a missing directory is created only by migration."""
    if provider == GOOSE:
        raise ValueError("register Goose through first-launch setup or the cockpit Accounts action")
    directory = Path(config_dir).expanduser().resolve()
    if not directory.is_dir():
        raise ValueError(f"{directory} is not a directory; accounts are added for directories that exist")
    write.add(registry.default_path(env), Account(provider, account_id, label or account_id, directory, True, None, (), ()), env)
    return 0


def run_rename(provider: str, account_id: str, label: str, env=os.environ) -> int:
    """Change an account's display label; the id never changes."""
    write.relabel(_account_path(provider, env), provider, account_id, label, env)
    return 0


def run_set_enabled(provider: str, account_id: str, enabled: bool, env=os.environ) -> int:
    """Enable or disable an account."""
    write.set_enabled(_account_path(provider, env), provider, account_id, enabled, env)
    return 0


def _account_path(provider: str, env) -> Path:
    native = registry.default_path(env)
    return native.with_name(registry.GOOSE_ACCOUNTS_FILE) if provider == GOOSE else native


def run_set_ceiling(provider: str, account_id: str, percent: str, env=os.environ) -> int:
    """Set the weekly reserve ceiling (0 to 100); the word `none` clears it."""
    write.set_ceiling(registry.default_path(env), provider, account_id, _ceiling(percent), env)
    return 0


def _ceiling(percent: str) -> float | None:
    if percent == "none":
        return None
    try:
        return float(percent)
    except ValueError:
        raise ValueError(f"ceiling must be a number from 0 to 100 or 'none', got {percent!r}") from None


def run_add_plan(provider: str, account_id: str, plan: str, since: str, estimated: bool, env=os.environ) -> int:
    """Add a plan period to an account; `since` is an ISO date or datetime (UTC when it has no offset)."""
    write.add_plan(registry.default_path(env), provider, account_id, plan, since, estimated, env)
    return 0


def run_add_extra(provider: str, account_id: str, on: str, amount: str, currency: str, note: str,
                  env=os.environ) -> int:
    """Add an extra-usage purchase to an account; `on` is an ISO date."""
    try:
        value = float(amount)
    except ValueError:
        raise ValueError(f"amount must be a non-negative number, got {amount!r}") from None
    write.add_extra_usage(registry.default_path(env), provider, account_id, on, value, currency, note, env)
    return 0
