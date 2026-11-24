"""The accounts pace may move a step to, per project: the person's own list, kept in the engine home.

Pace moves a step that runs on the project's account to another account when its own would land above the line
(`flow/pace.py`). That spends another account's week on this project's work, so it is the person's choice, project by
project: with nothing listed no account is allowed and pace waits instead. The list lives in `pace_accounts.toml` of
the engine home (outside every repository, so no repository file can widen it), one table per project id:

    ["<project id>"]
    accounts = ["work"]

A step pinned to an account (`step_accounts`) never moves, whatever is listed here.
"""

from __future__ import annotations

import json
import re
import tomllib
from collections.abc import Sequence
from pathlib import Path

from sakura import engine_home
from sakura.flow import gates

FILE = "pace_accounts.toml"
ACCOUNTS_KEY = "accounts"
ACCOUNT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:@-]{0,79}")
MAX_ACCOUNTS = 16
SEPARATOR = ","


class AccountsRefused(ValueError):
    """Raised, with a one-line reason, when the list of accounts cannot be kept."""


def allowed(repo: Path, home: Path | None) -> list[str]:
    """The account ids pace may move a step of `repo`'s project to; empty when the person listed none."""
    return _read(home).get(engine_home.project_id(repo), [])


def everywhere(home: Path | None) -> dict[str, list[str]]:
    """The lists of every project, by project id."""
    return _read(home)


def set_allowed(repo: Path, accounts: Sequence[str], home: Path | None) -> list[str]:
    """Keep `accounts` as the allowed ones of `repo`'s project (an empty list removes the project's entry) and return them.

    Raises AccountsRefused, and keeps nothing, for an id that is not a plain account id or for more than MAX_ACCOUNTS.
    """
    return set_allowed_by_id(engine_home.project_id(repo), accounts, home)


def set_allowed_by_id(project: str, accounts: Sequence[str], home: Path | None) -> list[str]:
    """`set_allowed` for a project known only by its id (an entry whose repository is no longer registered)."""
    chosen = _checked(accounts)
    kept = _read(home)
    kept.pop(project, None)
    kept.update({project: chosen} if chosen else {})
    gates.write_atomically(_path(home), _dump(kept).encode())
    return chosen


def parse(text: str) -> list[str]:
    """The account ids of a comma separated list (the form the act and the command take); empty text is an empty list."""
    return [part.strip() for part in text.split(SEPARATOR) if part.strip()]


def _checked(accounts: Sequence[str]) -> list[str]:
    chosen = list(dict.fromkeys(accounts))
    bad = next((name for name in chosen if not ACCOUNT_ID.fullmatch(name)), None)
    if bad is not None:
        raise AccountsRefused(f"{bad!r} is not an account id; use the ids of the accounts Leeway lists")
    if len(chosen) > MAX_ACCOUNTS:
        raise AccountsRefused(f"at most {MAX_ACCOUNTS} accounts can be allowed for a project")
    return chosen


def _path(home: Path | None) -> Path:
    return engine_home.root(home) / FILE


def _read(home: Path | None) -> dict[str, list[str]]:
    """The file as `{project id: accounts}`. A file that cannot be read allows nothing: pace waits instead of moving."""
    try:
        document = tomllib.loads(_path(home).read_text())
    except (OSError, tomllib.TOMLDecodeError):
        return {}
    return {project: list(table[ACCOUNTS_KEY]) for project, table in document.items()
            if isinstance(table, dict) and isinstance(table.get(ACCOUNTS_KEY), list)
            and all(isinstance(name, str) for name in table[ACCOUNTS_KEY])}


def _dump(kept: dict[str, list[str]]) -> str:
    return "".join(f"[{json.dumps(project)}]\n{ACCOUNTS_KEY} = {json.dumps(accounts)}\n\n"
                   for project, accounts in sorted(kept.items()))
