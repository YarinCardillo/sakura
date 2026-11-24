"""Explicit installation of quota hooks into discovered Claude profiles."""

from __future__ import annotations

import json
import os
import shlex
import tempfile
from pathlib import Path

from sakura.accounts import detect, registry
from sakura.accounts.providers import ADAPTERS, CLAUDE_ALTERNATE_DIRS
from sakura.quota_enforcement.client import Advisor, DEFAULT_URL

EVENTS = ("PreToolUse", "SessionStart", "UserPromptSubmit")
ORCA_PROFILES = "Library/Application Support/orca/claude-profiles/*/home"


def discover(home: Path, env: dict) -> list[Path]:
    """Find existing Claude profile directories by structure and registry, deduplicated by path."""
    found = [home / ADAPTERS["claude"].default_dir.removeprefix("~/")]
    found += [home / name for name in CLAUDE_ALTERNATE_DIRS]
    if env.get("CLAUDE_CONFIG_DIR"):
        found.append(Path(env["CLAUDE_CONFIG_DIR"]).expanduser())
    found += [item.config_dir for item in detect.detect(home, env) if item.provider == "claude"]
    found += [item.config_dir for item in registry.load(env=env).accounts if item.provider == "claude"]
    found += sorted(home.glob(ORCA_PROFILES))
    return list(dict.fromkeys(path.resolve() for path in found if path.is_dir()))


def profiles(directories: list[Path], kit: Path, url: str = DEFAULT_URL) -> list[Path]:
    """Atomically merge hooks per profile; rerunning safely resumes a partial installation."""
    Advisor(url)
    written = []
    for directory in dict.fromkeys(path.expanduser().resolve() for path in directories):
        if not directory.is_dir():
            raise ValueError(f"profile directory does not exist: {directory}")
        path = (directory / "settings.json").resolve()
        value = json.loads(path.read_text()) if path.exists() else {}
        _merge(value, directory, kit, url)
        _write(path, value)
        written.append(path)
    return written


def _merge(value: dict, directory: Path, kit: Path, url: str) -> None:
    if not isinstance(value, dict) or not isinstance(value.setdefault("hooks", {}), dict):
        raise ValueError("profile settings and hooks must be objects")
    for event in EVENTS:
        groups = value["hooks"].setdefault(event, [])
        if not isinstance(groups, list):
            raise ValueError(f"{event} hooks must be a list")
        groups = [_without_owned(group) for group in groups]
        groups = [group for group in groups if group.get("hooks")]
        command = shlex.join([str(kit), "quota-hooks", "run", event, "--config-dir", str(directory),
                              "--advisor-url", url])
        group = {"hooks": [{"type": "command", "command": command, "timeout": 5}]}
        if event == "PreToolUse":
            group["matcher"] = "^(Agent|Task|Bash)$"
        value["hooks"][event] = [*groups, group]


def _without_owned(group: dict) -> dict:
    return {**group, "hooks": [hook for hook in group.get("hooks", []) if not _owned(hook)]}


def _owned(hook: dict) -> bool:
    if hook.get("type") != "command":
        return False
    try:
        args = shlex.split(hook.get("command", ""))
    except ValueError:
        return False
    # Hooks installed before the rename name the `agentkit` alias; they are still this installer's own.
    return bool(args and Path(args[0]).name in ("sakura", "agentkit") and args[1:3] == ["quota-hooks", "run"])


def _write(path: Path, value: dict) -> None:
    descriptor, temporary = tempfile.mkstemp(prefix=".quota-hooks-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            mode = path.stat().st_mode & 0o777 if path.exists() else 0o600
            os.fchmod(output.fileno(), mode)
            output.write(json.dumps(value, indent=2) + "\n")
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
