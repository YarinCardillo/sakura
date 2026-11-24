"""Arbiter: rules on a builder's requests to change tests, timeouts or scan allowlists."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

from sakura.flow import runner, workspaces
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import git
from sakura.flow.phases import runs
from sakura.flow.store import Ruling

PHASE = "arbiter"
KINDS = ("test_change", "timeout", "allowlist")
DECISIONS = ("grant", "deny")
ARBITER_ROLES = ("plan_reviewer", "code_reviewer")
NO_ROLE = "none"
NO_ARBITER_WHY = "no arbiter role is configured (plan_reviewer and code_reviewer are none)"
NO_RULING_WHY = "the arbiter gave no valid ruling for this request"
DENIED_COST = "the requested change stays blocked until the owner decides"


def requests_in(result: dict | None) -> list[dict]:
    """Return the well-formed `ruling_requests` of a builder result; empty when there are none."""
    found = result.get("ruling_requests") if isinstance(result, dict) else None
    if not isinstance(found, list):
        return []
    return [item for item in found if isinstance(item, dict) and isinstance(item.get("path"), str) and item.get("kind") in KINDS]


def rule(ctx: FlowContext, task_id: str, title: str, workspace: workspaces.Workspace, requests: list[dict],
         extra_env: dict[str, str]) -> PhaseOutcome | None:
    """Ask the arbiter about `requests` and store one ruling per request.

    Returns a blocking outcome when the arbiter run failed; denies everything
    (and ledgers it) when no arbiter role is configured.
    """
    wanted = [(_relative(workspace.path, item["path"]), item["kind"]) for item in requests]
    role = _arbiter_role(ctx)
    if role is None:
        _store(ctx, [_denied(path, kind, NO_ARBITER_WHY) for path, kind in wanted])
        return None
    head = git(workspace.path, "rev-parse", "HEAD")
    result = ctx.runner.run(runner.RunRequest(role, PHASE, runner.load_template(PHASE),
                                              _values(ctx, task_id, title, workspace, requests), workspace.path, extra_env))
    failure = runs.failure_outcome(result, role, PHASE) or runs.workspace_changes(workspace.path, role, PHASE, head)
    if failure:
        return failure
    _store(ctx, _rulings(workspace.path, wanted, result.result))
    return None


def rulings_text(ctx: FlowContext) -> str:
    """Prompt text that tells a builder what the arbiter ruled; empty before any ruling."""
    entries = ctx.store.ruling_entries()
    if not entries:
        return ""
    lines = [f"- {e.get('decision')} {e.get('kind')} {e.get('path')}: {e.get('why')}" for e in entries]
    return "\n\nRulings on your requests (a grant allows exactly that change; a deny does not):\n" + "\n".join(lines)


def _arbiter_role(ctx: FlowContext) -> str | None:
    roles = ctx.config.values["roles"]
    return next((role for role in ARBITER_ROLES if roles[role] != NO_ROLE), None)


def _values(ctx: FlowContext, task_id: str, title: str, workspace: workspaces.Workspace, requests: list[dict]) -> dict[str, str]:
    store = ctx.store
    return {"spec_path": str(store.spec), "contract_path": str(store.contract), "plan_md_path": str(store.plan_md),
            "task_id": task_id, "title": title, "repo_path": str(workspace.path), "requests": json.dumps(requests, indent=2)}


def _rulings(root: Path, wanted: list[tuple[str, str]], result: dict | None) -> list[Ruling]:
    given = {}
    for item in (result.get("rulings") if isinstance(result, dict) else None) or []:
        ruling = _parse(root, item)
        if ruling:
            given[(ruling.path, ruling.kind)] = ruling
    return [given.get(pair) or _denied(*pair, NO_RULING_WHY) for pair in wanted]


def _parse(root: Path, item: object) -> Ruling | None:
    if not isinstance(item, dict) or item.get("kind") not in KINDS or item.get("decision") not in DECISIONS:
        return None
    texts = [item.get("path"), item.get("why"), item.get("cost_if_wrong")]
    if not all(isinstance(text, str) and text.strip() for text in texts) or not isinstance(item.get("spec_change"), bool):
        return None
    return Ruling(_relative(root, item["path"]), item["kind"], item["decision"], item["why"], item["cost_if_wrong"], item["spec_change"])


def _denied(path: str, kind: str, why: str) -> Ruling:
    return Ruling(path, kind, "deny", why, DENIED_COST, False)


def _relative(root: Path, raw: str) -> str:
    """Repo-relative form of a requested path, matching the form the hook looks up."""
    path = Path(raw)
    resolved = (path if path.is_absolute() else root / path).resolve()
    try:
        return resolved.relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(resolved)


def _store(ctx: FlowContext, rulings: list[Ruling]) -> None:
    for ruling in rulings:
        ctx.store.add_ruling(ruling)
    pending = [{"path": r.path, "kind": r.kind} for r in rulings if r.spec_change and r.decision == "grant"]
    stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    detail = "; ".join(f"{r.decision} {r.kind} {r.path}" for r in rulings)

    def change(state: dict) -> None:
        known = state.setdefault("pending_spec_rulings", [])
        known.extend(entry for entry in pending if entry not in known)
        state["history"].append({"phase": PHASE, "status": "ruled", "at": stamp, "detail": detail})

    ctx.store.update_state(change)
