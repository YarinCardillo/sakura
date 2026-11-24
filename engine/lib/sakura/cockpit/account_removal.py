"""What must hold before an account leaves Leeway's book, and what is tidied once it has.

The engine keeps the account's id in two places: a step pinned to it (`step_accounts` of the rules, a preset or a
project) and the accounts pace may move a project's steps to (`flow/pace_accounts.py`). Removing an account that
either still names would leave a step that asks the person forever, so `refusal` says which one holds it back. The
allow-lists of projects that are no longer registered cannot hold it back; `forget` drops the id from them.
"""

from __future__ import annotations

from pathlib import Path

from sakura.accounts import registry as account_registry
from sakura.accounts import write
from sakura.accounts.providers import GOOSE
from sakura.flow import config, pace_accounts, registry


def refusal(account_id: str) -> str | None:
    """The one-line reason the account cannot be removed now, or None when nothing of the engine uses it."""
    projects = registry.repos()
    return _pinned_reason(account_id, projects) or _allowed_reason(account_id, projects)


def forget(account_id: str) -> None:
    """Drop the account from every pace allow-list that still holds it (those of projects no longer registered)."""
    for project, accounts in pace_accounts.everywhere(None).items():
        if account_id in accounts:
            pace_accounts.set_allowed_by_id(project, [name for name in accounts if name != account_id], None)


def detach_goose(account_id: str) -> None:
    """Detach a Goose account and clear its Master default while preserving active and saved sessions."""
    from sakura.setup import apply

    path = account_registry.default_path().with_name(account_registry.GOOSE_ACCOUNTS_FILE)
    with write.locked(apply.operation_lock()):
        if apply.master_account() == {"provider": GOOSE, "id": account_id}:
            apply.save_master_account(None)
        write.remove(path, GOOSE, account_id)


def _pinned_reason(account_id: str, projects: list[Path]) -> str | None:
    for repo in (None, *projects):
        try:
            pins = config.load(repo, None).values["step_accounts"]
        except config.FlowConfigError as error:
            return f"cannot check the steps of {_name(repo)}: {error}"
        steps = sorted(step for step, pinned in pins.items() if pinned == account_id)
        if steps:
            return f"step {steps[0]}{_of(repo)} runs on it; give that step another account first"
    return None


def _allowed_reason(account_id: str, projects: list[Path]) -> str | None:
    for repo in projects:
        if account_id in pace_accounts.allowed(repo, None):
            return f"pace may move steps of {repo.name} to it; take it off that project's allowed accounts first"
    return None


def _of(repo: Path | None) -> str:
    return f" of {repo.name}" if repo else ""


def _name(repo: Path | None) -> str:
    return repo.name if repo else "the personal rules"
