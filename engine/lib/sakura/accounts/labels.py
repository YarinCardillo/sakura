"""Display names of accounts, resolved at read time so a rename shows on every surface at once, old rows included.

Stored rows and alerts keep the account id (a snapshot key: the bare id of a Claude account, `codex:<id>` for a
Codex one). A key no account owns any more renders as itself.
"""

from __future__ import annotations

import os
from collections.abc import Mapping

from sakura.accounts.providers import ADAPTERS
from sakura.accounts.registry import Account, AccountsConfigError, Registry, load

NO_ACCOUNTS = Registry(accounts=(), implicit=True)
PROVIDER_SUFFIX = " ({provider})"
PRIMARY_PROVIDER = "claude"


def load_or_empty(env: Mapping[str, str] = os.environ) -> Registry:
    """The registry, or an empty one when it cannot be loaded: a read-only view must render, with bare keys."""
    try:
        return load(env=env)
    except AccountsConfigError:
        return NO_ACCOUNTS


def label_of(account_key: str, registry: Registry) -> str:
    """The label of the account a snapshot key (or a bare Claude id) names, else the key itself."""
    account = _find(account_key, registry)
    return account.label if account else account_key


def display_name(account_key: str, registry: Registry) -> str:
    """`label_of`, plus ` (Codex)` for a non-Claude account so a same-label pair stays distinguishable."""
    account = _find(account_key, registry)
    if account is None:
        return account_key
    if account.provider == PRIMARY_PROVIDER:
        return account.label
    return account.label + PROVIDER_SUFFIX.format(provider=account.provider.title())


def _find(account_key: str, registry: Registry) -> Account | None:
    for account in registry.accounts:
        if ADAPTERS[account.provider].snapshot_key(account.id) == account_key:
            return account
    return None
