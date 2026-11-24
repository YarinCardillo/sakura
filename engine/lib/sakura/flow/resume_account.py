"""The account a conversation ran on, found again when it is continued.

A run records which account configuration its account came from (`account_ref`: a budget account of the registry, an
account a step is pinned to in Leeway, or none for the default one). A continuation resolves that account once, from
the account configuration and never from the lane's current settings or the operator's environment, and the one
resolution feeds the guard (its reserve and numbers), the environment the agent is launched with and the transcript
root of the continuation's record, so a message to an old conversation can neither spend another account's quota nor
look for the conversation in another folder. An account that is gone is refused in words.
"""

from __future__ import annotations

import os
import time
from collections.abc import Mapping
from dataclasses import dataclass, replace
from pathlib import Path

from sakura import programs
from sakura.accounts import goose
from sakura.accounts.providers import ADAPTERS, CODEX_ACCOUNT_PREFIX
from sakura.budget import account as budget_account
from sakura.budget import leeway_numbers
from sakura.flow import budget_guard, run_records, step_accounts
from sakura.usage.archive import settings as usage_settings

BUDGET, STEP = "budget", "step"
ROWS_TTL_SECONDS = 5.0
CLAUDE, CODEX = "claude", "codex"
_rows_cache: tuple[float, list[dict]] | None = None


class AccountProblem(RuntimeError):
    """Raised when the account of a conversation cannot be found now; the message is a sentence for the person."""


@dataclass(frozen=True)
class ResumeAccount:
    """The account a continuation runs on: its config `root`, what the guard judges (`account`) and the account pace
    judges (`pace_account`, empty for the project's)."""

    root: Path
    account: step_accounts.StepAccount
    pace_account: str = ""
    ref: dict | None = None  # the reference a record of the continuation carries (a record that had none is given one)


def forget() -> None:
    """Drop the remembered Leeway listing (a test, or a change of the account book)."""
    global _rows_cache
    _rows_cache = None


def resolve_record(record: dict, environ: Mapping[str, str] | None = None) -> ResumeAccount:
    """The account of the conversation run `record` belongs to (see `resolve`). A record from before references were kept
    is matched by its folder to a configured account of its provider (or the default folder); one that matches none is
    refused in words naming its account."""
    provider, root = record["provider"], str(record.get("transcript_root", ""))
    if provider == "goose":
        return _goose(record)
    if "account_ref" in record:
        return resolve(provider, record["account_ref"], root, environ)
    ref = _ref_from_root(provider, root)
    try:
        return resolve(provider, ref, root, environ)
    except AccountProblem:
        if ref is None:
            raise AccountProblem(f"The account {record.get('account') or Path(root).name} ran this conversation, and no "
                                 "configured account has its folder any more.") from None
        raise



def _goose(record: dict) -> ResumeAccount:
    ref = record.get("account_ref")
    binding = record.get("acp_binding")
    identity = ref.get("id") if isinstance(ref, dict) and ref.get("kind") == "goose" else None
    if not isinstance(binding, dict) or not isinstance(identity, str) or binding.get("account") != identity:
        raise AccountProblem("The Goose conversation has no verified account binding.")
    try:
        row = goose.lane_account(identity)
    except programs.ProgramError:
        raise AccountProblem("The bound Goose account is unavailable. Check its connection in Accounts.") from None
    if (goose.lane_identity(row) != binding.get("configuration") or row.goose_provider != binding.get("provider")
            or str(row.config_dir) != record.get("transcript_root")):
        raise AccountProblem("The bound Goose account configuration changed. Start a fresh lane attempt.")
    account = step_accounts.StepAccount(ADAPTERS["goose"].snapshot_key(identity), str(row.config_dir), None)
    return ResumeAccount(row.config_dir, account, ref=ref)

def resolve(provider: str, ref: dict | None, recorded_root: str, environ: Mapping[str, str] | None = None) -> ResumeAccount:
    """The account of a conversation of `provider` recorded with `ref` and stored under `recorded_root`.

    Raises AccountProblem when that account is no longer configured, is of another provider, or the conversation was
    stored under another folder than the one this Mac uses for the account."""
    environ = os.environ if environ is None else environ
    if ref is None:
        return _default(provider, recorded_root, environ)
    if ref.get("kind") == BUDGET:
        found = _budget(provider, str(ref.get("id", "")), str(ref.get("codex", "")))
    elif ref.get("kind") == STEP:
        found = _step(provider, str(ref.get("id", "")))
    else:
        raise AccountProblem("It ran on an account this version cannot find again.")
    _require_same_folder(found, recorded_root, str(ref.get("codex") or ref.get("id", "")))
    return replace(found, ref={key: value for key, value in ref.items() if key != "label"})


def _require_same_folder(found: ResumeAccount, recorded_root: str, name: str) -> None:
    if recorded_root and Path(recorded_root).resolve() != found.root.resolve():
        raise AccountProblem(f"The account {name} keeps its conversations in another folder now than when this one ran.")


def _ref_from_root(provider: str, recorded_root: str) -> dict | None:
    """The reference of the configured account of `provider` whose folder is `recorded_root`; None when none is (the
    default folder, or nothing)."""
    try:
        usage = usage_settings.load()
    except usage_settings.UsageConfigError:
        return None
    for source in usage.sources:
        if source.provider == provider and source.config_dir and recorded_root and \
                source.config_dir.resolve() == Path(recorded_root).resolve():
            return {"kind": BUDGET, "id": source.account, **({"codex": source.account} if provider == CODEX else {})}
    return None


def apply(env: dict, provider: str, resolved: ResumeAccount) -> dict:
    """`env` pointing the agent at the account's folder: the provider's config variable is dropped for the default folder
    (compared by resolved path: the home may reach it through a link) and set for any other. Claude keys its login by that
    variable, so the default account must never carry it."""
    variable, default = run_records.TRANSCRIPT_ROOTS[provider]
    if resolved.root.resolve() == (Path.home() / default).resolve():
        return {key: value for key, value in env.items() if key != variable}
    return {**env, variable: str(resolved.root)}


def _default(provider: str, recorded_root: str, environ: Mapping[str, str]) -> ResumeAccount:
    root = Path(run_records.transcript_root(provider, dict(environ)))
    if recorded_root and Path(recorded_root).resolve() != root.resolve():
        raise AccountProblem("It ran under another folder of the default account than this Mac uses now.")
    return ResumeAccount(root, _default_guard_account(provider, root), ref=None)


def _default_guard_account(provider: str, root: Path) -> step_accounts.StepAccount:
    """The registry account whose folder is `root`: the account the guard judges for a default-account conversation."""
    try:
        usage = usage_settings.load()
    except usage_settings.UsageConfigError as error:
        raise AccountProblem(f"The account of this conversation cannot be found: {error}") from error
    if provider == CODEX:
        owned = [source for source in usage.sources if source.provider == CODEX and source.config_dir
                 and source.config_dir.resolve() == root.resolve()]
        key = CODEX_ACCOUNT_PREFIX + owned[0].account if owned else ADAPTERS[CODEX].snapshot_key("default")
        return step_accounts.StepAccount(key, str(root), None)
    variable, default = run_records.TRANSCRIPT_ROOTS[CLAUDE]
    try:
        name = budget_account.resolve("", usage.sources, {} if root == Path.home() / default else {variable: str(root)},
                                      usage.disabled)
    except budget_account.UnknownAccountError as error:
        if not usage_settings.config_exists():
            return step_accounts.StepAccount("", str(root), None)  # no accounts are configured: the guard has no budget
        raise AccountProblem(f"The account of this conversation cannot be found: {error}") from error
    return step_accounts.StepAccount(name, str(root), budget_guard.leeway_snapshot(name))


def _budget(provider: str, name: str, codex_name: str = "") -> ResumeAccount:
    try:
        usage = usage_settings.load()
    except usage_settings.UsageConfigError as error:
        raise AccountProblem(f"The account {name} cannot be found: {error}") from error
    if provider == CODEX:
        named = codex_name or name
        found = [source for source in usage.sources if source.provider == CODEX and source.account == named and source.config_dir]
        if not found:
            raise AccountProblem(f"The Codex account {named} is not in the account registry any more.")
        return ResumeAccount(found[0].config_dir, step_accounts.StepAccount(CODEX_ACCOUNT_PREFIX + named,
                                                                           str(found[0].config_dir), None))
    found = [source for source in usage.sources if source.provider == CLAUDE and source.account == name and source.config_dir]
    if not found:
        raise AccountProblem(f"The account {name} is not in the account registry any more.")
    return ResumeAccount(found[0].config_dir, step_accounts.StepAccount(name, str(found[0].config_dir),
                                                                       budget_guard.leeway_snapshot(name)))


def _step(provider: str, identity: str) -> ResumeAccount:
    row = leeway_numbers.row_of(_rows(), identity)
    if row is None or not row.get("config_dir"):
        raise AccountProblem(f"The account {identity} is hidden or missing in Leeway.")
    if row["provider"] != provider:
        raise AccountProblem(f"The account {identity} is a {row['provider']} account and this agent runs {provider}.")
    account = step_accounts.StepAccount(ADAPTERS[provider].snapshot_key(row["id"]), row["config_dir"],
                                        leeway_numbers.snapshot_of([row], row["id"]))
    return ResumeAccount(Path(row["config_dir"]), account, row["id"])


def _rows() -> list[dict]:
    global _rows_cache
    if _rows_cache is None or time.monotonic() - _rows_cache[0] > ROWS_TTL_SECONDS:
        try:
            _rows_cache = (time.monotonic(), step_accounts.listing()[0])
        except step_accounts.StepAccountProblem as error:
            raise AccountProblem(str(error)) from error
    return _rows_cache[1]
