"""The names a person reads for an account and a plan, made from the ids when Leeway gave no better one."""

from __future__ import annotations

from sakura.accounts.providers import ADAPTERS

# The account every provider's own login creates: the person's own, which a person calls personal.
DEFAULT_ID = "default"
PERSONAL_WORD = "personal"
PLAN_WORDS = {"max5x": "Max 5x", "max20x": "Max 20x", "prolite": "Pro Lite"}


def account_label(account_id: str, label: str = "", provider: str = "") -> str:
    """The name of an account: `label` when it says more than the id does, else the provider and a word for the id
    (`Claude, personal` for `claude-default`). `provider` is read from the id (`claude-default`) when not given."""
    provider = provider or _provider_in(account_id)
    rest = _without_provider(account_id, provider)
    if label.strip() and label not in (account_id, rest, DEFAULT_ID):
        return label.strip()
    words = _words(PERSONAL_WORD if rest == DEFAULT_ID else rest)
    return f"{provider.capitalize()}, {words}" if provider else words.capitalize()


def plan_label(provider: str, plan: str | None) -> str:
    """The name of a plan with its provider (`Codex Pro Lite`); empty when there is no plan."""
    if not plan:
        return ""
    return f"{provider.capitalize()} {PLAN_WORDS.get(plan) or _words(plan).capitalize()}".strip()


def _provider_in(account_id: str) -> str:
    return next((name for name in ADAPTERS if account_id.startswith(f"{name}-")), "")


def _without_provider(account_id: str, provider: str) -> str:
    prefix = f"{provider}-"
    return account_id[len(prefix):] if provider and account_id.startswith(prefix) else account_id


def _words(text: str) -> str:
    return text.replace("-", " ").replace("_", " ")
