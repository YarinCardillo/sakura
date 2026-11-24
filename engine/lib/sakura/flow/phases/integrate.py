"""The checks around the engine's integration of approved work (`flow/final_head.py`): protected paths and the full
test run on the engine's head."""

from __future__ import annotations

import contextlib
import os
import hashlib
import json
import time
from pathlib import Path

from sakura import skips
from sakura import slots
from sakura.flow import cage, checks, completion, final_head, globs, inflation, process, repository_commands, workspaces
from sakura.flow.final_head import MergeBlocked
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git, tail
from sakura.flow.notify import ESCALATION_EVENT

FULL_LOG = "full-tests.log"
PROTECTED_GLOBS = [".sakura/**"]
ADDED = "A"
FULL_TESTS_PASSED, FULL_TESTS_NOT_CONFIGURED = "passed", "not configured"
NO_FULL_TESTS_LINE = ("tests.full is empty, so the engine ran none of the repository's own tests (only the contract "
                      "rows); to have it run them add [tests] full = [\"<command>\"] to .sakura/flow.toml")
GATE_RED_EVENT = "gate_red"


def check_protected(ctx: FlowContext, label: str, start: str, approved_head: str,
                    exempt: dict[str, str] | None = None) -> None:
    """Raise MergeBlocked when `start..approved_head` changes a protected path without a granted ruling.

    Protected: `state.protected_paths`, `.sakura/**`, and any file matching `brakes.test_patterns`
    that existed at the base commit and was changed or deleted. New test files are allowed. `exempt`: paths added
    with exactly these blobs (the contract flows the engine committed, `final_head.flow_blobs`). Both commits are
    read from the engine's store (sealed there first), never from a workspace or a ref a role writes.
    """
    state = ctx.store.read_state()
    store = final_head.sealed_candidate(ctx, start, approved_head)
    zero = Path((state.get("workspaces", {}).get("0") or {}).get("path") or ctx.repo)
    protected = {_relative(zero, entry) for entry in state.get("protected_paths", [])}
    patterns = ctx.config.values["brakes"]["test_patterns"]
    offending = sorted({
        path for status, path in _changes(store, start, approved_head)
        if _is_protected(store, state["base_commit"], protected, patterns, status, path)
        and not _is_exempt(store, approved_head, exempt or {}, status, path)
        and not _covered_by_ruling(ctx, store, path, start, approved_head)})
    if offending:
        raise MergeBlocked(f"{label} changes protected paths without a ruling: {', '.join(offending)}")


def run_full_tests(ctx: FlowContext, checkout: completion.Checkout | None = None) -> PhaseOutcome | None:
    """Run `tests.full` on the engine's head (`flow/final_head.py`); returns None when green (and records the tested
    head), else the blocking outcome.

    The commands run from the completion checkout of that head (`flow/completion.py`, `checkout` when the caller holds
    one, an evaluation fix's candidate), caged on read-only source with their caches and build output in the
    checkout's build folder, so a test can neither swap a tracked file for the run nor lean on anything in a workspace.
    No branch or workspace is read: a commit a survivor adds to the task branch is not what is tested. The head is
    recorded only when the checkout still holds exactly that revision after the run.
    """
    ctx.store.update_state(lambda state: state.update(full_green_head=None, full_green_inputs_sha256=None))
    if not ctx.config.values["tests"]["full"]:
        ctx.store.append_ledger("build", NO_FULL_TESTS_LINE)
        try:
            return _record_green(ctx, final_head.proved(ctx)[1], FULL_TESTS_NOT_CONFIGURED)
        except GitError as error:
            return PhaseOutcome("blocked", f"full tests not green: {error}")
    try:
        for command in ctx.config.values["tests"]["full"]:
            repository_commands.require_approved(ctx.store, ctx.config, command)
    except repository_commands.CommandNotApproved as error:
        return PhaseOutcome("blocked", f"tests.full not run: {error}", gate=checks.APPROVAL_GATE)
    try:
        with contextlib.nullcontext(checkout) if checkout else completion.opened(ctx) as current:
            failure = _run_full_commands(ctx, current)
            if failure:
                return failure
            completion.require_unchanged(current)
    except (completion.CompletionRefused, workspaces.WorkspaceError, GitError) as error:
        return PhaseOutcome("blocked", f"full tests not green: {error}")
    return _record_green(ctx, current.revision, FULL_TESTS_PASSED)


def _record_green(ctx: FlowContext, head: str, outcome: str) -> None:
    """Record the tested head; `outcome` says whether a suite really ran (the report states it)."""
    input_hash = full_tests_inputs_hash(ctx)
    ctx.store.update_state(lambda state: state.update(full_green_head=head, full_green_inputs_sha256=input_hash,
                                                      full_tests=outcome))
    return None


def _run_full_commands(ctx: FlowContext, checkout: completion.Checkout) -> PhaseOutcome | None:
    ctx.store.operations_dir.mkdir(parents=True, exist_ok=True)
    log = ctx.store.operations_dir / FULL_LOG
    log.unlink(missing_ok=True)
    reports = []
    for index, command in enumerate(ctx.config.values["tests"]["full"], start=1):
        try:
            outcome, output, queue_timed_out = _run_command(ctx, checkout, command, index, log)
        except (repository_commands.CommandNotApproved, cage.CageError) as error:
            return PhaseOutcome("blocked", f"tests.full not run: {error}", gate=checks.APPROVAL_GATE)
        reports.append(skips.parse(output))
        failure = (PhaseOutcome("blocked", "stalled", event=ESCALATION_EVENT)
                   if queue_timed_out else _failure_of(ctx, outcome, command, log))
        if failure:
            _report_skips(skips.merge(reports), log)
            return failure
    _report_skips(skips.merge(reports), log)
    return None


def full_tests_inputs_hash(ctx: FlowContext) -> str:
    """Bind full-test publication to the commands, time limits and effective slot capacity."""
    values = ctx.config.values
    load, timeouts = values["load"], values["timeouts"]
    inputs = {"commands": values["tests"]["full"], "build_timeout": timeouts["build"],
              "idle_timeout": timeouts["idle_seconds"],
              "slots": slots.slot_count(os.environ, os.cpu_count() or 1, load["slots"])}
    return hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()


def _changes(store: Path, start: str, approved_head: str) -> list[tuple[str, str]]:
    """Changed paths as (status, path); a rename counts as a deletion plus an addition."""
    names = git(store, "diff", "--name-status", "--no-renames", "-z", start, approved_head).split("\0")
    return [(status, path) for status, path in zip(names[0::2], names[1::2]) if path]


def _is_protected(store: Path, base: str, protected: set[str], patterns: list[str], status: str, path: str) -> bool:
    if path in protected or globs.matches_any(path, PROTECTED_GLOBS):
        return True
    return status != ADDED and globs.matches_any(path, patterns) and _existed(store, base, path)


def _is_exempt(store: Path, head: str, exempt: dict[str, str], status: str, path: str) -> bool:
    return status == ADDED and path in exempt and git(store, "rev-parse", f"{head}:{path}").strip() == exempt[path]


def _covered_by_ruling(ctx: FlowContext, store: Path, path: str, start: str, approved_head: str) -> bool:
    if ctx.store.granted(path, "test_change"):
        return True
    if not ctx.store.granted(path, "timeout"):
        return False
    return inflation.timeout_only_increase(_content_at(store, start, path), _content_at(store, approved_head, path))


def _content_at(store: Path, revision: str, path: str) -> str:
    try:
        return git(store, "show", f"{revision}:{path}")
    except GitError:
        return ""


def _existed(store: Path, base: str, path: str) -> bool:
    try:
        git(store, "cat-file", "-e", f"{base}:{path}")
    except GitError:
        return False
    return True


def _relative(zero: Path, entry: str) -> str:
    try:
        return Path(entry).resolve().relative_to(zero.resolve()).as_posix() if Path(entry).is_absolute() else entry
    except ValueError:
        return entry


def _run_command(ctx: FlowContext, checkout: completion.Checkout, command: str, index: int,
                 log: Path) -> tuple[process.ProcessOutcome, str, bool]:
    raw = ctx.store.operations_dir / f"full-tests-{index}.raw"
    raw.unlink(missing_ok=True)
    timeouts = ctx.config.values["timeouts"]
    launch = repository_commands.plan(ctx.store, ctx.config, command, checkout.source, "tests-full",
                                      build_dir=checkout.build)
    wait_started = time.monotonic()
    queue_timed_out = False
    try:
        with process.heavy_slot(ctx.config, f"tests.full {index}", timeout_seconds=timeouts["build"],
                                stop_file=ctx.store.stop_file):
            scratch = repository_commands.scratch_log(launch.env, raw)
            try:
                outcome = process.tracked_run(
                    ctx,
                    launch.argv, cwd=checkout.source, env=launch.env, log_path=scratch.path, log_file=scratch.file,
                    budget_seconds=timeouts["build"], idle_seconds=timeouts["idle_seconds"],
                    stop_file=ctx.store.stop_file)
            finally:
                repository_commands.keep_log(scratch, raw)
    except slots.SlotTimeout:
        queue_timed_out = True
        waited = time.monotonic() - wait_started
        output = (f"stalled waiting for a heavy-work slot: tests.full {index}; "
                  f"waited {waited:.3f}s (wait budget: {timeouts['build']}s); "
                  f"command: {command}; log: {log}\n")
        raw.write_text(output)
        outcome = process.ProcessOutcome(None, True, False, False, 0)
    except slots.SlotStopped:
        output = "stopped waiting for a heavy-work slot\n"
        raw.write_text(output)
        outcome = process.ProcessOutcome(None, False, False, True, 0)
    output = raw.read_text(errors="replace")
    print(output, end="", flush=True)
    with log.open("a") as stream:
        stream.write(f"$ {command}\n{output}\nexit {outcome.exit_code}\n")
    if queue_timed_out:
        ctx.store.append_ledger("build", output.strip())
    return outcome, output, queue_timed_out


def _failure_of(ctx: FlowContext, outcome: process.ProcessOutcome, command: str, log: Path) -> PhaseOutcome | None:
    if outcome.stopped:
        return PhaseOutcome("stopped", "stopped during the full tests")
    left_running = getattr(outcome, "left_running", False)
    if outcome.stalled or outcome.timed_out or left_running:
        label = "timed out" if outcome.timed_out else process.LEFT_RUNNING if left_running else "stalled"
        return PhaseOutcome("blocked", f"{label}: full tests ({command}); see {log}", event=ESCALATION_EVENT)
    if outcome.exit_code != 0:
        reason = f"full tests failed (exit {outcome.exit_code}): {command}; see {log}\n{tail(log)}"
        return PhaseOutcome("blocked", reason, event=GATE_RED_EVENT)
    return None


def _report_skips(report: skips.SkipReport, log: Path) -> None:
    lines = [f"skipped: {entry.path}: {entry.reason}" for entry in report.entries]
    lines.append(f"sakura test full: {report.count} skipped")
    print("\n".join(lines), flush=True)
    with log.open("a") as stream:
        stream.write("\n".join(lines) + "\n")
