"""The account side of the cockpit's actions: the target, the checks of its four inputs and the refusal text.

Pure helpers with no import of `actions`, which registers the five account actions in its table. An account is
addressed by `["account", provider, id]`; its commands run `sakura accounts ...`, which keeps its own rules
(atomic, locked, refused on a legacy host or for an unknown account).
"""

from __future__ import annotations

import os
import re
import shlex
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.cockpit.accounts import AccountRow
from sakura.flow.render import strip_controls

KIND_ACCOUNT = "account"
MAX_LABEL = 60
MAX_REASON = 200  # one line, as long as the other one-line inputs of the cockpit
PLAN_SLUG = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
PERCENT = re.compile(r"(?:100(?:\.0{1,6})?|[0-9]{1,2}(?:\.[0-9]{1,6})?|none)")
SINCE = re.compile(r"\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d{1,6})?)?(?:Z|[+-]\d{2}:\d{2})?)?")
CLI_ERROR = re.compile(r"^sakura: error: (.*)$", re.MULTILINE)
# A token that starts like a path, or names one of the files the accounts commands read and write.
PATH_LIKE = re.compile(r"(?:^|[\s'\"(=])(?:/|~|\.{1,2}/|[A-Za-z]:[\\/])|\\|\.(?:toml|db|lock|tmp)\b")
LEGACY_HINT = "run sakura accounts migrate first"
LEGACY_HOST_TEXT = "this host still keeps its accounts in the legacy store; run sakura accounts migrate first"
UNREADABLE_TEXT = "the accounts command refused; run sakura accounts on the host for the reason"
SHOWN_PROGRAM = "sakura"


@dataclass(frozen=True)
class AccountTarget:
    """The target of an account action. `key` is `("account", provider, id)`."""

    provider: str
    id: str

    kind = KIND_ACCOUNT

    @property
    def key(self) -> tuple:
        return (KIND_ACCOUNT, self.provider, self.id)

    @property
    def repo(self) -> Path:
        """Where the command runs: the accounts commands read no repository, so the root, which names no one."""
        return Path(os.sep)


def target_of(row: AccountRow) -> AccountTarget:
    """The target an account row of the snapshot stands for."""
    return AccountTarget(row.provider, row.id)


def leeway_targets() -> list[AccountTarget]:
    """The accounts Leeway's book lists, as targets; none when Leeway cannot be read."""
    from sakura.usage import leeway

    try:
        return [AccountTarget(row["provider"], row["id"]) for row in leeway.accounts()]
    except leeway.LeewayError:
        return []


def label_problem(value: str) -> str | None:
    """Why `value` is not an account label: 1 to 60 printable characters on one line, not blank."""
    if not value.strip() or len(value) > MAX_LABEL:
        return f"must be 1 to {MAX_LABEL} characters"
    return None if strip_controls(value) == value and "\n" not in value and "\t" not in value else (
        "must be printable characters on one line")


def percent_problem(value: str) -> str | None:
    """Why `value` is not a ceiling: a number from 0 to 100, or the word `none`."""
    return None if PERCENT.fullmatch(value) else "must be a number from 0 to 100, or none"


def plan_problem(value: str) -> str | None:
    """Why `value` is not a plan name: a lowercase slug of at most 32 characters."""
    return None if PLAN_SLUG.fullmatch(value) else f"does not match {PLAN_SLUG.pattern}"


def since_problem(value: str) -> str | None:
    """Why `value` is not a plan start: `YYYY-MM-DD` or an ISO 8601 datetime."""
    try:
        if SINCE.fullmatch(value):
            datetime.fromisoformat(value)
            return None
    except ValueError:
        pass
    return "must be YYYY-MM-DD or an ISO 8601 datetime"


def plan_choice_problem(provider: str, plan: str) -> str | None:
    """Why `plan` is not one of the plans `provider` offers (the list `accounts plan` enforces too)."""
    choices = ADAPTERS[provider].plan_choices
    return None if plan in choices else f"must be one of {', '.join(choices)} for {provider}"


def shown_command(argv: list[str]) -> str:
    """The command as a confirmation shows it: no install path, no working directory."""
    return shlex.join([SHOWN_PROGRAM, *argv[1:]])


INPUT_PROBLEMS = {"label": label_problem, "percent": percent_problem, "plan": plan_problem, "since": since_problem}


def refusal_text(output: str) -> str:
    """The one-line text for what an accounts command said (a refusal or a failure), never carrying a path.

    The registry's own errors name the files they read, and this text goes to every node the cockpit allows:
    a message that looks like it holds a path is replaced by a fixed one. The legacy-host refusal is such a
    message (it names the store), so it is recognized and given its own fixed text.
    """
    found = CLI_ERROR.findall(output)
    message = strip_controls(found[-1]).strip() if found else ""
    if LEGACY_HINT in message:
        return LEGACY_HOST_TEXT
    if not message or PATH_LIKE.search(message):
        return UNREADABLE_TEXT
    return message[:MAX_REASON]
