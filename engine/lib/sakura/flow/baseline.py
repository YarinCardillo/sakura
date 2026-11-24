"""Run contract flows against the base build and record what each one did."""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Callable

from sakura import slots
from sakura.flow import cage, checks, loopback, process, readonly_checkout, repository_commands, stacks
from sakura.flow.contract import ContractError, Row
from sakura.flow.engine import FlowContext

EVIDENCE_DIR = "contract-base"
EXIT_LINE = re.compile(r"(?i)exit[^0-9\n]*(\d+)")
API_LABEL = "api"


class FlowInterrupted(RuntimeError):
    """Raised when a flow was stopped, stalled or timed out: it has no result."""

    def __init__(self, row_id: str, reason: str) -> None:
        super().__init__(f"flow of row {row_id} {reason}")
        self.row_id = row_id
        self.stopped = reason == "was stopped"


class LocalSocketsRefused(FlowInterrupted):
    """Check rows failed on a loopback socket the cage refused; the message carries the command that lifts it."""

    def __init__(self, row_ids: list[str], message: str) -> None:
        RuntimeError.__init__(self, message)
        self.row_id = row_ids[0]
        self.stopped = False


def run_rows(ctx: FlowContext, workspace_path: Path, base_url: str, port: int, rows: list[Row],
             evidence_dir: str = EVIDENCE_DIR, stack_id: str = "",
             keepalive: Callable[[], None] | None = None, build_dir: Path | None = None) -> dict[str, int]:
    """Run every row's flow in the workspace; returns each row's exit code.

    Each run is logged to `evidence/<evidence_dir>/<row>-<viewport|api>.log` (`contract-base` on the base build), the
    first line being `exit <code>`. `stack_id` is the app stack kept active meanwhile (workspace 0's by default).
    A `keepalive` marks a call from a child of the engine (the evaluator's tool): it keeps the stack active itself
    and records no process in the task state, which that child may not change. Raises FlowInterrupted when a flow
    was stopped, stalled or timed out, since that is not a result. With `build_dir` (the engine's completion
    checkout of the revision a receipt will name, `flow/completion.py`) the rows cannot write the source or the Git
    folder; their caches, build output and tmp go to that folder.
    """
    out_dir = ctx.store.evidence_dir / evidence_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    listed = ctx.config.values["checks"]
    checks.require_approved(ctx.store.read_state(), rows, ctx.steps, listed["approved"], listed["remembered"])
    try:
        policy = cage.load_policy(ctx.config.values["knobs"]["cage"])
        run_cage = checks.checks_cage(ctx.store, workspace_path, f"checks-{evidence_dir.replace('/', '-')}", policy,
                                      checks.loopback_ports(ctx.config, port), build_dir, listed["local_sockets"])
    except cage.CageError as error:
        raise ContractError(f"cannot cage the contract commands: {error}") from error
    codes = {row.id: _run_one(ctx, workspace_path, base_url, port, row, out_dir, run_cage, listed,
                              stack_id or f"{ctx.store.task_id}:0", keepalive, build_dir is not None) for row in rows}
    if not (listed["local_sockets"] or policy.local_network):
        _require_no_refused_sockets(ctx, rows, codes, out_dir)
    return codes


def _require_no_refused_sockets(ctx: FlowContext, rows: list[Row], codes: dict[str, int], out_dir: Path) -> None:
    """Raise LocalSocketsRefused when a failed row's log shows the cage refusing its own loopback socket."""
    refused = [row.id for row in rows if codes[row.id] != 0
               and loopback.is_socket_denial(_inside(out_dir, f"{row.id}-{row.viewport or API_LABEL}.log").read_text(errors="replace"))]
    if refused:
        raise LocalSocketsRefused(refused, loopback.approval_message(ctx.store.task_id, refused))


def wrong_on_base(rows: list[Row], codes: dict[str, int]) -> list[str]:
    """Describe every row whose flow result contradicts its kind (new must fail, baseline must pass)."""
    problems = []
    for row in rows:
        if row.baseline and codes[row.id] != 0:
            problems.append(f"{row.id} (baseline) fails on the base build but must pass")
        if not row.baseline and codes[row.id] == 0:
            problems.append(f"{row.id} passes on the base build but must fail")
    return problems


def _run_one(ctx: FlowContext, cwd: Path, base_url: str, port: int, row: Row, out_dir: Path,
             run_cage: cage.RunCage, listed: dict, stack_id: str, keepalive: Callable[[], None] | None,
             is_read_only: bool) -> int:
    """Run one row's command in the checks cage: a listed command as its words, an approved one through `sh -c`.

    A failed run on a read-only checkout whose output shows a refused write gets a closing line that says so.
    """
    raw = _inside(ctx.store.operations_dir, f"contract-flow-{row.id}.raw")
    log = _inside(out_dir, f"{row.id}-{row.viewport or API_LABEL}.log")
    raw.unlink(missing_ok=True)
    env = {**process.scrubbed_environment(os.environ), **run_cage.env, "SAKURA_FLOW_PORT": str(port),
           "SAKURA_FLOW_BASE_URL": base_url, "SAKURA_FLOW_VIEWPORT": row.viewport or ""}
    try:
        argv = run_cage.wrap(checks.argv(row.flow, listed["approved"], listed["remembered"]))
        _require_the_apps_listener(ctx, stack_id, row)
        outcome = _supervised(ctx, argv, cwd, env, raw, row, stack_id, keepalive)
        _require_the_apps_listener(ctx, stack_id, row)
    except slots.SlotTimeout as error:
        raise FlowInterrupted(row.id, "stalled waiting for a heavy-work slot") from error
    except slots.SlotStopped as error:
        raise FlowInterrupted(row.id, "was stopped") from error
    output = raw.read_text(errors="replace")
    if outcome.exit_code is None:
        log.write_text(f"interrupted\n$ {row.flow}\n{output}")
        raise FlowInterrupted(row.id, _interruption(outcome))
    refused_write = is_read_only and outcome.exit_code != 0 and not loopback.is_socket_denial(output)
    hint = readonly_checkout.failure_hint(output) if refused_write else ""
    closing = f"\n{hint}\n" if hint else ""
    log.write_text(f"exit {outcome.exit_code}\n$ {row.flow}\n{output}{closing}")
    return outcome.exit_code


def _require_the_apps_listener(ctx: FlowContext, stack_id: str, row: Row) -> None:
    """Raise FlowInterrupted (the row has no result, so no receipt) unless every listener on the registered app's port
    is a process of the app's own group (review 15, finding 2); nothing to check without a registered app."""
    entry = (ctx.store.read_state().get("stacks") or {}).get(stack_id)
    if not entry:
        return
    problem = process.listener_problem(entry["pid"], entry["port"])
    if problem:
        raise FlowInterrupted(row.id, f"is not proved: {problem}; no receipt")


def _supervised(ctx: FlowContext, argv: list[str], cwd: Path, env: dict, raw: Path, row: Row, stack_id: str,
                keepalive: Callable[[], None] | None) -> process.ProcessOutcome:
    """Run the command under the heavy-work governor while the app stack stays active (see `run_rows`)."""
    timeouts = ctx.config.values["timeouts"]
    budget = {"budget_seconds": timeouts["contract"], "idle_seconds": timeouts["idle_seconds"],
              "stop_file": ctx.store.stop_file}
    if keepalive:
        keepalive()
    with process.heavy_slot(ctx.config, f"contract flow {row.id}", timeout_seconds=timeouts["contract"],
                            stop_file=ctx.store.stop_file):
        scratch = repository_commands.scratch_log(env, raw)
        try:
            if keepalive:
                return process.external_tracked_run(ctx.store, argv, cwd=cwd, env=env, log_path=scratch.path,
                                                    on_poll=keepalive, log_file=scratch.file, **budget)
            stacks.touch(ctx, stack_id)
            return process.tracked_run(ctx, argv, cwd=cwd, env=env, log_path=scratch.path, log_file=scratch.file,
                                       on_poll=lambda: (stacks.touch_if_due(ctx, stack_id), stacks.sweep(ctx)), **budget)
        finally:
            repository_commands.keep_log(scratch, raw)


def _interruption(outcome: process.ProcessOutcome) -> str:
    if outcome.stopped:
        return "was stopped"
    if outcome.left_running:
        return process.LEFT_RUNNING
    return "stalled (no output)" if outcome.stalled else "timed out"


def _inside(directory: Path, name: str) -> Path:
    """Return directory/name, refusing names that would leave the directory."""
    path = directory / name
    if path.resolve().parent != directory.resolve():
        raise ContractError(f"path {name!r} escapes {directory}")
    return path
