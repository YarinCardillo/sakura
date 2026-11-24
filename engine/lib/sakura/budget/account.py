"""Run a role on the account named by `budget.account`: Claude roles by `CLAUDE_CONFIG_DIR`, Codex roles by `CODEX_HOME`."""

from __future__ import annotations

import os
from collections.abc import Iterable, Mapping
from pathlib import Path

from sakura.accounts.providers import ADAPTERS, CODEX_ACCOUNT_PREFIX
from sakura.usage.archive.quota import claude_config_env
from sakura.usage.archive.settings import Source

CLAUDE = "claude"
CODEX = "codex"


class UnknownAccountError(ValueError):
    """`budget.account` names no enabled Claude account in the account registry."""


def apply(env: Mapping[str, str], account: str, sources: Iterable[Source], disabled: Iterable[Source] = ()) -> dict[str, str]:
    """`env` pointing Claude Code at the config directory of `account`.

    An empty account returns the environment as it is. The default `~/.claude` leaves
    `CLAUDE_CONFIG_DIR` unset (see `claude_config_env`). An account no source defines, or that is among
    the `disabled` sources, raises UnknownAccountError instead of silently running on the default account.
    """
    if not account:
        return dict(env)
    claude = [source for source in sources if source.provider == CLAUDE and source.config_dir is not None]
    for source in claude:
        if source.account == account:
            return claude_config_env(env, source.config_dir)
    _refuse_disabled(account, disabled)
    known = ", ".join(sorted({source.account for source in claude})) or "none"
    raise UnknownAccountError(f"budget account {account!r} is not a Claude account in the account registry (known: {known})")


def apply_codex(env: Mapping[str, str], account: str, sources: Iterable[Source]) -> dict[str, str]:
    """`env` pointing Codex at the config directory of the Codex account paired with `account` (see `codex_pair`).

    Without a pairing the environment is returned as it is: those Codex roles run unguarded.
    """
    paired = codex_pair(account, sources)
    if not account or paired is None or paired.config_dir is None:
        return dict(env)
    return {**env, ADAPTERS[CODEX].env_var: str(paired.config_dir)}


def resolve(account: str, sources: Iterable[Source], env: Mapping[str, str] = os.environ,
            disabled: Iterable[Source] = ()) -> str:
    """The account name to read quota snapshots for.

    A named account is returned as given, unless it is among the `disabled` sources. An empty one means the
    account whose config directory the engine runs with: the registry Claude account whose directory is
    `CLAUDE_CONFIG_DIR`, or the default `~/.claude` when it is unset. Raises UnknownAccountError, naming that
    directory, when no account has it: a variable pointing at a directory no account owns never falls back.
    """
    if account:
        _refuse_disabled(account, disabled)
        return account
    directory = _engine_config_dir(env)
    for source in sources:
        if source.provider == CLAUDE and source.config_dir is not None and _same_dir(source.config_dir, directory):
            return source.account
    raise UnknownAccountError(f"budget.account is empty and no Claude account in the registry has the config "
                              f"directory {directory} ({ADAPTERS[CLAUDE].env_var}, or {ADAPTERS[CLAUDE].default_dir} when unset)")


def codex_account(claude_account: str | None, sources: Iterable[Source]) -> str | None:
    """The quota snapshot account (`codex:<name>`) that guards the Codex roles of a flow, None when none applies."""
    paired = codex_pair(claude_account, sources)
    return None if paired is None else CODEX_ACCOUNT_PREFIX + paired.account


def codex_pair(claude_account: str | None, sources: Iterable[Source]) -> Source | None:
    """The Codex account paired with the flow's Claude budget account, None when none applies.

    The Codex account with the same id wins (one id is one person or workspace); failing that, the only Codex
    account there is. With several Codex accounts and no id match, or with none, the Codex roles run unguarded.
    """
    codex = [source for source in sources if source.provider == CODEX]
    named = [source for source in codex if source.account == claude_account]
    if claude_account and named:
        return named[0]
    return codex[0] if len({source.account for source in codex}) == 1 else None


def _refuse_disabled(account: str, disabled: Iterable[Source]) -> None:
    if any(source.provider == CLAUDE and source.account == account for source in disabled):
        raise UnknownAccountError(f"budget account {account!r} is disabled in the account registry")


def _engine_config_dir(env: Mapping[str, str]) -> Path:
    adapter = ADAPTERS[CLAUDE]
    return Path(env.get(adapter.env_var) or adapter.default_dir).expanduser()


def _same_dir(first: Path, second: Path) -> bool:
    return first.expanduser().resolve() == second.resolve()
