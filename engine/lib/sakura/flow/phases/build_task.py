"""One build task: builder rounds with focused tests and code review, in the task's own worktree."""

from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path

from sakura import slots, testing
from sakura.budget import routing
from sakura.flow import budget_gate, build_status, engine, final_head, flowfiles, logread, plan, process, render, roles, run_log, run_records
from sakura.flow import runner, workspaces
from sakura.flow.engine import FlowContext, PhaseOutcome, opus_gate
from sakura.flow.gitutil import git
from sakura.flow.notify import ESCALATION_EVENT
from sakura.flow.phases import arbiter, runs

BUILD_TASK_ENV = "SAKURA_FLOW_BUILD_TASK"
MAX_RULING_CYCLES = 3
MAX_NO_CHANGE_CHARS = 400
MAX_STRAY_FILES_SHOWN = 10
MAX_TAIL_LINES = 30
MAX_TAIL_CHARS = 3000
NO_COMMIT_FINDING = "builder made no commit"
FOCUSED_FAILED_FINDING = "focused tests failed (sakura test focused); fix them and commit"
PENDING, BUILDING, REVIEW = "pending", "building", "review"
APPROVED, MERGED = build_status.APPROVED, build_status.MERGED


class Halt(Exception):
    """A task cannot continue; carries the phase outcome that ends the build."""

    def __init__(self, outcome: PhaseOutcome) -> None:
        super().__init__(outcome.reason)
        self.outcome = outcome


def new_entry() -> dict:
    """The `state.build[id]` record of a task that has not started."""
    return {"status": PENDING, "workspace": "", "branch": "", "rounds": 0, "start": "", "last_head": "", "findings": [], "approved_head": "",
            "failures": 0}


def patch(ctx: FlowContext, task_id: str, **fields) -> dict:
    """Update fields of `state.build[task_id]` and return the new record."""
    return ctx.store.update_state(lambda state: state["build"][task_id].update(fields))["build"][task_id]


def work(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace) -> PhaseOutcome | None:
    """Run builder and review rounds until the task is approved; returns the blocking outcome otherwise."""
    try:
        while ctx.store.read_state()["build"][task.id]["status"] != APPROVED:
            _round(ctx, task, workspace)
    except Halt as halt:
        return halt.outcome
    return None


def _round(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace) -> None:
    entry = ctx.store.read_state()["build"][task.id]
    number = entry["rounds"] + 1
    findings = [] if entry["status"] == REVIEW else _build(ctx, task, workspace, entry)
    nothing_to_change = bool(ctx.store.read_state()["build"][task.id].get("no_change"))
    if not findings and engine.current_steps(ctx).code_review and not nothing_to_change:
        findings = _review(ctx, task, workspace, number)
    if findings:
        _fail(ctx, task, number, findings)
    else:
        # The commit the focused tests and the reviewer saw, never the workspace's HEAD read again.
        approved = ctx.store.read_state()["build"][task.id]["last_head"]
        patch(ctx, task.id, status=APPROVED, rounds=number, findings=[], approved_head=approved)


def _build(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace, entry: dict) -> list[str]:
    patch(ctx, task.id, status=BUILDING, no_change="")
    declared = _run_builder(ctx, task, workspace, entry)
    head = git(workspace.path, "rev-parse", "HEAD").strip()
    if head == entry["last_head"]:
        return _no_commit(ctx, task, workspace, entry, declared)
    patch(ctx, task.id, last_head=head)
    failed = _focused_findings(ctx, task, workspace)
    if failed:
        return failed
    if engine.current_steps(ctx).code_review:
        patch(ctx, task.id, status=REVIEW)
    return []


def _no_commit(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace, entry: dict,
               declared: dict | None) -> list[str]:
    """A builder round with no new commit: the task's finding, unless the builder says the repository already does
    what the task asks, changed nothing and committed nothing in this task. Then the task needs no change: it is
    approved at its start, skips code review, and its reason goes to the ledger and the report."""
    raw = (declared or {}).get("no_change")
    reason = render.strip_controls(raw).strip() if isinstance(raw, str) else ""
    if not reason or entry["last_head"] != entry["start"]:
        return [NO_COMMIT_FINDING + _refusal_clause(ctx, workspace)]
    left = flowfiles.uncommitted_beyond_caches(workspace.path)
    if left:
        return [_stray_files_finding(left)]
    patch(ctx, task.id, no_change=reason[:MAX_NO_CHANGE_CHARS])
    ctx.store.append_ledger("build", f"{task.id} needs no change", agent_values=[reason[:MAX_NO_CHANGE_CHARS]])
    return []


def _refusal_clause(ctx: FlowContext, workspace: workspaces.Workspace) -> str:
    """Why the builder's commits were refused, as its tool printed it, from the log of its latest run in this task's
    workspace (another task's builder is another lane of the build); "" when they were not."""
    here = os.path.realpath(workspace.path)
    runs_by_start = sorted((record for record in ctx.store.read_state().get("runs", {}).values()
                            if record["role"] == "builder" and record["phase"] == "build"
                            and os.path.realpath(record["cwd"]) == here),
                           key=lambda record: (record["started_at"], record["name"]))
    if not runs_by_start:
        return ""
    try:
        data = logread.read_log(run_records.log_path(ctx.store, runs_by_start[-1]["name"])).encode()
    except (OSError, logread.UnreadableLog):
        return ""
    refused = run_log.refused_commits(roles.RoleSpec(runs_by_start[-1]["provider"]), data)
    return f": the commit check refused its commits: {render.strip_controls(refused[-1])}" if refused else ""  # what its tool printed


def _stray_files_finding(left: list[str]) -> str:
    shown = ", ".join(render.strip_controls(name) for name in left[:MAX_STRAY_FILES_SHOWN])
    more = f" and {len(left) - MAX_STRAY_FILES_SHOWN} more" if len(left) > MAX_STRAY_FILES_SHOWN else ""
    return (f"{NO_COMMIT_FINDING}: the nothing-to-change ending was refused because {shown}{more} "
            f"{'is' if len(left) == 1 else 'are'} left in the worktree uncommitted; commit them, remove them, or stop the task")


def _focused_findings(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace) -> list[str]:
    """Run the focused tests as the engine does (outside the builder's sandbox, which cannot open local ports); the
    finding of a red run carries the tail of what the engine saw, since the builder cannot reproduce it there."""
    seen: list[str] = []
    try:
        with process.heavy_slot(ctx.config, "tests.focused", task_id=task.id,
                                timeout_seconds=ctx.config.values["timeouts"]["build"], stop_file=ctx.store.stop_file):
            timeouts = ctx.config.values["timeouts"]
            test_code = testing.run(workspace.path, "focused", command_runner=lambda command, cwd:
                                    _recorded(seen, process.run_test_command(
                                        ctx, command, cwd, budget_seconds=timeouts["build"],
                                        idle_seconds=timeouts["idle_seconds"])))
    except slots.SlotTimeout as error:
        raise Halt(PhaseOutcome("blocked", f"stalled waiting for heavy-work slot: tests.focused ({task.id})",
                                event=ESCALATION_EVENT)) from error
    except slots.SlotStopped as error:
        raise Halt(PhaseOutcome("stopped", f"stopped waiting for heavy-work slot: tests.focused ({task.id})")) from error
    except process.TestCommandInterrupted as error:
        if error.outcome.stopped:
            raise Halt(PhaseOutcome("stopped", f"stopped during tests.focused ({task.id})")) from error
        reason = ("timed out" if error.outcome.timed_out else
                  "output cap exceeded" if error.outcome.overflowed else
                  "log replaced" if error.outcome.log_replaced else
                  process.LEFT_RUNNING if error.outcome.left_running else "stalled")
        raise Halt(PhaseOutcome("blocked", f"{reason}: tests.focused ({task.id})", event=ESCALATION_EVENT)) from error
    return [] if test_code == 0 else [FOCUSED_FAILED_FINDING + _output_tail(seen)]


def _recorded(seen: list[str], result: tuple[int, str]) -> tuple[int, str]:
    seen.append(result[1])
    return result


def _output_tail(seen: list[str]) -> str:
    """The last lines of the last failing command's output, cleaned, as an addition to a finding."""
    lines = render.strip_controls(seen[-1]).strip().splitlines() if seen else []
    tail = "\n".join(lines[-MAX_TAIL_LINES:])[-MAX_TAIL_CHARS:]
    return f"\nThe engine's output, last lines:\n{tail}" if tail else ""


def _run_builder(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace, entry: dict) -> dict | None:
    """Run the builder; while it only asks for rulings, get them from the arbiter and rerun it (no fix round).

    Returns the builder's final result line, parsed."""
    for _ in range(MAX_RULING_CYCLES + 1):
        request = _builder_request(ctx, task, workspace, entry)
        result = ctx.runner.run(request)
        _require_success(result, "builder", "build")
        asked = arbiter.requests_in(result.result)
        if not asked or git(workspace.path, "rev-parse", "HEAD").strip() != entry["last_head"]:
            return result.result
        failure = arbiter.rule(ctx, task.id, task.title, workspace, asked, _env(task, workspace))
        if failure:
            raise Halt(failure)
    raise Halt(PhaseOutcome("blocked", f"{task.id} keeps asking for rulings after {MAX_RULING_CYCLES} arbiter rounds"))


def _builder_request(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace,
                     entry: dict) -> runner.RunRequest:
    """The builder run, routed by the task's class and failures; raises Halt at the Opus gate until it is approved."""
    failures = entry.get("failures", 0)
    approved = _opus_approved(ctx.store.read_state(), task.id)
    route = routing.route("builder", task.task_class, failures, ctx.config)
    if route.needs_approval and not approved:
        raise Halt(PhaseOutcome("await", f"{task.id} needs opus: {route.reason}{_estimate_text(ctx, task, route)}",
                                gate=opus_gate(task.id)))
    values = _build_values(ctx, task, workspace, entry["findings"])
    return runner.RunRequest("builder", "build", runner.load_template("build"), values, workspace.path,
                             _env(task, workspace), task_class=task.task_class, failures=failures, opus_approved=approved)


def _opus_approved(state: dict, task_id: str) -> bool:
    """Whether the owner approved Opus for this task of the current plan; an approval of another plan does not count."""
    approval = state["approvals"].get(opus_gate(task_id))
    return bool(approval) and approval.get("plan_sha256") == state["plan_sha256"]


def _estimate_text(ctx: FlowContext, task: plan.BuildTask, route: routing.Route) -> str:
    """The extra cost of the Opus run, appended to the gate reason; empty while no budget account is set."""
    return budget_gate.opus_extra_text(ctx, task)


def _review(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace, number: int) -> list[str]:
    """The reviewer judges the commit the focused tests passed at (`last_head`), named by its SHA, in a sealed copy of
    its own (`final_head.review_copy`), never in the builder's workspace or the task's repository."""
    entry = ctx.store.read_state()["build"][task.id]
    head = entry["last_head"]
    with final_head.review_copy(ctx, f"review-{task.id.lower()}", entry["start"], head) as (repository, copy):
        request = runner.RunRequest("code_reviewer", "review", runner.load_template("review"),
                                    _review_values(ctx, task, copy, entry), copy, _env(task, workspace),
                                    repository=repository)
        result = ctx.runner.run(request)
        _require_success(result, "code_reviewer", "review")
        changed = runs.workspace_changes(copy, "code_reviewer", "review", head)
    if changed:
        raise Halt(changed)
    verdict, findings = runs.parse_verdict(result.result)
    _record_review(ctx, task, number, verdict, findings)
    return [] if verdict == runs.APPROVE else findings


def _fail(ctx: FlowContext, task: plan.BuildTask, number: int, findings: list[str]) -> None:
    failures = ctx.store.read_state()["build"][task.id].get("failures", 0) + 1
    patch(ctx, task.id, status=BUILDING, rounds=number, findings=findings, failures=failures)
    ctx.store.append_ledger("build", f"{task.id} round {number} needs changes", agent_values=findings)
    limit = ctx.config.values["knobs"]["fix_rounds"]
    if number > limit:
        raise Halt(PhaseOutcome("blocked", f"{task.id} still needs changes after {limit} fix rounds: " + "; ".join(findings)))


def _require_success(result, role: str, phase: str) -> None:
    failure = runs.failure_outcome(result, role, phase)
    if failure:
        raise Halt(failure)


def _record_review(ctx: FlowContext, task: plan.BuildTask, number: int, verdict: str, findings: list[str]) -> None:
    detail = f"{task.id} round {number}" + (": " + "; ".join(findings) if findings else "")
    entry = {"phase": "review", "status": verdict.lower(), "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
             "detail": detail}
    ctx.store.update_state(lambda state: state["history"].append(entry))
    ctx.store.append_ledger("review", f"{task.id} review round {number}: {verdict}", agent_values=findings)


def _env(task: plan.BuildTask, workspace: workspaces.Workspace) -> dict[str, str]:
    ports = workspace.ports
    return {BUILD_TASK_ENV: task.id, "SAKURA_FLOW_PORT": str(ports[0]), "SAKURA_FLOW_PORTS": f"{ports[0]}-{ports[-1]}"}


def _paths(ctx: FlowContext) -> dict[str, str]:
    store = ctx.store
    return {"spec_path": str(store.spec), "contract_path": str(store.contract), "plan_md_path": str(store.plan_md)}


def _build_values(ctx: FlowContext, task: plan.BuildTask, workspace: workspaces.Workspace, findings: list[str]) -> dict[str, str]:
    ports = workspace.ports
    return {**_paths(ctx), "task_id": task.id, "title": task.title, "rows": ", ".join(task.rows),
            "files": ", ".join(task.files), "port": str(ports[0]), "ports": f"{ports[0]}-{ports[-1]}",
            "findings": _findings_text(findings), "rulings": arbiter.rulings_text(ctx)}


def _review_values(ctx: FlowContext, task: plan.BuildTask, copy: Path, entry: dict) -> dict[str, str]:
    return {**_paths(ctx), "task_id": task.id, "title": task.title, "repo_path": str(copy),
            "start": entry["start"], "branch": entry["last_head"],
            "flows": final_head.flows_review_text(ctx.store.read_state())}


def _findings_text(findings: list[str]) -> str:
    if not findings:
        return ""
    return "\n\nYour previous round was rejected; address every item and commit:\n" + "\n".join(f"- {item}" for item in findings)
