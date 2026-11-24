"""Folders that hold an agent account's login: every agent sandbox the engine writes denies them.

A static floor of known folder names, every account folder the registry names (enabled or not), every dot folder
in the home whose name says Claude or Codex, and every folder detection finds. The registry must load: a cage
built without it could leave a login readable.
"""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from pathlib import Path

from sakura.accounts import detect, registry
from sakura.accounts.providers import GOOSE_NATIVE_DIRS

KNOWN_LOGIN_FOLDERS = (".claude", ".claude-work", ".codex", ".goose", *GOOSE_NATIVE_DIRS)
LOGIN_FOLDER_NAME = re.compile(r"\.[\w.-]*(claude|codex|goose)[\w.-]*", re.IGNORECASE)


def login_folders(home: Path, env: Mapping[str, str] = os.environ) -> list[Path]:
    """Every folder in `home` (and every registered one elsewhere) that holds or may hold an agent login.

    Raises registry.AccountsConfigError when the account registry cannot be read.
    """
    scoped = {**env, "HOME": str(home)}
    folders = [home / name for name in KNOWN_LOGIN_FOLDERS]
    folders += [account.config_dir for account in registry.load(env=scoped).accounts]
    if home.is_dir():
        folders += [entry for entry in home.iterdir() if entry.is_dir() and LOGIN_FOLDER_NAME.fullmatch(entry.name)]
    folders += [candidate.config_dir for candidate in detect.detect(home, scoped, dot_directories=True)]
    return sorted({Path(os.path.realpath(folder)) for folder in folders})
