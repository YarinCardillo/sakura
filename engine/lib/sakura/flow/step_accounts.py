"""The account each step of a lane runs on, and what the launcher and the guard do about it.

A preset names an account for a step (`steps.build.account`), or leaves it to the project. The launcher points the
step's agent at that account's folder; the budget guard judges the reserve on that account's numbers. With one account
every run uses it whatever the pipeline says. An account that Leeway does not list (hidden, removed, or of another
provider than the step's agent) never makes a step run on some other account: the step asks the person.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.budget import leeway_numbers
from sakura.budget.guard import Snapshot
from sakura.flow import pace, pace_check
from sakura.usage import leeway
from sakura.usage.archive.quota import claude_config_env

ACCOUNT_RULE = "account"
PROJECT_ACCOUNT = pace.PROJECT_ACCOUNT
CODEX = "codex"
CLAUDE = "claude"


class StepAccountProblem(RuntimeError):
    """A step's account cannot be used; the message says why, in words for the person."""


@dataclass(frozen=True)
class StepAccount:
    """The account a step runs on, resolved once: the launcher takes its folder, the guard its key (which names its
    reserve and archive) and its readings, so the two never look at different accounts."""

    key: str
    config_dir: str
    snapshot: Snapshot | None


def account_of(values: Mapping, state: Mapping, role: str, step: str = "") -> str:
    """The account the step `role` runs is pinned to: where pace moved it, else the preset's, else `project`.

    `step` names the pipeline step when it is not the role's own (the final review runs the code reviewer)."""
    step = pace_check.step_of(role, step)
    if step is None:
        return PROJECT_ACCOUNT
    moved = ((state.get("budget") or {}).get(pace_check.ACCOUNT_KEY) or {}).get(step)
    return moved or values["step_accounts"].get(step, PROJECT_ACCOUNT)


def listing() -> tuple[list[dict], int]:
    """The accounts Leeway lists as usable (its `status` omits hidden and removed ones) and how many accounts the book
    holds, hidden ones included. Raises StepAccountProblem when Leeway cannot be asked."""
    try:
        return list(leeway.status()["accounts"]), len(leeway.accounts())
    except leeway.LeewayError as error:
        raise StepAccountProblem(f"the account of this step cannot be checked: {error}") from None


def chosen_row(values: Mapping, state: Mapping, role: str, provider: str, step: str = "") -> dict | None:
    """The Leeway row of the account the step runs on, or None when the project's account applies.

    With one account in the book a pinned step runs on that account, whatever the pipeline names (None when it is of
    another provider than the step's, or not listed). Raises StepAccountProblem for an
    account that is hidden or missing, or of another provider than `provider`.
    """
    pinned = account_of(values, state, role, step)
    if pinned == PROJECT_ACCOUNT:
        return None
    rows, known = listing()
    if known <= 1:
        return _only_row(rows, provider)
    row = leeway_numbers.row_of(rows, pinned)
    if row is None:
        raise StepAccountProblem(f"the account {pinned} is hidden or missing in Leeway")
    if provider in (CLAUDE, CODEX) and row["provider"] != provider:
        raise StepAccountProblem(f"the account {pinned} is a {row['provider']} account and this step runs {provider}")
    return row


def _only_row(rows: Sequence[dict], provider: str) -> dict | None:
    """The row of the book's only account when it is listed and fits the step's provider."""
    fits = [row for row in rows if row["provider"] == provider and row.get("config_dir")]
    return fits[0] if len(fits) == 1 else None


def resolve(values: Mapping, state: Mapping, role: str, provider: str, step: str = "") -> StepAccount | None:
    """The account the step runs on, or None when the project's account applies (see `chosen_row` for the cases and
    the StepAccountProblem it raises). Every path to an account (pinned, pace switch, the only one) ends here."""
    row = chosen_row(values, state, role, provider, step)
    if row is None:
        return None
    return of_row(row, provider)


def of_row(row: Mapping, provider: str) -> StepAccount:
    """The account of the Leeway `row`, resolved as a step's own is."""
    adapter = ADAPTERS[CODEX if provider == CODEX else CLAUDE]
    return StepAccount(adapter.snapshot_key(row["id"]), row["config_dir"], leeway_numbers.snapshot_of([row], row["id"]))


def apply(env: Mapping[str, str], values: Mapping, state: Mapping, role: str, provider: str,
          step: str = "") -> dict[str, str]:
    """`env` pointing the step's agent at its account's folder; unchanged for the project's account."""
    resolved = resolve(values, state, role, provider, step)
    return dict(env) if resolved is None else point_at(env, resolved.config_dir, provider)


def point_at(env: Mapping[str, str], config_dir: str, provider: str) -> dict[str, str]:
    """`env` pointing the agent of `provider` at the login folder `config_dir`."""
    if provider == CODEX:
        return {**env, ADAPTERS[CODEX].env_var: config_dir}
    return claude_config_env(env, Path(config_dir))


def problem(values: Mapping, state: Mapping, role: str, provider: str, step: str = "") -> str:
    """Why the step may not run on its account, or an empty string."""
    try:
        chosen_row(values, state, role, provider, step)
    except StepAccountProblem as error:
        return str(error)
    return ""
