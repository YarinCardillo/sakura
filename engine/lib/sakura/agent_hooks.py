"""JSON hook dispatcher shared by Claude Code and Codex."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

from sakura import guard, repository
from sakura.flow import brakes, denied_commands, task_repository


def run(agent: str, event: str) -> int:
    """Evaluate an agent hook payload and emit its documented decision object."""
    try:
        payload = json.loads(sys.stdin.read() or "{}")
        repo = _worktree(Path(payload.get("cwd") or Path.cwd()), os.environ)
        brakes.mark_hook_ran(repo, os.environ)
        output = _dispatch(repo, event, payload, agent)
    except Exception as error:
        output = _deny(f"sakura failed closed: {error}") if event == "PreToolUse" else _block(
            f"sakura failed closed: {error}"
        )
    if output:
        print(json.dumps(output))
    return 0


def _worktree(location: Path, env) -> Path:
    """The worktree the call works in. In a run the engine started (its task in `env`) the hook runs outside the
    sandbox in a folder the role writes, so the task and its repositories come from what the engine named and the
    workspace is verified against the state before any Git runs there (review 10, finding 1)."""
    task_id = env.get(brakes.TASK_ENV)
    if not task_id:
        return repository.root(location)
    _store, workspace = task_repository.open_for_helper(env, task_id, location)
    return workspace


def _dispatch(repo: Path, event: str, payload: dict, agent: str = "claude") -> dict | None:
    if os.environ.get(brakes.TASK_ENV):
        flow_decision = brakes.decision(repo, event, payload, os.environ)
        if flow_decision:
            return _decision(agent, *flow_decision)
    tool = payload.get("tool_name")
    session = str(payload.get("session_id") or "anonymous")
    if event == "PreToolUse" and tool in {"Bash", "Edit", "Write", "apply_patch", "ApplyPatch"}:
        guard.save_snapshot(repo, session)
        if guard.changed_since_snapshot(repo, session):
            return _deny("sakura guarded-file fingerprint changed or is unavailable")
    if event == "PreToolUse" and tool == "Bash":
        command = str((payload.get("tool_input") or {}).get("command", ""))
        # Codex's sandbox cannot deny these programs, so its hook does by name (the Claude cage has a rule and a profile).
        if agent == "codex" and (denied := denied_commands.reason(command)):
            return _deny(denied)
        decision = guard.command_decision(repo, command)
        return _decision(agent, *decision) if decision else None
    if event == "PostToolUse" and tool in {"Bash", "Edit", "Write", "apply_patch", "ApplyPatch"} and guard.changed_since_snapshot(repo, session):
        return _block("sakura guarded-file fingerprint changed or is unavailable")
    return None


def _decision(agent: str, kind: str, reason: str) -> dict:
    if kind == "ask" and agent == "claude":
        return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask", "permissionDecisionReason": reason}}
    if kind == "ask":
        return _deny("ask the user to run gh pr ready themselves")
    return _deny(reason)


def _deny(reason: str) -> dict:
    return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason}}


def _block(reason: str) -> dict:
    return {"decision": "block", "reason": reason}
