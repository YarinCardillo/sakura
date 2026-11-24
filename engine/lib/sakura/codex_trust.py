"""Strict selection of installer-owned Codex hooks eligible for trust."""

from __future__ import annotations

import hashlib
import json
import shlex
import tomllib
from pathlib import Path

from sakura import engine_home, repository

HOOK_EVENTS = ("PreToolUse", "PostToolUse")
TRUST_FILE = "codex-trust.json"


def canonical_commands() -> list[str]:
    """Return the exact Codex hook commands the installer writes."""
    executable = Path(__file__).resolve().parents[2] / "bin/sakura"
    return [shlex.join([str(executable), "hook", "codex", event]) for event in HOOK_EVENTS]


def record(repo: Path) -> str:
    """Store the digest of the canonical hook commands as trusted; returns the digest."""
    digest = _digest()
    path = engine_home.kit_dir(repo) / TRUST_FILE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"sha256": digest}))
    return digest


def is_recorded(repo: Path) -> bool:
    """Return whether the stored digest equals the current canonical hook commands."""
    try:
        stored = json.loads((engine_home.kit_dir(repo) / TRUST_FILE).read_text())
    except (OSError, ValueError, repository.RepositoryError):
        return False
    return isinstance(stored, dict) and stored.get("sha256") == _digest()


def eligible_commands(repo: Path) -> list[str]:
    """Return only exact sakura Codex hook commands written in this repository."""
    source = repo / ".codex/hooks.json"
    try:
        data = json.loads(source.read_text())
    except (OSError, ValueError):
        return []
    kit = Path(__file__).resolve().parents[2]
    expected = {f"hook codex {event}" for event in HOOK_EVENTS}
    commands = []
    for groups in data.get("hooks", {}).values():
        for group in groups:
            for hook in group.get("hooks", []):
                command = hook.get("command")
                if isinstance(command, str) and _is_exact(command, kit, expected):
                    commands.append(command)
    return commands


def _is_exact(command: str, kit: Path, expected: set[str]) -> bool:
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    if len(argv) != 4 or argv[1:3] != ["hook", "codex"] or " ".join(argv[1:]) not in expected:
        return False
    try:
        executable = Path(argv[0]).resolve(strict=True)
    except OSError:
        return False
    canonical = shlex.join([str(kit / "bin/sakura"), *argv[1:]])
    return executable == (kit / "bin/sakura").resolve() and command == canonical


def workspace_hooks_exact(workdir: Path) -> str | None:
    """Return why the workspace Codex hooks are not exactly the sakura pair, or None when they are.

    Every hook entry of `.codex/hooks.json` (any event, group or matcher) must be
    one of the canonical sakura commands, both must be present, and
    `.codex/config.toml` must not define hooks.
    """
    problem = _hooks_json_problem(workdir / ".codex/hooks.json")
    return problem or _config_toml_problem(workdir / ".codex/config.toml")


def _hooks_json_problem(path: Path) -> str | None:
    problem = f"{path} must contain exactly the two sakura Codex hooks and nothing else"
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        return f"{path} is missing or not a plain sakura hook file"

    if not isinstance(data, dict) or set(data) != {"hooks"}:
        return problem
    hooks = data["hooks"]
    if not isinstance(hooks, dict) or set(hooks) != set(HOOK_EVENTS):
        return problem

    commands = dict(zip(HOOK_EVENTS, canonical_commands()))
    for event in HOOK_EVENTS:
        groups = hooks[event]
        if not isinstance(groups, list) or len(groups) != 1 or not isinstance(groups[0], dict):
            return problem
        group = groups[0]
        if set(group) not in ({"hooks"}, {"hooks", "matcher"}):
            return problem
        if group.get("matcher", "*") != "*":
            return problem
        entries = group["hooks"]
        if not isinstance(entries, list) or len(entries) != 1 or not isinstance(entries[0], dict):
            return problem
        entry = entries[0]
        if set(entry) != {"type", "command"} or entry["type"] != "command" or entry["command"] != commands[event]:
            return problem
    return None


def _config_toml_problem(path: Path) -> str | None:
    if not path.exists():
        return None
    try:
        table = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError):
        return f"{path} cannot be read"
    return f"{path} defines hooks" if "hooks" in table else None


def _digest() -> str:
    return hashlib.sha256(json.dumps(canonical_commands()).encode()).hexdigest()
