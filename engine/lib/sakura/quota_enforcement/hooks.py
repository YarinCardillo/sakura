"""Translate quota advice into Claude Code hook decisions."""

from __future__ import annotations

import logging
from pathlib import Path

from sakura.accounts.providers import ADAPTERS
from sakura.quota_enforcement import commands

LOG = logging.getLogger(__name__)
ROLES = (("explor", "explorer"), ("plan", "planner"), ("review", "reviewer"), ("evaluat", "evaluator"))


def evaluate(event: str, payload: dict, advisor, env: dict, *, role: str | None = None,
             task_class: str = "integration") -> dict | None:
    """Return a hook decision, allowing work with a warning if the advisor fails."""
    try:
        query = request(payload, "claude", role, task_class)
        if event == "SessionStart":
            return _session(advisor, env, query)
        if event == "UserPromptSubmit":
            advice = advisor.advise(query)
            if advice["verdict"] == "deny":
                return {"decision": "block", "reason": summary(advice)}
            return _context(event, summary(advice))
        if event == "PreToolUse" and payload.get("tool_name") in {"Agent", "Task"}:
            return _agent(payload, advisor, env, query)
        if event == "PreToolUse" and payload.get("tool_name") == "Bash":
            return _codex(payload, advisor, env, {**query, "agent": "codex"})
    except (OSError, ValueError, KeyError, TypeError) as error:
        _warn(error)
    return None


def request(payload: dict, agent: str, role: str | None = None, task_class: str = "integration") -> dict:
    """Build only the documented advise request fields, without task text."""
    kind = str(payload.get("tool_input", {}).get("subagent_type", "")).lower()
    role = role or next((value for token, value in ROLES if token in kind), "builder")
    return {"repo": str(payload.get("cwd") or Path.cwd()), "role": role,
            "task_class": task_class, "agent": agent}


def current_config(env: dict, agent: str = "claude") -> Path:
    """Resolve the current provider home, including its default when unset."""
    adapter = ADAPTERS[agent]
    default = Path(env.get("HOME") or Path.home()) / adapter.default_dir.removeprefix("~/")
    return Path(env.get(adapter.env_var) or default).expanduser().resolve()


def summary(advice: dict) -> str:
    """Describe the returned routing and retry time without guessing credentials."""
    text = f"Quota {advice['verdict']}: {advice['reason']}."
    if all(advice.get(key) for key in ("agent", "model", "effort", "config_dir")):
        text += (f" Use {advice['agent']} model={advice['model']} effort={advice['effort']} "
                 f"config_dir={advice['config_dir']}.")
    return text + (f" Retry at {advice['retry_at']}." if advice.get("retry_at") else "")


def _agent(payload: dict, advisor, env: dict, query: dict) -> dict:
    advice = advisor.advise(query)
    reason = summary(advice)
    if advice["verdict"] in {"deny", "wait"}:
        return deny(reason)
    if advice["agent"] != "claude" or Path(advice["config_dir"]).expanduser().resolve() != current_config(env):
        return deny(reason + " Relaunch on the advised account/provider.")
    return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": reason,
                                   "updatedInput": {**payload["tool_input"], "model": advice["model"]}}}


def deny(reason: str) -> dict:
    """Deny a tool call using Claude Code's event-specific output format."""
    return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                   "permissionDecisionReason": reason}}


def _context(event: str, text: str) -> dict:
    return {"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}


def _warn(error: Exception) -> None:
    LOG.warning("quota advisor unavailable; allowing work (%s)", type(error).__name__)


def _session(advisor, env: dict, query: dict) -> dict:
    output = _context("SessionStart", summary(advisor.advise(query)))
    try:
        warning = _account_warning(advisor.status(), env)
        if warning:
            output["systemMessage"] = warning
            output["hookSpecificOutput"]["additionalContext"] += " " + warning
    except (OSError, ValueError, KeyError, TypeError) as error:
        _warn(error)
    return output


def _account_warning(status: dict, env: dict) -> str | None:
    threshold = float(env.get("SAKURA_QUOTA_THRESHOLD_PCT", "95"))
    for account in status["accounts"]:
        paths = [account["config_dir"], *account.get("aliases", [])]
        if account["provider"] != "claude" or current_config(env) not in {Path(path).expanduser().resolve() for path in paths}:
            continue
        current = account.get("seven_day", {}).get("pct")
        forecast = account.get("forecast", {}).get("next_round_pct")
        values = [value for value in (current, forecast) if isinstance(value, (int, float))]
        if values and max(values) >= threshold:
            stale = " (stale snapshot)" if account.get("stale") else ""
            return (f"Quota warning: current account {account['label']} reaches {max(values):g}% "
                    f"against {threshold:g}% threshold{stale}.")
    return None


def _codex(payload: dict, advisor, env: dict, query: dict) -> dict | None:
    command = payload.get("tool_input", {}).get("command", "")
    for launch in commands.launches(command, env, query["repo"]):
        advice = advisor.advise({**query, "repo": str(launch.repo)})
        if advice["verdict"] in {"deny", "wait"}:
            return deny(summary(advice))
        if not launch.is_resolved:
            return deny(summary(advice) + " Relaunch with absolute literal account and repository paths.")
        if (advice["agent"] != "codex" or launch.model != advice["model"]
                or Path(advice["config_dir"]).expanduser().resolve() != current_config(launch.env, "codex")):
            return deny(summary(advice) + " Relaunch codex exec with the advised model/account.")
    return None
