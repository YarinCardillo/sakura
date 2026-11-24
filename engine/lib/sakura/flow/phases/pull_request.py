"""Create a verified draft pull request after pushing the exact tested head: the engine's own (`flow/final_head.py`)."""

from __future__ import annotations

import json
import os
import re
from pathlib import Path

from sakura.flow import completion, contract, delivery, final_head, git_config, overlap, owner_marks, process
from sakura.flow.engine import OVERLAP_GATE, FlowContext, PhaseOutcome
from sakura import git_program, profile, programs
from sakura.flow import gitutil
from sakura.flow.gitutil import GitError, git
from sakura.flow.render import strip_controls
from sakura.flow.phases import integrate

PHASE = "pr"
GH = "gh"
PR_BODY = "pr-body.md"
PR_URL = re.compile(r"https?://\S+/pull/\d+")
# What `gh` says when the remote is not a GitHub host it knows, or there is no remote.
NOT_GITHUB_MESSAGES = ("known github host", "no git remotes found")
URL_USERINFO = re.compile(r"(?<=://)[^/@\s]*@")


def run(ctx: FlowContext) -> PhaseOutcome:
    """Push the engine's head as the task branch and open a draft pull request.

    The head is the engine's own integration of the approved work, checked against its store; no workspace or branch
    is read, so a commit a survivor adds to the task branch is never pushed (review 11, finding 1).
    """
    try:
        state = ctx.store.read_state()
        rows = contract.load(ctx.store, state["contract_sha256"])
        if state.get("full_green_inputs_sha256") != integrate.full_tests_inputs_hash(ctx):
            return PhaseOutcome("blocked", "full tests have not passed on the current test configuration")
        try:
            store, head = final_head.proved(ctx)
        except GitError as error:
            return PhaseOutcome("blocked", f"not pushed: {error}")
        problem = _precondition(state, rows, ctx.steps, head, ctx.store.task_id, ctx.store.root, _checks_on(ctx))
        if problem:
            return PhaseOutcome("blocked", problem)
        if head == state.get("base_commit"):
            return _nothing_to_deliver(ctx, state, head)
        delivered = state.get("pr", {})
        if delivered.get("head") == head and (delivered.get("url") or delivered.get("note")):
            return PhaseOutcome("next")
        push_url = state.get("push_url", "")
        if not push_url:
            return PhaseOutcome("blocked", "no push URL was recorded when the task started (the repository had no origin)")
        forbidden = _unpushable_branch(ctx.repo, state["branch"], state.get("base_branch", ""))
        if forbidden:
            return PhaseOutcome("blocked", forbidden)
        failure = _deliver_branch(ctx, state, store, head, push_url)
        if failure:
            return failure
        existing = _existing_draft(ctx, state, head)
        if isinstance(existing, PhaseOutcome):
            return _delivered_without_host(ctx, state, head, push_url) if existing.kind == "skip" else existing
        if existing:
            delivery.record(ctx.store, delivery.PULL_REQUEST, head, state["branch"], url=existing)
            return PhaseOutcome("next")
        return _create(ctx, state, rows, head)
    except (contract.ContractError, GitError) as error:
        return PhaseOutcome("blocked", str(error))


def _deliver_branch(ctx: FlowContext, state: dict, store: Path, head: str, push_url: str) -> PhaseOutcome | None:
    """Check the overlap, push and verify the push, and record it as the lane's delivery (`delivery.record`, upgraded
    later when a pull request is found or opened), all under the project's delivery lock: another lane's check finds either
    nothing of this one or the recorded delivery, never a push that is not recorded yet. None when the branch is delivered."""
    timeouts = ctx.config.values["timeouts"]
    try:
        with delivery.project_lock(ctx.repo, timeouts["build"], ctx.store.stop_file.exists):
            held = _hold_for_overlap(ctx, state, store, head)
            if held:
                return held
            failure = _push(ctx, store, head, state["branch"], push_url)
            if failure:
                return failure
            if _remote_head(ctx.repo, state["branch"], push_url) != head:
                return PhaseOutcome("blocked", "push not verified")
            delivery.record(ctx.store, delivery.kind_of_push(push_url), head, state["branch"])
    except delivery.DeliveryLockUnavailable as unavailable:
        stopped = ctx.store.stop_file.exists()
        return PhaseOutcome("stopped" if stopped else "blocked", "stopped waiting for another lane's delivery"
                            if stopped else str(unavailable))
    return None


def _hold_for_overlap(ctx: FlowContext, state: dict, store: Path, head: str) -> PhaseOutcome | None:
    """Wait for the person when merging this head with a head another lane of the project delivered conflicts; their
    approval (`sakura flow approve <task> overlap`) covers this head and exactly the conflicts they were shown, and is
    checked again here every time delivery proceeds, so a lane that delivers another conflicting head in between
    makes the delivery wait again."""
    conflicts = overlap.delivered_conflicts(ctx, state, store, head)
    if not conflicts:
        return None
    revision = overlap.revision(head, conflicts)
    approved = (state.get("approvals") or {}).get(OVERLAP_GATE, {})
    if approved.get("head") == head and approved.get("revision") == revision:
        return None
    first = conflicts[0]
    shown = ", ".join(first["files"]) or "files not shown"
    ctx.store.update_state(lambda current: current.update(overlap={
        "head": head, "lane": first["lane"], "files": first["files"], "revision": revision, "conflicts": conflicts}))
    others = f" (and {len(conflicts) - 1} more lane{'s' if len(conflicts) > 2 else ''})" if len(conflicts) > 1 else ""
    return PhaseOutcome("await", f"merging with {first['lane']}{others}, which delivered first, conflicts in {shown}",
                        gate=OVERLAP_GATE)


def _checks_on(ctx: FlowContext) -> bool:
    """Whether the lane's preset runs the checks step, read from the config the lane runs with now (a preset the person
    switched to mid-way is already in it, `engine.apply_inbox`)."""
    return bool(ctx.config.values["knobs"]["checks"])


def pr_body(state: dict, rows: list[contract.Row], steps, rulings: list[dict], report_path: str,
            checks_on: bool = True) -> str:
    """Build a pull request body from engine state, contract rows and rulings; `checks_on` False says plainly that
    the preset switched the checks off, so no contract row was run."""
    lines = [f"Task: {state.get('task', '')}"]
    if not checks_on:
        lines.append("Verification: checks were switched off by the preset, so no contract row was run")
    elif steps.evaluate:
        passed = sum(row.passes for row in rows)
        lines.append(f"Verification: {passed}/{len(rows)} contract rows passed by the evaluator")
    else:
        lines.append(f"Verification: not verified by an evaluator (preset {state['preset']})")
    by_prover = [f"{len(contract.proven_by(rows, steps, prover))} {prover}" for prover in contract.PROVERS
                 if checks_on and contract.proven_by(rows, steps, prover)]
    if by_prover:
        lines.append("Provers: " + ", ".join(by_prover))
    disabled = []
    if not steps.plan_review:
        disabled.append("plan review")
    if not steps.code_review:
        disabled.append("code review")
    if disabled:
        lines.append("Review: not reviewed: " + ", ".join(disabled))
    spec_rulings = [entry for entry in rulings if entry.get("spec_change")]
    if spec_rulings:
        lines += ["", "Spec-changing rulings (pending owner confirmation):"]
        lines.extend(f"- {entry['decision']} {entry['kind']} {entry['path']}: {entry['why']}" for entry in spec_rulings)
    lines += ["", f"Report: {report_path}"]
    return "\n".join(lines) + "\n"


def _precondition(state: dict, rows: list[contract.Row], steps, head: str, task_id: str = "",
                  root: Path | None = None, checks_on: bool = True) -> str:
    """Why the engine may not push or open the pull request yet: while the checks step is on, every row must be green,
    each pass bound to this contract and this head (a check or evaluator receipt, the person's mark). With the checks
    switched off by the preset the rows were never meant to run, so none is required; the full tests still are."""
    if not checks_on:
        return "" if state.get("full_green_head") == head else "full tests have not passed on the current head"
    digest = contract.rows_digest(rows)
    open_rows = [row for row in rows if not completion.row_proven(state, row, steps, digest, head, root)]
    owner = [row for row in open_rows if contract.prover_of(row, steps) == contract.OWNER]
    if len(owner) < len(open_rows):
        return "contract rows are still open"
    if owner:
        return owner_marks.blocked_reason(task_id, owner)
    if state.get("full_green_head") != head:
        return "full tests have not passed on the current head"
    return ""


def _unpushable_branch(repo: Path, branch: str, base_branch: str) -> str:
    """Why `branch` may not be pushed, or ''.

    It may not be a protected branch (main, master, the profile's protected and integration branches, the base of
    this task, the remote's default), live under the integration prefix, or break the profile's `branch_pattern`.
    """
    settings = profile.load(repo).values
    if branch in profile.all_protected_names(repo, base_branch, profile.remote_default_branch(repo)):
        return f"refusing to push {branch!r}: it is a protected branch (main, master, a protected, integration or default branch)"
    if branch.startswith(profile.INTEGRATION_BRANCH_PREFIX):
        return f"refusing to push {branch!r}: branches under {profile.INTEGRATION_BRANCH_PREFIX} belong to the integration flow"
    if not re.fullmatch(settings["branch_pattern"], branch):
        return f"refusing to push {branch!r}: it does not match the profile's branch_pattern"
    return ""


def _push(ctx: FlowContext, store: Path, head: str, branch: str, url: str) -> PhaseOutcome | None:
    """Push `head`, the engine's head whose receipts were validated, as `branch` to the URL recorded when the task
    started, named explicitly: no remote or rewrite rule of the config applies, and no branch ref is read (a survivor
    may move it). The bytes come from the engine's store (`flow/sealed.py`), where the engine made or sealed that
    head, never from a workspace's object database; the push names it by its ID. How to sign in to and reach the
    remote comes from the person's repository config (`git_config.transport_entries`), read here, outside every cage:
    the task's own repository holds none of it."""
    if gitutil.is_command_transport(url):
        return PhaseOutcome("blocked", f"refusing to push to {url!r}: the ext:: transport runs a program")
    argv, env = gitutil.command(store, "push", url, f"{head}:refs/heads/{branch}")
    env.update(_transport_environment(ctx.repo))
    outcome, output, log = _command(ctx, store, "push", argv, env)
    _say(output)
    if outcome.stopped:
        return PhaseOutcome("stopped", "stopped during push")
    if outcome.stalled or outcome.timed_out:
        return PhaseOutcome("blocked", f"push stalled; see {log}")
    if outcome.exit_code != 0:
        return PhaseOutcome("blocked", f"git push failed: {output.strip()}")
    return None


def _transport_environment(repo: Path) -> dict[str, str]:
    """The person's credential and HTTP settings for a delivery call the engine makes outside their repository."""
    return gitutil.config_environment(git_config.transport_entries(git_program.common_dir_of(repo)))


def _remote_head(repo: Path, branch: str, url: str) -> str:
    """The remote's head of `branch`, asked from the person's repository (its config signs in to the remote)."""
    output = git(repo, "ls-remote", url, f"refs/heads/{branch}")
    return output.split()[0] if output.split() else ""


def _nothing_to_deliver(ctx: FlowContext, state: dict, head: str) -> PhaseOutcome:
    """The task branch is still the base: every build task found nothing to change, so there is nothing to push."""
    note = "nothing to change: the repository already does this, so no branch was pushed"
    ctx.store.update_state(lambda current: current.update(pr={"url": "", "head": head, "note": note}))
    return PhaseOutcome("skip", note)


def _delivered_without_host(ctx: FlowContext, state: dict, head: str, push_url: str) -> PhaseOutcome:
    """The branch is pushed and verified but there is no GitHub host to open a pull request on: that is delivery,
    with one line saying where the branch is and what the person does next."""
    note = f"branch {state['branch']} pushed to {URL_USERINFO.sub('', push_url)}; open the pull request yourself"
    delivery.record(ctx.store, delivery.kind_of_push(push_url), head, state["branch"], note=note)
    print(note, flush=True)
    return PhaseOutcome("skip", note)


def _existing_draft(ctx: FlowContext, state: dict, head: str) -> str | PhaseOutcome | None:
    """The open draft for this head and base, None when there is none, or an outcome: `skip` when `gh` has no
    GitHub host to ask (a plain remote, an unknown host, no `gh` at all), else why the lookup failed. `gh` runs in
    the person's repository, whose remotes name the host (the task's own repository has none)."""
    argv = ["gh", "pr", "list", "--state", "open", "--head", state["branch"], "--base", state["base_branch"],
            "--json", "url,headRefOid,baseRefName,isDraft"]
    try:
        outcome, output, log = _command(ctx, ctx.repo, "pull-request-lookup", argv)
    except (FileNotFoundError, programs.ProgramMissing):
        return PhaseOutcome("skip", "the gh command is not installed")
    except programs.ProgramError as error:
        return PhaseOutcome("blocked", str(error))
    if outcome.stopped:
        return PhaseOutcome("stopped", "stopped while checking for an existing pull request")
    if outcome.exit_code and any(text in output.lower() for text in NOT_GITHUB_MESSAGES):
        # Delivery to a plain remote: gh's raw line stays in the log, the person gets our note alone.
        return PhaseOutcome("skip", "the remote is not a GitHub host")
    _say(output)
    if outcome.stalled or outcome.timed_out or outcome.exit_code != 0:
        cause = strip_controls(next(iter(output.strip().splitlines()), "")).strip()
        return PhaseOutcome("blocked", f"pull request lookup failed{f': {cause}' if cause else ''}; see {log}")
    try:
        entries = json.loads(output)
    except ValueError:
        return PhaseOutcome("blocked", "pull request lookup returned invalid JSON")
    if not isinstance(entries, list):
        return PhaseOutcome("blocked", "pull request lookup returned an invalid result")
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("headRefOid") != head:
            continue
        if entry.get("baseRefName") != state["base_branch"] or not entry.get("url"):
            continue
        if not entry.get("isDraft"):
            return PhaseOutcome("blocked", "an existing pull request for this head is not a draft")
        return entry["url"]
    return None


def _create(ctx: FlowContext, state: dict, rows: list[contract.Row], head: str) -> PhaseOutcome:
    body_path = ctx.store.root / PR_BODY
    body_path.write_text(pr_body(state, rows, ctx.steps, ctx.store.ruling_entries(), str(ctx.store.report),
                                   _checks_on(ctx)))
    title = _title(ctx.store.spec, ctx.store.task_id)
    argv = ["gh", "pr", "create", "--draft", "--base", state["base_branch"], "--head", state["branch"],
            "--title", title, "--body-file", str(body_path)]
    try:
        outcome, output, log = _command(ctx, ctx.repo, "pull-request", argv)
        _say(output)
    except programs.ProgramError as error:
        return PhaseOutcome("blocked", str(error))
    if outcome.stopped:
        return PhaseOutcome("stopped", "stopped during pull request creation")
    if outcome.stalled or outcome.timed_out:
        return PhaseOutcome("blocked", f"pull request creation stalled; see {log}")
    if outcome.exit_code != 0:
        return PhaseOutcome("blocked", f"gh pr create failed: {output.strip()}")
    match = PR_URL.search(output)
    if not match:
        return PhaseOutcome("blocked", "gh pr create returned no pull request URL")
    delivery.record(ctx.store, delivery.PULL_REQUEST, head, state["branch"], url=match.group(0))
    return PhaseOutcome("next")


def _command(ctx: FlowContext, cwd: Path, name: str, argv: list[str], env: dict[str, str] | None = None):
    """Run `argv` supervised; `gh` is the engine's pinned copy (never one a repository put on PATH) with an
    environment made for it. Raises programs.ProgramError when there is none. What the command said is in its log
    and in the result; the caller decides whether the person sees it (`_say`)."""
    if argv[0] == GH:
        executable = programs.path(GH, cwd)
        argv, env = [executable, *argv[1:]], programs.environment("gh", executable)
    ctx.store.operations_dir.mkdir(parents=True, exist_ok=True)
    log = ctx.store.operations_dir / f"{name}.log"
    log.unlink(missing_ok=True)
    timeouts = ctx.config.values["timeouts"]
    # The engine's own uncaged commands: a push may leave the person's ssh connection running by design.
    outcome = process.monitored_run(argv, cwd=cwd, env=env or process.child_environment(os.environ), log_path=log,
                                    budget_seconds=timeouts["build"], idle_seconds=timeouts["idle_seconds"],
                                    stop_file=ctx.store.stop_file, on_start=lambda _pid: None, own_group=False)
    return outcome, log.read_text(errors="replace"), log


def _say(output: str) -> None:
    print(output, end="", flush=True)


def _title(spec: Path, task_id: str) -> str:
    if spec.is_file():
        for line in spec.read_text(errors="replace").splitlines():
            if line.startswith("# "):
                return line[2:].strip()
    return task_id
