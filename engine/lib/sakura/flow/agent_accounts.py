"""The accounts a step may run on: Leeway's book, which is also where the steps' pins come from.

A step is pinned to a Leeway account id (`step_accounts`), and Leeway holds the person's names for them, so the book is the
one list of accounts the rules, the models and the pins agree on. When Leeway cannot be asked, the engine's own account
registry stands in, as it did before the book was the source."""

from __future__ import annotations

from pathlib import Path

from sakura.accounts import registry
from sakura.accounts.providers import GOOSE, SUBSCRIPTION_PROVIDERS
from sakura.accounts.registry import Account
from sakura.usage import leeway


def usable() -> list[Account]:
    """The accounts a step may run on, in the book's order; Leeway's hidden and removed ones are not among them."""
    try:
        rows = leeway.status()["accounts"]
    except leeway.LeewayError:
        return [*_registered(), *goose_accounts()]
    subscriptions = [Account(row["provider"], row["id"], _label(row), Path(row.get("config_dir") or ""), True, None, (), ())
            for row in rows if row["provider"] in SUBSCRIPTION_PROVIDERS]
    return [*subscriptions, *goose_accounts()]


def _label(row: dict) -> str:
    """Leeway's name for the account; a book row with none is named by its program, never by its id."""
    return str(row.get("label") or "").strip() or f"{row['provider'].capitalize()} account"


def _registered() -> list[Account]:
    try:
        return [account for account in registry.load_subscriptions().accounts if account.enabled]
    except registry.AccountsConfigError:
        return []

def goose_accounts() -> list[Account]:
    """Select enabled Goose references independently of subscription discovery."""
    try:
        stored = registry.load_stored(registry.default_path().with_name(registry.GOOSE_ACCOUNTS_FILE))
    except registry.AccountsConfigError:
        return []
    return [account for account in stored.accounts if account.enabled and account.provider == GOOSE] if stored else []
