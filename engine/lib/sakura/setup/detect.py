"""First-launch inventory without executing adapters or opening candidate files."""

from __future__ import annotations

import os
from pathlib import Path

from sakura import programs
from sakura.accounts import detect as account_detect
from sakura.accounts import registry
from sakura.accounts.providers import ADAPTERS
from sakura.flow import cage
from sakura.flow.render import strip_controls
from sakura.setup.harnesses import HARNESSES, Harness, qualify, selected_program


class ConsentRequired(ValueError):
    """The host has not requested a structural scan."""


def detect(*, consented: bool = False) -> dict:
    """Return the supported harnesses and local account candidates after explicit consent."""
    if not consented:
        raise ConsentRequired("ask before scanning for harnesses")
    home = cage.account_home().resolve()
    folders = account_detect.list_dot_directories(home)
    registered = _registered(home)
    harnesses, logins = [], []
    for harness in HARNESSES:
        row = _harness_row(harness)
        harnesses.append(row)
        ordered = (*harness.native_folders(home), os.environ.get(ADAPTERS[harness.id].env_var),
                   *(folder for folder in folders if harness.matches(folder, home)))
        candidates = dict.fromkeys(path for entry in ordered if entry and (path := _resolve(entry)) is not None)
        start = len(logins)
        for folder in candidates:
            if harness.is_candidate(folder, home) and harness.has_login_structure(folder):
                account = registered.get((harness.id, folder))
                logins.append(_login(harness, folder, row, account, len(logins) - start))
    return {"harnesses": harnesses, "logins": logins}


def _registered(home: Path) -> dict:
    env = {**os.environ, "HOME": str(home)}
    path = registry.default_path(env)
    try:
        books = [registry.load_stored(path, env), registry.load_stored(path.with_name(registry.GOOSE_ACCOUNTS_FILE), env)]
    except registry.AccountsConfigError:
        return {}
    return {(account.provider, account.config_dir.resolve()): account for book in books if book for account in book.accounts}


def _resolve(entry: str | Path) -> Path | None:
    try:
        return Path(entry).resolve(strict=True)
    except (OSError, RuntimeError):
        return None


def _harness_row(harness: Harness) -> dict:
    row = {"id": harness.id, "label": harness.name, "program_path": None, "version": None, "found": False,
           "adapter_path": None, "adapter_version": None, "readiness": "unavailable"}
    try:
        native = selected_program(harness.id, None, "install_program")
        row.update(program_path=native, found=True)
        adapter = native if harness.adapter == harness.id else selected_program(harness.adapter, None, "install_adapter")
        row["adapter_path"] = adapter
        installed = qualify(harness.id)
    except programs.ProgramError as error:
        reason = str(error)
        return {**row, "reason": reason, "readiness": "unknown" if reason == "qualification_required" else "unavailable"}
    return {**row, "version": (installed.native or installed.adapter).version, "adapter_version": installed.adapter.version,
            "readiness": "unknown", "reason": "connection_required"}


def _login(harness: Harness, folder: Path, row: dict, account: registry.Account | None, index: int) -> dict:
    label = " ".join(strip_controls(account.label).split())[:60] if account else None
    proposed = harness.name if index == 0 else f"{harness.name} {folder.name.lstrip('.')}"
    reference = {"provider": account.provider, "id": account.id} if account else None
    return {"harness": harness.id, "folder": str(folder), "logged_in": None, "in_book_as": label,
            "proposed_name": (label or proposed)[:60], "readiness": row["readiness"], "reason": row["reason"],
            "registered_account": reference}
