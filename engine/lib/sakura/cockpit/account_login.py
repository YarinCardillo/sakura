"""Adding an agent account from the app: a new login folder (`g`) and the account book entry (`j`).

Leeway's account book is where the apps keep accounts (renames, hidden ones), so an account is added there, through the
pinned helper (`usage/leeway.py`), and named in a second call. The engine never runs a login and never reads what is in
a login folder: it makes an empty private folder and says the command the person runs themselves in a terminal.

An account folder is only taken when it is a dot folder of the person's home whose name holds `claude` or `codex`
(`accounts/folders.py`): those are the folders every agent cage denies, so a lane can never read the login that is
added here. Both sentences below are fixed: none repeats a path or the helper's own text, which goes to stderr.
"""

from __future__ import annotations

import json
import os
import shlex
import stat
import sys
from pathlib import Path

from sakura import programs
from sakura.accounts import folders, registry, write
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.flow import cage, exits
from sakura.flow.render import strip_controls
from sakura.flow.surface import KIT_BIN
from sakura.setup import harnesses
from sakura.usage import leeway

PROVIDERS = tuple(ADAPTERS)
LOGIN_COMMANDS = {"claude": ("claude", "auth", "login"), "codex": ("codex", "login")}
FIRST_NUMBER = 2
LAST_NUMBER = 99
FOLDER_MODE = 0o700
OTHERS_WRITE = stat.S_IWGRP | stat.S_IWOTH
NOT_ABSOLUTE = "the path is not absolute; choose the full path of the login folder"
NOT_A_LOGIN_FOLDER = ("the folder must be a dot folder of your home whose name holds claude or codex, "
                      "such as ~/.claude-2")
NOT_A_FOLDER = "the path is not a folder"
NOT_YOURS = "the folder is not yours, or others can write it; choose a private folder of your own"
NEEDS_LEEWAY = "adding an account needs the Leeway helper, which was not found"
ALREADY_ADDED = "that folder is already an account"
NAME_DASH = "a name cannot start with a dash"
NOT_ADDED = "Leeway did not add the account"
NOT_NAMED = "Leeway added the account but did not take its name; rename it from the accounts list"
NO_FREE_FOLDER = "no free login folder name is left in your home"
CANNOT_MAKE = "the login folder could not be made"


def new_login_folder(provider: str) -> tuple[int, str]:
    """Make an empty private login folder for `provider` in the home (`.claude-2`, `.claude-3`, ... or `.codex-N`) and
    return the folder and the command to log in as JSON: `provider`, `folder`, `command` (one line for a terminal),
    `argv` and `env` (the same command by parts). Nothing is run and nothing is read."""
    home = cage.account_home()
    if provider == GOOSE:
        try:
            if not harnesses.qualify(GOOSE).setup_safe:
                raise programs.ProgramError("safe_setup_required")
        except programs.ProgramError as error:
            return exits.USAGE, str(error)
    for number in range(FIRST_NUMBER, LAST_NUMBER + 1):
        folder = home / f".{provider}-{number}"
        try:
            os.mkdir(folder, FOLDER_MODE)
        except FileExistsError:
            continue
        except OSError as error:
            return exits.FAILED, _unsaid("login folder", error, CANNOT_MAKE)
        return exits.OK, json.dumps(_login(provider, folder))
    return exits.USAGE, NO_FREE_FOLDER


def add_account(folder: str, provider: str, label: str, upstream: str | None = None) -> tuple[int, str]:
    """Add the login folder `folder` as a `provider` account named `label`; return the account as JSON (`id`,
    `provider`, `label`) or one fixed sentence saying why not."""
    problem = _label_problem(label) or folder_problem(folder, provider)
    if problem:
        return exits.USAGE, problem
    if provider == GOOSE:
        try:
            added = write.register_goose(Path(folder), label, upstream)
        except (registry.AccountsConfigError, OSError) as error:
            return exits.USAGE, str(error) if isinstance(error, registry.AccountsConfigError) else NOT_ADDED
        return exits.OK, json.dumps({"id": added.id, "provider": GOOSE, "label": added.label})
    if not leeway.is_available():
        return exits.USAGE, NEEDS_LEEWAY
    try:
        added = add_to_book(os.path.realpath(folder), provider)
    except leeway.LeewayError as error:
        return exits.FAILED, _unsaid("account", error, NOT_ADDED)
    if added is None:
        return exits.USAGE, ALREADY_ADDED
    try:
        leeway.account_action("rename", added["id"], label)
    except leeway.LeewayError as error:
        return exits.FAILED, _unsaid("account", error, NOT_NAMED)
    return exits.OK, json.dumps({"id": added["id"], "provider": provider, "label": label})


def add_to_book(folder: str, provider: str) -> dict | None:
    """The account Leeway made for `folder`; None when the book gained none (the folder was already there)."""
    known = {row["id"] for row in leeway.accounts()}
    added = [row for row in leeway.account_add(folder, provider) if row["id"] not in known and row["provider"] == provider]
    return added[0] if len(added) == 1 else None


def _label_problem(label: str) -> str | None:
    """The helper's parser has no `--` terminator, so a name that starts with a dash would be read as an option."""
    return NAME_DASH if label.startswith("-") else None


def folder_problem(folder: str, provider: str | None = None) -> str | None:
    """Why `folder` cannot be an account's login folder, or None. The folder is resolved once; links are followed."""
    if not os.path.isabs(folder):
        return NOT_ABSOLUTE
    chosen = Path(os.path.realpath(folder))
    home = cage.account_home().resolve()
    if provider == GOOSE:
        if not harnesses.HARNESS_BY_ID[GOOSE].is_candidate(chosen, home):
            return "choose a private Goose configuration folder in your home"
    elif (chosen.parent != home or not folders.LOGIN_FOLDER_NAME.fullmatch(chosen.name)
          or harnesses.HARNESS_BY_ID[GOOSE].matches(chosen, home)):
        return NOT_A_LOGIN_FOLDER
    try:
        info = chosen.lstat()
    except OSError:
        return NOT_A_FOLDER
    if not stat.S_ISDIR(info.st_mode):
        return NOT_A_FOLDER
    return NOT_YOURS if info.st_uid != os.getuid() or info.st_mode & OTHERS_WRITE else None


def _login(provider: str, folder: Path) -> dict:
    if provider == GOOSE:
        argv, env = [str(KIT_BIN), "setup", "configure-goose", f"--folder={folder}"], {}
    else:
        argv, env = list(LOGIN_COMMANDS[provider]), {ADAPTERS[provider].env_var: str(folder)}
    command = shlex.join(["env", *(f"{name}={value}" for name, value in env.items()), *argv])
    return {"provider": provider, "folder": str(folder), "command": command, "argv": argv, "env": env}


def _unsaid(what: str, error: Exception, sentence: str) -> str:
    """`sentence`, after the error's own text went to stderr, where only the machine's owner reads it."""
    print(f"cockpit: add {what}: {strip_controls(str(error))}", file=sys.stderr)
    return sentence
