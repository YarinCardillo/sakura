"""A role whose provider has no account runs on an account that exists, and the person is told in words.

A preset (a built-in one included) names a provider per tier, and the person may cancel one provider. No provider is
kept for or from a role: when the accounts hold none of the provider a role is set to, the role runs on the first
account there is, with that provider's own default model, and the engine says so. Nothing changes while an account of
the role's provider exists, or when no account is known at all (then the role fails as it would have)."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

AGENT_PROVIDERS = ("claude", "codex")


@dataclass(frozen=True)
class Substitute:
    """The provider and account a role runs on instead, and the words that say why (account: an `AgentAccount`)."""

    provider: str
    account: object
    words: str


def substitute(provider: str, role: str, accounts: Sequence) -> Substitute | None:
    """The substitute for a `role` set to `provider`, or None when it keeps its own.

    `accounts` have `id`, `label` and `provider`; the first one of an agent provider is taken, in the order given."""
    if provider not in AGENT_PROVIDERS or any(account.provider == provider for account in accounts):
        return None
    found = next((account for account in accounts if account.provider in AGENT_PROVIDERS), None)
    if found is None:
        return None
    words = f"No {provider.title()} account exists, so the {role} runs on the {found.provider.title()} account ({found.label})."
    return Substitute(found.provider, found, words)
