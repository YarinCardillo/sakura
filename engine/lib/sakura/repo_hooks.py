"""Optional execution of explicitly trusted repository hook commands."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

from sakura import engine_home


def trust(repo: Path, settings_file: Path | None = None) -> str:
    """Approve the exact current hash of a repository hook settings file."""
    source = settings_file or repo / ".claude/settings.json"
    content = source.read_bytes()
    digest = hashlib.sha256(content).hexdigest()
    target = engine_home.kit_dir(repo) / "trusted-hooks.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"path": str(source.resolve()), "sha256": digest}))
    return digest


def approved_commands(repo: Path) -> list[str]:
    """Read commands once, only when their settings file still has user approval."""
    source = repo / ".claude/settings.json"
    approval = engine_home.kit_dir(repo) / "trusted-hooks.json"
    try:
        trusted = json.loads(approval.read_text())
        content = source.read_bytes()
    except (OSError, ValueError):
        return []
    if trusted.get("path") != str(source.resolve()):
        return []
    if trusted.get("sha256") != hashlib.sha256(content).hexdigest():
        return []
    settings = json.loads(content)
    groups = settings.get("hooks", {}).get("PreToolUse", [])
    return [item["command"] for group in groups for item in group.get("hooks", []) if item.get("type") == "command"]


def run_approved(repo: Path) -> int:
    """Run only commands approved for the exact current settings content."""
    commands = approved_commands(repo)
    if not commands:
        return 0
    return max((subprocess.run(command, shell=True, cwd=repo).returncode for command in commands), default=0)
