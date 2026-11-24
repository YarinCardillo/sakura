"""Apply first-launch choices and resume an interrupted operation."""

import json
import os
import re
import sys
from pathlib import Path

from sakura import engine_home, text_limits
from sakura.accounts import registry, write
from sakura.accounts.providers import GOOSE
from sakura.cockpit import account_actions, account_login
from sakura.flow import brakes, config, exits, pace, presets, rules
from sakura.flow.render import strip_controls
from sakura.setup.harnesses import HARNESS_BY_ID
from sakura.usage import leeway

OP = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}")
MAX_CHOICES = 65_536
MAX_FOLDER = 1024
MASTER_ACCOUNT_FILE = "master-account.json"


class SetupRefused(ValueError):
    """The supplied choices cannot be applied."""


def input_problem(name: str, value: str) -> str | None:
    """Validate the outer action inputs before accepting them into the cockpit log."""
    if name == "op":
        return None if OP.fullmatch(value) else "must be an operation id of up to 80 letters, digits, dashes or underscores"
    return text_limits.one_line_problem(value, MAX_CHOICES)


def parse_choices(op: str, raw_choices: str) -> dict:
    """Decode and validate the operation payload without looking at the host."""
    for name, value in (("op", op), ("choices", raw_choices)):
        if problem := input_problem(name, value):
            raise SetupRefused(f"invalid {name}: {problem}")
    try:
        choices = json.loads(raw_choices)
    except (ValueError, RecursionError):
        raise SetupRefused("choices must be a JSON object") from None
    if not isinstance(choices, dict) or "accounts" not in choices or choices.keys() - {"accounts", "preset", "pace",
                                                                                       "master_account"}:
        raise SetupRefused("choices must contain accounts and may include preset, pace and master_account")
    if "preset" in choices and not isinstance(choices["preset"], str):
        raise SetupRefused("preset must be text")
    if "pace" in choices and choices["pace"] not in pace.LEVELS:
        raise SetupRefused("choose one of the engine's pace levels")
    if not isinstance(choices["accounts"], list):
        raise SetupRefused("accounts must be a list")
    for account in choices["accounts"]:
        _check_account(account)
    _check_master(choices.get("master_account"), choices["accounts"])
    return choices


def run(op: str, choices: dict) -> tuple[int, str]:
    """Apply validated choices and return the book names and requested login folders."""
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        return exits.USAGE, "setup can only run outside engine roles and agent cages"
    try:
        folder = engine_home.root() / "setup"
        with write.locked(operation_lock()):
            return exits.OK, json.dumps(_apply(folder / f"{op}.json", choices))
    except SetupRefused as error:
        return exits.USAGE, str(error)
    except presets.PresetError as error:
        print(f"setup: {strip_controls(str(error))}", file=sys.stderr)
        return exits.USAGE, "Sakura could not use the selected preset. Choose an available preset."
    except leeway.LeewayError:
        return exits.FAILED, "Leeway could not finish adding the chosen accounts. Retry setup to resume."
    except registry.AccountsConfigError as error:
        return exits.USAGE, account_actions.refusal_text(f"sakura: error: {error}")
    except (OSError, ValueError, engine_home.EngineHomeError, config.FlowConfigError, rules.RuleRefused):
        return exits.FAILED, "Sakura could not save the setup choices. Retry setup to resume."


def _check_account(account: object) -> None:
    required = {"harness", "folder", "name"}
    if not isinstance(account, dict) or not required <= account.keys() or account.keys() - (required | {"provider_config"}):
        raise SetupRefused("each account must contain harness, folder and name")
    if not all(isinstance(account[key], str) for key in required) or account["harness"] not in HARNESS_BY_ID:
        raise SetupRefused("each account must name a supported harness and give its folder and name as text")
    if problem := text_limits.one_line_problem(account["folder"], MAX_FOLDER):
        raise SetupRefused(f"invalid login folder: {problem}")
    if problem := account_actions.label_problem(account["name"]):
        raise SetupRefused(f"invalid account name: {problem}")
    if account["name"].startswith("-"):
        raise SetupRefused(account_login.NAME_DASH)
    if "provider_config" in account:
        if account["harness"] != GOOSE:
            raise SetupRefused("provider_config is available for Goose accounts")
        try:
            registry.provider_config(account["provider_config"])
        except registry.AccountsConfigError as error:
            raise SetupRefused(str(error)) from None


def _check_master(choice: object, accounts: list[dict]) -> None:
    if choice is None:
        return
    if isinstance(choice, dict) and all(isinstance(value, str) for value in choice.values()):
        if choice.keys() == {"harness", "folder"} and any(
                (row["harness"], row["folder"]) == (choice["harness"], choice["folder"]) for row in accounts):
            return
        if _is_reference(choice):
            return
    raise SetupRefused("master_account must name a checked login, an existing account, or null")


def _is_reference(value: object) -> bool:
    return (isinstance(value, dict) and value.keys() == {"provider", "id"}
            and isinstance(value["provider"], str) and value["provider"] in HARNESS_BY_ID
            and isinstance(value["id"], str) and text_limits.one_line_problem(value["id"], 120) is None)


def operation_lock() -> Path:
    """Return the lock shared by first-launch apply and detach of its default account."""
    return engine_home.root() / "setup" / "operations"


def master_account() -> dict | None:
    """Return the optional default reference, excluding unreadable or invalid records."""
    try:
        stored = json.loads((engine_home.root() / "setup" / MASTER_ACCOUNT_FILE).read_text())
    except (OSError, ValueError, RecursionError, engine_home.EngineHomeError):
        return None
    return stored if _is_reference(stored) else None


def save_master_account(reference: dict | None) -> None:
    """Set or clear the default while the caller holds the setup operations lock."""
    if master_account() != reference:
        write.replace_atomically(engine_home.root() / "setup" / MASTER_ACCOUNT_FILE, json.dumps(reference))


def _apply(path: Path, choices: dict) -> dict:
    state = _read(path, choices)
    if not state["done"]:
        _apply_pending(path, choices, state)
        state["done"] = True
        _save(path, state)
    result = {"accounts": [{**row, "folder": choice["folder"]}
                           for row, choice in zip(state["accounts"], choices["accounts"], strict=True)]}
    if "master_account" in choices:
        result["master_account"] = state["master_account"]
    return result


def _apply_pending(path: Path, choices: dict, state: dict) -> None:
    preset = choices.get("preset")
    if preset is not None:
        presets.resolve(preset)
    default = choices.get("master_account")
    if _is_reference(default):
        _require_enabled(default)
    remaining = choices["accounts"][len(state["accounts"]):]
    folders = [_checked_folder(account) for account in remaining]
    if not path.exists():
        _save(path, state)
    for account, folder in zip(remaining, folders, strict=True):
        state["accounts"].append(_apply_account(path, state, account, folder))
        state["created"] = None
        _save(path, state)
    if preset is not None and presets.chosen() != preset:
        presets.choose(preset)
    if "pace" in choices:
        _apply_pace(choices["pace"])
    if "master_account" in choices:
        resolved = _resolve_master(default, choices["accounts"], state["accounts"])
        save_master_account(resolved)
        state["master_account"] = resolved


def _apply_pace(level: str) -> None:
    if not (engine_home.root() / config.PACE_FILE).exists() or config.load(None).values["knobs"]["pace"] != level:
        rules.set_pace(level, None)


def _resolve_master(choice: dict | None, selections: list[dict], results: list[dict]) -> dict | None:
    if choice is None:
        return None
    if "folder" not in choice:
        return choice
    for selected, registered in zip(selections, results, strict=True):
        if (selected["harness"], selected["folder"]) == (choice["harness"], choice["folder"]):
            reference = {"provider": registered["provider"], "id": registered["id"]}
            _require_enabled(reference)
            return reference
    raise SetupRefused("choose a checked login for the Master default")


def _require_enabled(reference: dict) -> None:
    provider, account_id = reference["provider"], reference["id"]
    if provider == GOOSE:
        available = any(row.enabled and (row.provider, row.id) == (provider, account_id)
                        for row in registry.load().accounts)
    else:
        available = any((row["provider"], row["id"]) == (provider, account_id) and not row["hidden"]
                        for row in leeway.accounts(refresh=True))
    if not available:
        raise SetupRefused("choose an enabled account for the Master default")


def _read(path: Path, choices: dict) -> dict:
    try:
        state = json.loads(path.read_text())
    except FileNotFoundError:
        state = {"choices": choices, "accounts": [], "created": None, "done": False}
    if state["choices"] != choices:
        raise SetupRefused("these choices differ from the recorded setup operation")
    return state


def _checked_folder(account: dict) -> Path:
    raw = account["folder"]
    if problem := account_login.folder_problem(raw, account["harness"]):
        raise SetupRefused(problem)
    folder = Path(os.path.realpath(raw))
    harness = HARNESS_BY_ID[account["harness"]]
    if harness.id != GOOSE and not harness.matches(folder, folder.parent):
        raise SetupRefused("the login folder does not match the chosen harness")
    return folder


def _apply_account(path: Path, state: dict, account: dict, folder: Path) -> dict:
    provider, name = account["harness"], account["name"]
    if provider == GOOSE:
        upstream = account.get("provider_config")
        registered = write.register_goose(folder, name, registry.provider_config(upstream) if upstream is not None else None)
        return {"id": registered.id, "provider": provider, "label": registered.label}
    row = _account_in_folder(leeway.accounts(refresh=True), provider, folder)
    if row is None:
        if state["created"] is not None:
            raise leeway.LeewayUnavailable(account_login.NOT_ADDED)
        row = account_login.add_to_book(str(folder), provider)
        if row is None:
            raise leeway.LeewayUnavailable(account_login.NOT_ADDED)
        state["created"] = _named(row)
        _save(path, state)
    # An unrecorded account or an account renamed since the add already has its own name.
    if _named(row) == state["created"] and (row.get("label") or row["id"]) != name:
        renamed = leeway.account_action("rename", row["id"], name)
        row = next((entry for entry in renamed if entry["id"] == row["id"]), None)
        if row is None:
            raise leeway.LeewayUnavailable(account_login.NOT_NAMED)
    return _named(row)


def _account_in_folder(book: list[dict], provider: str, folder: Path) -> dict | None:
    for row in book:
        if row["provider"] == provider:
            path = leeway.config_dir(row["id"], provider)
            if path and Path(os.path.realpath(path)) == folder:
                return row
    return None


def _named(row: dict) -> dict:
    return {"id": row["id"], "provider": row["provider"], "label": row.get("label") or row["id"]}


def _save(path: Path, state: dict) -> None:
    write.replace_atomically(path, json.dumps(state))
