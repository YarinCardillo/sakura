"""Command line of `sakura flow`."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from sakura import actors, engine_home, interpreter, programs, repository
from sakura.cockpit import decided
from sakura.budget import settings as budget_settings
from sakura.flow import agent_feed, agent_identity, agent_resume, agent_steer, baseline, brakes, budget_guard, checks, config, contract, conversations, engine, exits, flags, gates, inbox, local_report, migrate, owner_marks, pace, preset_cli, process, registry, render, resume, run_records, store, surface, tasks, wake, waits
from sakura.flow import changes, delivery, feed, final_head, gitutil, lane_ceiling, lane_questions, run_view, task_repository, tmux
from sakura.flow.tmux import TmuxClient, TmuxError
from sakura import loadinfo
from sakura.tracks import starts
from sakura.tracks.domain import Refusal as TrackRefusal

USAGE_FAILURES = (config.FlowConfigError, repository.RepositoryError, tasks.TaskStartError, flags.FlagError,
                  actors.ActorError, surface.SurfaceUnavailable, conversations.ReopenRefused,
                  engine_home.EngineHomeError, owner_marks.MarkRefused, contract.ContractError,
                  lane_ceiling.LaneCeilingRefused, TrackRefusal)
ENGINE_ROLE_REFUSAL = brakes.ENGINE_ROLE_REFUSAL
REVISION_HELP = ("answer only the revision the person was shown (the part after @ in a wait's key); a task that moved "
                 "on is refused with `no longer waiting` and nothing changes")
SANDBOX_REFUSAL = "operator commands cannot run inside an agent cage: the engine home is not writable here"
# A lane with no engine driving it that `flow stop` lists as stopped at once (a `running` one with the lock free has a dead
# driver behind it); a driven lane keeps its status and gets the stop file.
IDLE_STOPPABLE = ("blocked", "failed", "awaiting_approval", "running")
WAITING_STATUSES = ("awaiting_approval", "blocked", "failed", "stopped")
QUEUED_NOTICE = "applies at the next safe boundary"
WAKE_ACTOR = "cli:budget-wake"
EVENTS_WAIT_SECONDS = 60
EVENTS_POLL_SECONDS = 0.2


def main(argv: list[str]) -> int:
    """Run an `sakura flow` command and return its exit code."""
    args = _parser().parse_args(argv)
    try:
        _remember_repository(args)
        return COMMANDS[args.command](args)
    except USAGE_FAILURES as error:
        print(error)
        return exits.USAGE
    except (store.EngineBusy, surface.DriverRunning) as busy:
        print(busy)
        return exits.BUSY
    except TmuxError as error:
        print(_without_controls(str(error)))
        return exits.FAILED
    except (store.StateError, registry.RegistryError) as error:
        print(error)
        return exits.FAILED


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura flow")
    sub = parser.add_subparsers(dest="command", required=True)
    start = sub.add_parser("start")
    start.add_argument("ref", nargs="?", help="the task: a file in the repository (or an issue or page, per spec.source), "
                                              "or the task text itself")
    start.add_argument("-m", "--message", help="the task text, when it is not in a file")
    start.add_argument("--new", action="store_true")
    start.add_argument("--inbox-op", default="", help=argparse.SUPPRESS)
    start.add_argument("--inbox-context", default="", help=argparse.SUPPRESS)
    start.add_argument("--preset")
    start.add_argument("--dial")
    start.add_argument("--familiarity")
    start.add_argument("--role", action="append", default=[])
    start.add_argument("--set", action="append", default=[], dest="settings")
    start.add_argument("--relax", action="append", default=[])
    start.add_argument("--after")
    start.add_argument("--when-cheap", action="store_true", help="not urgent: wait for the night when the week's pace is high")
    approve = sub.add_parser("approve")
    approve.add_argument("task")
    approve.add_argument("gate", type=_gate_name, metavar=f"{engine.SPEC_GATE}|{checks.APPROVAL_GATE}|{engine.BUDGET_GATE}|{engine.BUDGET_WAIT_GATE}|{engine.OPUS_GATE_PREFIX}<build task>")
    approve.add_argument("--continue", action="store_true", dest="continue_after")
    approve.add_argument("--remember", action="store_true",
                         help="with the checks gate: also run these exact commands without asking in later contracts")
    approve.add_argument("--local-sockets", action="store_true",
                         help="with the checks gate: let this project's commands open their own loopback sockets "
                              "(the system's high loopback ports open to them); remembered for the project")
    approve.add_argument("--revision", default=None, help=REVISION_HELP)
    resume = sub.add_parser("resume")
    resume.add_argument("task")
    resume.add_argument("--note", default="", help="operator guidance for the next run")
    resume.add_argument("--stopped-revision", default=None, help="resume only this stopped revision")
    resume.add_argument("--startup-ack", default=None, help=argparse.SUPPRESS)
    resume.add_argument("--continues-start", action="store_true", help=argparse.SUPPRESS)
    for driver in (start, approve, resume):
        driver.add_argument("--detach", action="store_true")
    recontract = sub.add_parser("recontract")
    recontract.add_argument("task")
    stop = sub.add_parser("stop")
    stop.add_argument("task", nargs="?")
    stop.add_argument("--all", action="store_true", dest="stop_all")
    stop.add_argument("--revision", help="stop only this lane state revision")
    steer = sub.add_parser("steer")
    steer.add_argument("task")
    steer.add_argument("text")
    steer.add_argument("--run", default="", help="write to one agent (a run of the lane) instead: live to a running Claude "
                       "run, held for a running one that takes no live input")
    steer.add_argument("--json", action="store_true", help="with --run: the outcome as one line of JSON")
    worker = sub.add_parser("agent-worker", help=argparse.SUPPRESS)
    worker.add_argument("task")
    worker.add_argument("run")
    worker.add_argument("--begun", action="store_true", help=argparse.SUPPRESS)
    dial = sub.add_parser("dial")
    dial.add_argument("task")
    dial.add_argument("step")
    speed = sub.add_parser("pace", help="a lane's own pace: slowest, slow, normal, fast or fastest")
    speed.add_argument("task")
    speed.add_argument("level")
    switch = sub.add_parser("preset", help="switch a running lane to another preset at its next step")
    switch.add_argument("task")
    switch.add_argument("name")
    step = sub.add_parser("step", help="turn one optional step on or off for one lane, from its next step")
    step.add_argument("task")
    step.add_argument("step_name")
    step.add_argument("state")
    role = sub.add_parser("role")
    role.add_argument("task")
    role.add_argument("role_name")
    role.add_argument("value")
    status = sub.add_parser("status")
    status.add_argument("task", nargs="?")
    status.add_argument("--json", action="store_true")
    sessions = sub.add_parser("sessions")
    sessions.add_argument("task")
    sessions.add_argument("--json", action="store_true")
    feed_parser = sub.add_parser("feed", help="what a lane's roles wrote, the engine's notices and the board posts, in time order, as JSON")
    feed_parser.add_argument("task")
    feed_parser.add_argument("--run", default="", help="one agent's conversation instead: that run, with the runs it continues "
                             "or is continued by, its tools with their times, the text it is writing now and the messages sent to it")
    feed_parser.add_argument("--since", default="", help="the cursor of the previous page; none starts at the beginning")
    feed_parser.add_argument("--json", action="store_true", required=True, help="the only format")
    changes_parser = sub.add_parser("changes", help="the files a lane changed with their line counts, or one file's patch, as JSON")
    changes_parser.add_argument("task")
    changes_parser.add_argument("--file", help="print the patch of this changed file instead of the list")
    changes_parser.add_argument("--json", action="store_true", required=True, help="the only format")
    watch = sub.add_parser("watch")
    watch.add_argument("task")
    watch.add_argument("run")
    events = sub.add_parser("events")
    events.add_argument("task")
    events.add_argument("--wait", action="store_true")
    sub.add_parser("attach").add_argument("task")
    reopen = sub.add_parser("open")
    reopen.add_argument("task")
    reopen.add_argument("run")
    reopen.add_argument("--message", help="send this message to a Goose lane headlessly")
    reopen.add_argument("--fresh", action="store_true", help="start a new Goose conversation on the same route")
    sub.add_parser("report").add_argument("task")
    cleanup = sub.add_parser("cleanup")
    cleanup.add_argument("task")
    cleanup.add_argument("--revision", help="discard only this lane state revision")
    cleanup.add_argument("--discard", action="store_true",
                         help="remove a failed or stopped lane's workspaces, branches and repository without a merged pull request")
    run_row = sub.add_parser("run-row", help="the engine's own run of an evaluator row, called by its run_row tool")
    run_row.add_argument("row_id")
    run_row.add_argument("--viewport")
    sub.add_parser("mode").add_argument("mode", nargs="?", choices=config.GATE_MODES)
    sub.add_parser("register").add_argument("path")
    sub.add_parser("wake")
    sub.add_parser("migrate-state").add_argument("--repo", default=".")
    restore = sub.add_parser("restore-head", help="put workspace 0 back on the engine's head, so an owner row can be marked")
    restore.add_argument("task")
    restore.add_argument("--expect-workspace", default=None, help="restore only while workspace 0 is at this head")
    restore.add_argument("--expect-head", default=None, help="restore only to this sealed engine head")
    restore.add_argument("--row", default=None, help="the owner row the restore was offered on")
    restore.add_argument("--revision", default=None, help=REVISION_HELP)
    mark = sub.add_parser("mark", help="the person's verdict on an owner contract row")
    mark.add_argument("task")
    mark.add_argument("row")
    mark.add_argument("verdict", choices=tuple(owner_marks.VERDICTS))
    mark.add_argument("--note", default="")
    mark.add_argument("--revision", default=None, help=REVISION_HELP)
    mark.add_argument("--continue", action="store_true", dest="continue_after",
                      help="resume the task when this was its last open owner row")
    mark.add_argument("--detach", action="store_true")
    reply = sub.add_parser("reply", help="the person's answer to a question a role asked")
    reply.add_argument("task")
    reply.add_argument("question")
    reply.add_argument("digest")
    reply.add_argument("--choice", default="")
    reply.add_argument("--text", default="")
    for writer in (start, approve, resume, recontract, stop, steer, dial, speed, switch, step, role, cleanup, mark, restore, reply):
        writer.add_argument("--actor")
    return parser


def _remember_repository(args: argparse.Namespace) -> None:
    """Record the repository of the working directory for the cockpit; a failure warns and never stops the command.

    Engine role sessions do not register: their working directories are task workspaces.
    """
    if args.command in ("register", "wake") or os.environ.get(brakes.ROLE_ENV):
        return
    try:
        registry.remember(_repository())
    except repository.RepositoryError:
        return
    except registry.RegistryError as error:
        print(f"cannot remember repository: {error}", file=sys.stderr)


def _actor(args: argparse.Namespace) -> str:
    """Return the validated actor of this command as text (flag, else SAKURA_ACTOR, else cli:<user>)."""
    return actors.format(actors.resolve(args.actor))


def _require_operator() -> None:
    """Refuse operator commands issued from inside an engine role: by its variable, and by the sandbox itself.

    The variable is the agent's to unset; the cage is not, so a command that cannot write the engine's trust
    root is refused whatever its environment says.
    """
    if os.environ.get(brakes.ROLE_ENV):
        raise tasks.TaskStartError(ENGINE_ROLE_REFUSAL)
    if engine_home.sandboxed():
        raise tasks.TaskStartError(SANDBOX_REFUSAL)


def _ledger_request(task_store: store.TaskStore, text: str, actor: str) -> None:
    task_store.append_ledger("operator", f"{text} actor={actor}")


def _proposal_context(args: argparse.Namespace) -> dict:
    if not args.inbox_context:
        return {}
    try:
        data = json.loads(args.inbox_context)
    except (ValueError, RecursionError):
        raise tasks.TaskStartError("invalid Inbox source context") from None
    fields = {"source", "author", "title", "detail"}
    if (not args.inbox_op or not isinstance(data, dict) or set(data) != fields
            or any(not isinstance(value, str) or len(value) > 8000 for value in data.values())):
        raise tasks.TaskStartError("invalid Inbox source context")
    return data


def _start(args: argparse.Namespace) -> int:
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    if tasks.TRACK_START_ID.fullmatch(args.inbox_op):
        return _start_reserved_track(args, repo)
    return _start_lane(args, actor, repo)


def _start_reserved_track(args: argparse.Namespace, repo: Path) -> int:
    dispatch = starts.dispatch_options(args.inbox_op, args.actor)
    parser = _parser()
    if args != parser.parse_args(["start", *dispatch]):
        raise tasks.TaskStartError("Track starts use the saved reservation inputs")

    def run(saved: starts.Reservation) -> tuple[int, str]:
        frozen = parser.parse_args(["start", *starts.dispatch_options(saved.task, saved.actor), *saved.inputs.options()])
        created = []
        code = _start_lane(frozen, saved.actor, repo, track_start=True, emit_created=created.append)
        return code, created[0] if len(created) == 1 else ""

    code, made = starts.TrackStarts().execute(engine_home.project_id(repo), args.inbox_op, actors.resolve(args.actor), run)
    if made:
        _print_created(made)
    return code


def _print_created(task_id: str) -> None:
    print(f"{tasks.CREATED_LANE_PREFIX}{task_id}")


def _start_lane(args: argparse.Namespace, actor: str, repo: Path, *, track_start: bool = False,
                emit_created: Callable[[str], None] = _print_created) -> int:
    overrides = flags.build_overrides(args.preset, args.dial, args.familiarity, args.role, args.settings, args.relax)
    cfg = config.load(repo, overrides=overrides or None)
    overrides = _with_lane_pace(overrides, cfg)
    tasks.check_codex_trust(cfg, repo)
    tasks.check_git_config(repo)
    known = tasks.known_tasks(repo)
    given = tasks.resolve_input(repo, cfg, args.ref, args.message)
    print(given.description)
    if notice := engine_home.trust_records_notice():
        print(notice)
    if args.inbox_op and (not args.new or not args.detach or not (track_start or args.inbox_op.startswith("proposal-"))
                          or not store.TASK_ID.fullmatch(args.inbox_op)):
        raise tasks.TaskStartError("invalid Inbox lane reservation")
    proposal_context = _proposal_context(args)
    taken = {name: "" for name in known} if args.new else known
    if args.detach:
        _tmux_surface(cfg)  # refused before the task is created, as every detached command is
    if not args.new and given.ref in known.values():
        task_id = store.task_id_for(given.ref, known)
        print(f"resuming {task_id}")
        task_store = store.TaskStore(repo, task_id)
        return _detach_resume(cfg, task_store, actor) if args.detach else _resume_existing(repo, task_store, actor)
    task_id = args.inbox_op or store.task_id_for(given.ref, taken, given.label)
    task_store = store.TaskStore(repo, task_id)
    title = tasks.split_title(given.text)[0] if given.text is not None else None
    state = tasks.initial_state(repo, cfg, task_id, given.ref, overrides, urgent=not args.when_cheap,
                                track_title=title if track_start else None)
    marker = tasks.TRACK_START_KEY if track_start else tasks.INBOX_START_KEY
    if args.inbox_op:
        state[marker] = args.inbox_op
    if given.text is not None:
        state["input_text"] = True
        state[tasks.TITLE_KEY] = title
    if proposal_context:
        state[tasks.PROPOSAL_CONTEXT_KEY] = proposal_context
        if given.text == "Review the selected Inbox proposal.":
            state[tasks.TITLE_KEY] = tasks.split_title(proposal_context["title"])[0]
    input_data = given.input_bytes()
    if input_data is not None:
        state[tasks.INPUT_DIGEST_KEY] = hashlib.sha256(input_data).hexdigest()
    if args.after:
        from sakura.flow.phases import intake

        state["base_branch"], state["base_commit"] = intake.stacked_base(repo, args.after)
    is_created = False
    with contextlib.ExitStack() as admitted:
        if not task_store.exists():
            admitted.enter_context(lane_ceiling.admit(actors.parse(actor), cfg.values["knobs"]["lanes"]))
        with task_store.lock():
            if task_store.exists() and args.inbox_op:
                retry = task_store.read_state()
                if retry.get(marker) != args.inbox_op:
                    source = "Track" if track_start else "Inbox"
                    raise tasks.TaskStartError(f"{source} lane reservation is already in use")
                emit_created(task_id)
                if not retry.get(tasks.PENDING_START_KEY):
                    return exits.OK
                if track_start:
                    _ensure_track_opener(task_store, actor)
                _make_task_repository(task_store, cfg, repo)
                is_created = True
            elif task_store.exists():
                _require_same_task(task_store, given.ref, args.new, task_id)
                print(f"resuming {task_id}")
                if not args.detach:
                    _ledger_request(task_store, "resume requested", actor)
                    return _advance(repo, _clear_stop(task_store, actor), keep_awake=True)
            else:
                if args.detach:
                    state[tasks.PENDING_START_KEY] = True
                with starts.admit_task_creation(task_store, track_start):
                    task_store.create(state, input_data)
                is_created = True
                emit_created(task_id)
                admitted.close()  # the lane counts now: the next open need not wait for the rest of this start
                if track_start:
                    _ensure_track_opener(task_store, actor)
                _make_task_repository(task_store, cfg, repo)
                if not track_start:
                    _ledger_request(task_store, "start requested", actor)
                task_store.append_ledger("engine", preset_cli.summary(cfg.values))
                if not args.detach:
                    return _advance(repo, task_store)
    # The task file was read once, above, and is stored: the window resumes the task, it never reads the file again.
    # Resuming a task this very start created is its continuation, not a resume the person asked for.
    return _detach_resume(cfg, task_store, actor, continues_start=is_created)


def _ensure_track_opener(task_store: store.TaskStore, actor: str) -> None:
    opener = lane_ceiling.opener(task_store)
    if opener is None:
        _ledger_request(task_store, "start requested", actor)
    elif opener != actors.parse(actor):
        raise tasks.TaskStartError("The reservation's opener does not match the saved lane")


def _make_task_repository(task_store: store.TaskStore, cfg: config.FlowConfig, repo: Path) -> None:
    """Make the task's own repository now, outside every cage (`flow/task_repository.py`); a failure is said here and
    the task, already created, makes it again when it resumes."""
    try:
        task_repository.ensure(task_store, cfg.values, repo)
    except (task_repository.TaskRepositoryError, gitutil.GitError) as error:
        raise tasks.TaskStartError(f"task {task_store.task_id} was created but its repository was not: {error}; run "
                                   f"sakura flow resume {task_store.task_id} once that is fixed") from error


def _require_same_task(task_store: store.TaskStore, ref: str, is_new: bool, task_id: str) -> None:
    """Raise unless the task another start created between our check and our lock is the one this start names."""
    if is_new:
        raise tasks.TaskStartError(f"task {task_id} was created concurrently; run the command again")
    stored_ref = task_store.read_state().get("ref")
    if stored_ref != ref:
        raise tasks.TaskStartError(
            f"task {task_id} was created concurrently for {_without_controls(str(stored_ref))}, "
            f"not {_without_controls(ref)}; use --new to start this spec as its own task")


def _resume(args: argparse.Namespace) -> int:
    _require_operator()
    actor = _actor(args)
    if args.note and (problem := resume.note_problem(args.note)):
        raise tasks.TaskStartError(f"invalid note: {problem}")
    repo = _repository()
    task_store = _existing(repo, args.task)
    args.note = args.note.strip()
    if args.continues_start:
        if args.note:
            raise tasks.TaskStartError("a continuation of a start carries no resume note")
        return _continue_start(repo, task_store, args)
    if args.stopped_revision is not None and args.detach:
        return _detach_stopped(task_store, actor, args)
    if args.stopped_revision is not None:
        return _resume_stopped(repo, task_store, actor, args)
    if args.detach:
        return _detach_resume(_task_config(repo, task_store), task_store, actor, note=args.note)
    return _resume_existing(repo, task_store, actor, args.note)


def _with_lane_pace(overrides: dict, cfg: config.FlowConfig) -> dict:
    """`overrides` with the pace the new lane starts at: the one chosen, else the engine's now, so only `flow pace` on the
    lane changes it afterwards."""
    knobs = cfg.values["knobs"]
    return {**overrides, "knobs": {**overrides.get("knobs", {}), "lane_pace": pace.lane_level(knobs)}}


def _continue_start(repo: Path, task_store: store.TaskStore, args: argparse.Namespace) -> int:
    """Run the first driver of a task a detached start created. The start recorded that it is pending; the run consumes
    it under the task lock, so a replayed, stale or invented continuation is refused and records nothing."""
    if args.stopped_revision is not None:
        raise tasks.TaskStartError("a continuation of a start carries no stopped revision")
    with task_store.lock():
        if not task_store.read_state().get(tasks.PENDING_START_KEY):
            raise tasks.TaskStartError(f"no start of {task_store.task_id} is waiting to continue; "
                                       f"run sakura flow resume {task_store.task_id} to drive it")
        return _advance(repo, task_store, keep_awake=True)


def _clear_pending_start(task_store: store.TaskStore) -> None:
    """Whichever driver runs first consumes the pending start: a later continuation of it is stale."""
    if task_store.read_state().get(tasks.PENDING_START_KEY):
        task_store.update_state(lambda state: state.pop(tasks.PENDING_START_KEY, None))


def _detach_resume(cfg: config.FlowConfig, task_store: store.TaskStore, actor: str,
                   continues_start: bool = False, *, note: str = "") -> int:
    """Run `sakura flow resume` of the task in its `engine` window; `continues_start` for the run a start of the task
    began, which the ledger does not call a resume."""
    marks = ["--continues-start"] if continues_start else []
    if note:
        marks.append(f"--note={note}")
    return _detach(cfg, task_store, ["resume", f"--actor={actor}", *marks, "--", task_store.task_id])


def _resume_stopped(repo: Path, task_store: store.TaskStore, actor: str, args: argparse.Namespace) -> int:
    with contextlib.ExitStack() as stack:
        try:
            stack.enter_context(task_store.lock())
            state = task_store.read_state()
            resume.require_stopped(state, args.stopped_revision, _owner_rows_marked(repo, task_store, state))
            from sakura.flow import phases
            ctx = engine.open_context(repo, task_store)
            tasks.check_codex_trust(ctx.config, repo)
            blocked = decided.find_wait(repo, [task_store], waits.BLOCKED, decided.ANY_SUBJECT)
        except Exception as error:
            resume.acknowledge(args.startup_ack, "refused")
            raise tasks.TaskStartError("cannot resume: driver startup refused") from error
        try:
            task_store.update_state(lambda current: resume.admit(current, args.stopped_revision, args.note, actor))
        except tasks.TaskStartError:
            resume.acknowledge(args.startup_ack, "refused")
            raise
        _finish_resume_request(task_store, actor, args.note, blocked)
        with process.hold_awake(os.getpid()):
            resume.acknowledge(args.startup_ack, "started")
            code = engine.advance(ctx)
        print(_summary(task_store.read_state(), str(task_store.report)))
        return code


def _owner_rows_marked(repo: Path, task_store: store.TaskStore, state: dict) -> bool:
    """Whether the task is blocked only on owner rows and the person has marked them all."""
    if not owner_marks.is_blocked_on_owner_rows(state):
        return False
    return owner_marks.all_marked(task_store, state, config.steps(_task_config(repo, task_store).values))


def _detach_stopped(task_store: store.TaskStore, actor: str, args: argparse.Namespace) -> int:
    with tempfile.TemporaryDirectory(prefix="sakura-resume-") as directory:
        receipt = Path(directory) / "startup"
        options = [f"--note={args.note}"] if args.note else []
        command = ["resume", f"--stopped-revision={args.stopped_revision}", f"--startup-ack={receipt}",
                   f"--actor={actor}", *options, "--", args.task]
        _detach(_task_config(task_store.repo, task_store), task_store, command)
        code = resume.wait_for_start(receipt)
        if code != exits.OK:
            print("resume startup refused" if code == exits.USAGE else "resume startup outcome unknown")
        return code


def _resume_existing(repo: Path, task_store: store.TaskStore, actor: str, note: str = "") -> int:
    if not note:
        if blocked := decided.find_wait(repo, [task_store], waits.BLOCKED, decided.ANY_SUBJECT):
            decided.record_now(blocked, "resume", actor)
        return _drive(repo, task_store, "resume requested", actor)
    with task_store.lock():
        blocked = decided.find_wait(repo, [task_store], waits.BLOCKED, decided.ANY_SUBJECT)
        inbox.enqueue(task_store, "steer", note, actor=actor)
        _finish_resume_request(task_store, actor, note, blocked)
        return _advance(repo, task_store, keep_awake=True)


def _finish_resume_request(task_store: store.TaskStore, actor: str, note: str, blocked: dict | None) -> None:
    request = f"resume requested actor={actor}"
    task_store.append_ledger("operator", f"{request} note={note}" if note else request)
    _clear_stop(task_store, actor)
    if blocked:
        decided.record_now(blocked, "resume", actor, text=note or None)


def _recontract(args: argparse.Namespace) -> int:
    _require_operator()
    actor = _actor(args)
    task_store = _existing(_repository(), args.task)
    with task_store.lock():
        old_digest, spec_digest = _validated_recontract_inputs(task_store, args.task)
        archive = task_store.root / "contract-history" / old_digest
        try:
            _archive_contract(task_store, archive, old_digest, spec_digest)
            contract.authorize_replan(task_store, _rewind_recontract)
        except (OSError, ValueError) as error:
            raise tasks.TaskStartError(f"could not authorize contract regeneration: {error}") from error
        shutil.rmtree(task_store.evidence_dir / baseline.EVIDENCE_DIR, ignore_errors=True)
        task_store.append_ledger("engine", f"contract regeneration authorized; archived {old_digest} actor={actor}")
    print(f"re-contract authorized for {args.task}; resume to regenerate from the approved spec")
    return exits.OK


def _validated_recontract_inputs(task_store: store.TaskStore, task: str) -> tuple[str, str]:
    state = task_store.read_state()
    if state["status"] not in ("blocked", "failed") or state["phase"] != "plan":
        raise tasks.TaskStartError(f"cannot recontract {task} outside a blocked or failed plan")
    if state.get("active_run") or state.get("active_runs"):
        raise tasks.TaskStartError(f"cannot recontract {task} while runs are active")
    spec_digest = state.get("approvals", {}).get("spec", {}).get("sha256", "")
    if spec_digest != hashlib.sha256(task_store.spec.read_bytes()).hexdigest():
        raise tasks.TaskStartError(f"cannot recontract {task} after the approved spec changed")
    digest = state.get("contract_sha256", "")
    if not digest:
        raise tasks.TaskStartError(f"cannot recontract {task} without a saved contract")
    try:
        contract.load(task_store, digest)
    except contract.ContractError as error:
        raise tasks.TaskStartError(f"cannot recontract {task}: {error}") from error
    return digest, spec_digest


def _rewind_recontract(state: dict) -> None:
    state["phase"] = "contract"
    state["status"] = "stopped"
    state["reason"] = "authorized contract regeneration; resume required"
    state["completed"]["contract"] = "recontract-authorized"
    state["completed"].pop("plan", None)
    state["plan_sha256"] = ""
    state["plan_review_findings"] = []
    state["rounds"] = {}
    waits.advance_candidate_generation(state)
    state["approvals"].pop(checks.APPROVAL_GATE, None)


def _archive_contract(task_store: store.TaskStore, archive: Path, digest: str, spec_digest: str) -> None:
    """Preserve the authenticated contract and its base evidence before re-planning; idempotent per digest."""
    if archive.exists():
        manifest = json.loads((archive / "archive-manifest.json").read_text())
        archived_digest = hashlib.sha256((archive / "contract.json").read_bytes()).hexdigest()
        if (manifest.get("contract_sha256") == digest and manifest.get("spec_sha256") == spec_digest
                and archived_digest == digest):
            return
        raise ValueError(f"archive for contract {digest} does not match current inputs")
    archive.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{archive.name}.pending-", dir=archive.parent))
    evidence = task_store.evidence_dir / baseline.EVIDENCE_DIR
    try:
        shutil.copy2(task_store.contract, temporary / "contract.json")
        if evidence.is_dir():
            shutil.copytree(evidence, temporary / "evidence" / baseline.EVIDENCE_DIR)
        manifest = {"contract_sha256": digest, "spec_sha256": spec_digest, "evidence_archived": evidence.is_dir()}
        (temporary / "archive-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        temporary.replace(archive)
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


def _clear_stop(task_store: store.TaskStore, actor: str) -> store.TaskStore:
    """Remove the operator's STOP file, if any, before the task is driven again."""
    if task_store.clear_stop():
        task_store.append_ledger("engine", f"resumed by operator actor={actor}")
    return task_store


def _require_lane_owner(task_store: store.TaskStore, actor: str) -> None:
    principal = actors.parse(actor)
    if principal.source == actors.MASTER_SOURCE and lane_ceiling.opener(task_store) != principal:
        raise tasks.TaskStartError(lane_ceiling.MASTER_OWNERSHIP_REFUSAL)


def _stop(args: argparse.Namespace) -> int:
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    if args.stop_all == bool(args.task):
        raise tasks.TaskStartError("give a task or --all")
    if args.stop_all and args.revision is not None:
        raise tasks.TaskStartError("--revision requires a single task")
    targets = list(tasks.known_tasks(repo)) if args.stop_all else [args.task]
    for task_id in targets:
        task_store = _existing(repo, task_id)
        if args.revision is None:
            _publish_stop(task_store, actor)
            try:
                with task_store.lock():
                    _stop_idle_blocked(task_store)
            except store.EngineBusy:
                pass
        else:
            with task_store.lock():
                _publish_stop(task_store, actor, args.revision)
                _stop_idle_blocked(task_store)
        print(f"stop requested for {task_id}")
    return exits.OK


def _publish_stop(task_store: store.TaskStore, actor: str, expected: str | None = None) -> None:
    _require_lane_owner(task_store, actor)
    blocked = decided.find_wait(task_store.repo, [task_store], waits.BLOCKED, decided.ANY_SUBJECT)
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")

    def require_state(state: dict) -> None:
        _require_lane_owner(task_store, actor)
        if expected is not None:
            _require_revision(expected, resume.revision(state), task_store.task_id)

    check = require_state if expected is not None or actors.parse(actor).source == actors.MASTER_SOURCE else None
    task_store.publish_stop(f"{at} operator\n", check)
    _ledger_request(task_store, "stop requested", actor)
    if blocked:
        decided.record_now(blocked, "stop", actor)


def _stop_idle_blocked(task_store: store.TaskStore) -> None:
    """Mark an idle lane stopped under its driver lock."""
    state = task_store.read_state()
    if state["status"] in IDLE_STOPPABLE:
        was = lane_ceiling.state_word(state["status"])
        reason = f"stopped by the operator; it was {was}" + (f": {state['reason']}" if state["reason"] else "")
        task_store.update_state(lambda current: current.update(status="stopped", reason=reason))


def _steer(args: argparse.Namespace) -> int:
    if args.run:
        return _steer_agent(args)
    return _enqueue(args, "steer", args.text.rstrip("\n"))


def _steer_agent(args: argparse.Namespace) -> int:
    """Write to one agent and print what came of it (`flow/agent_steer.py`); a refusal is an outcome, not an error."""
    _require_operator()
    actor, repo = _actor(args), _repository()
    task_store = _existing(repo, args.task)
    task_store.become_side_writer()  # the lane's engine may be running: this write must not look like tampering to it
    state = task_store.read_state()
    if state["status"] == engine.STATUS_CLEANED:
        raise tasks.TaskStartError(f"task {args.task} is cleaned and no longer takes steer commands")
    text = args.text.rstrip("\n")
    cfg = config.load(repo, overrides=state.get("overrides") or None)
    inbox.validate(cfg, "steer", text, "", repo)
    _require_lane_owner(task_store, actor)
    outcome = agent_steer.send(task_store, cfg, repository.common_dir(repo), args.run, text, actor)
    print(outcome.to_json() if args.json else _without_controls(outcome.words))
    return exits.OK


def _agent_worker(args: argparse.Namespace) -> int:
    """Supervise the run that continues an agent's conversation, or wait for an agent to end and continue it with what was
    held for it (`flow/agent_resume.py`). Started by `steer --run`, never by hand."""
    _require_operator()
    repo = _repository()
    task_store = _existing(repo, args.task)
    task_store.become_side_writer()  # it writes the task's state beside the lane's engine, through the same locked writer
    ctx = engine.open_context(repo, task_store)
    if args.begun:
        answered = agent_resume.run_begun(ctx, args.run)
        if not answered and task_store.read_state()["runs"][args.run]["provider"] == "goose":
            return exits.FAILED
    agent_resume.work(ctx, args.run)
    return exits.OK


def _dial(args: argparse.Namespace) -> int:
    return _enqueue(args, "dial", args.step)


def _pace(args: argparse.Namespace) -> int:
    return _enqueue(args, "pace", args.level)


def _preset(args: argparse.Namespace) -> int:
    return _enqueue(args, "preset", args.name)


def _step(args: argparse.Namespace) -> int:
    return _enqueue(args, "step", args.state, args.step_name)


def _role(args: argparse.Namespace) -> int:
    return _enqueue(args, "role", args.value, args.role_name)


def _enqueue(args: argparse.Namespace, kind: str, value: str, role: str = "") -> int:
    """Validate an operator command against the task's effective config and queue it; never takes the lock."""
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    task_store = _existing(repo, args.task)
    state = task_store.read_state()
    if state["status"] == engine.STATUS_CLEANED:
        raise tasks.TaskStartError(f"task {args.task} is cleaned and no longer takes {kind} commands")
    inbox.validate(config.load(repo, overrides=state.get("overrides") or None), kind, value, role, repo, state)
    _require_lane_owner(task_store, actor)
    queued = inbox.enqueue(task_store, kind, value, role, actor)
    if kind == "steer" and inbox.wait_live(task_store, inbox.command_id(queued)):
        role = inbox.live_role(task_store, inbox.command_id(queued))
        print(f"delivered{f' ({role})' if role else ''}: steer for {args.task} went to the running role")
        return exits.OK
    print(f"queued: {kind} for {args.task}; {QUEUED_NOTICE}")
    return exits.OK


def _drive(repo: Path, task_store: store.TaskStore, request: str = "", actor: str = "") -> int:
    """Take the task lock, then ledger an operator `request` and clear STOP: a busy engine keeps its stop."""
    with task_store.lock():
        if request:
            _ledger_request(task_store, request, actor)
            _clear_stop(task_store, actor)
        return _advance(repo, task_store, keep_awake=True)


def _advance(repo: Path, task_store: store.TaskStore, keep_awake: bool = False) -> int:
    """Drive the task; `keep_awake` holds a sleep assertion (resumed tasks, not the first spec draft)."""
    from sakura.flow import phases

    _clear_pending_start(task_store)
    ctx = engine.open_context(repo, task_store)
    tasks.check_codex_trust(ctx.config, repo)
    with process.hold_awake(os.getpid()) if keep_awake else contextlib.nullcontext():
        code = engine.advance(ctx)
    state = task_store.read_state()
    print(_summary(state, str(task_store.report)))
    return code


def _summary(state: dict, report: str = "") -> str:
    """The last line(s) a command prints: the stop reason, or for a finished task where its report is and where the
    work went (the pull request URL, or the line saying the branch was pushed and none was opened)."""
    if state["status"] == "report_ready" and report:
        delivered = state.get("pr", {}).get("url") or state.get("pr", {}).get("note", "")
        return "\n".join([f"report_ready: {report}", *([delivered] if delivered else [])])
    if state["status"] == "awaiting_approval" or not state["reason"]:
        return state["reason"] or state["status"]
    return f"{state['status']}: {state['reason']}"


def _approve(args: argparse.Namespace) -> int:
    _require_operator()
    if args.detach and not args.continue_after:
        raise tasks.TaskStartError("--detach needs --continue")
    for flag, given in (("--remember", args.remember), ("--local-sockets", args.local_sockets)):
        if given and args.gate != checks.APPROVAL_GATE:
            raise tasks.TaskStartError(f"{flag} applies only to the {checks.APPROVAL_GATE} gate")
    actor = _actor(args)
    repo = _repository()
    task_store = _existing(repo, args.task)
    if args.detach:
        command = ["approve", "--continue", f"--actor={actor}", *(["--remember"] if args.remember else []),
                   *(["--local-sockets"] if args.local_sockets else []),
                   *([f"--revision={args.revision}"] if args.revision else []), "--", args.task, args.gate]
        return _detach(_task_config(repo, task_store), task_store, command,
                       before=lambda: _require_answerable(task_store, args))
    asked = _ask_of(repo, task_store, waits.APPROVAL, args.gate)
    with task_store.lock():
        _require_answerable(task_store, args)
        with task_store.as_side_writer():  # a lane's own store, alive in this process or beside it, must not read this as tampering
            _record_approval(task_store, args.gate, actor, args.remember, args.local_sockets)
    decided.record_now(asked, "approve", actor)
    if args.continue_after:
        print(f"approved; continuing {args.task}")
        return _drive(repo, task_store)
    print(f"approved; run sakura flow resume {args.task}")
    return exits.OK


def _require_answerable(task_store: store.TaskStore, args: argparse.Namespace) -> None:
    """Raise TaskStartError unless the gate may be approved and, when the answer names a revision, the gate still
    shows that one. The authoritative call runs under the task lock, in the process that records the approval."""
    _require_approvable(task_store, args.task, args.gate, args.local_sockets)
    state = task_store.read_state()
    sockets_only = args.gate == checks.APPROVAL_GATE and args.local_sockets
    if not waits.gate_pending(state, args.gate) and not sockets_only:
        raise tasks.TaskStartError(f"no longer waiting: {args.task} is not waiting for the approval of the {args.gate} "
                                   "gate; nothing was done")
    _require_revision(args.revision, waits.approval_revision(task_store, state, args.gate), args.task)


def _require_revision(given: str | None, current: str, task: str) -> None:
    """Raise TaskStartError (`no longer waiting`, nothing changed) when `given` is not the revision now waiting."""
    if given is not None and given != current:
        raise tasks.TaskStartError(f"no longer waiting: {task} has moved on since the answer was drawn; nothing was done")


def _ask_of(repo: Path, task_store: store.TaskStore, kind: str, subject: str) -> dict:
    """The ask the person is about to answer, as the decided-asks log keeps it: the engine's own record while it is
    readable, else what this command knows."""
    return decided.find_wait(repo, [task_store], kind, subject) or decided.fallback_wait(repo, task_store.task_id, kind, subject)


def _gate_name(text: str) -> str:
    """Argparse type of the approve gate: `spec`, `budget`, `budget-wait`, or `opus:<build task>`."""
    task_id = text.removeprefix(engine.OPUS_GATE_PREFIX)
    if text in (engine.SPEC_GATE, engine.BUDGET_GATE, engine.BUDGET_WAIT_GATE, engine.OVERLAP_GATE, engine.FINAL_REVIEW_GATE,
                checks.APPROVAL_GATE) \
            or (text.startswith(engine.OPUS_GATE_PREFIX) and task_id):
        return text
    raise argparse.ArgumentTypeError(f"unknown gate {text!r}; use {engine.SPEC_GATE}, {engine.BUDGET_GATE}, "
                                     f"{engine.BUDGET_WAIT_GATE}, {engine.OVERLAP_GATE}, {engine.FINAL_REVIEW_GATE} or "
                                     f"{engine.OPUS_GATE_PREFIX}<build task>")


def _require_approvable(task_store: store.TaskStore, task: str, gate: str, local_sockets: bool = False) -> None:
    """Raise TaskStartError unless `gate` of the task may be approved.

    The `checks` gate also takes `--local-sockets` alone: a check row can fail on a socket the cage refused after its
    command was approved, and then no command waits."""
    from sakura.flow.phases import intake

    if gate == engine.BUDGET_GATE:
        if not (task_store.read_state().get("budget") or {}).get("preflight"):
            raise tasks.TaskStartError(f"cannot approve {task}: no budget preflight to approve yet")
        return
    if gate == engine.BUDGET_WAIT_GATE:
        if not task_store.read_state().get(budget_guard.WAIT_KEY):
            raise tasks.TaskStartError(f"cannot approve {task}: not waiting on the budget")
        return
    if gate == checks.APPROVAL_GATE:
        if not task_store.read_state().get(checks.PENDING_KEY) and not local_sockets:
            raise tasks.TaskStartError(f"cannot approve {task}: no contract commands wait for approval")
        return
    if gate == engine.FINAL_REVIEW_GATE:
        if not (task_store.read_state().get("final_review") or {}).get("asked_head"):
            raise tasks.TaskStartError(f"cannot approve {task}: no final review waits for approval")
        return
    if gate == engine.OVERLAP_GATE:
        if not task_store.read_state().get("overlap"):
            raise tasks.TaskStartError(f"cannot approve {task}: no overlap waits for approval")
        return
    if gate.startswith(engine.OPUS_GATE_PREFIX):
        task_id = gate.removeprefix(engine.OPUS_GATE_PREFIX)
        if task_id not in task_store.read_state().get("build", {}):
            raise tasks.TaskStartError(f"cannot approve {task}: no build task {_without_controls(task_id)}")
        return
    problem = intake.spec_problem(task_store.spec.read_text()) if task_store.spec.exists() else "no spec.md yet"
    if problem:
        raise tasks.TaskStartError(f"cannot approve {task}: {problem}")


def _record_approval(task_store: store.TaskStore, gate: str, actor: str, remember: bool = False,
                     local_sockets: bool = False) -> None:
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    if gate == engine.SPEC_GATE:
        digest = hashlib.sha256(task_store.spec.read_bytes()).hexdigest()
        task_store.update_state(lambda state: state["approvals"].update(spec={"at": at, "sha256": digest}))
        _ledger_request(task_store, "spec approved", actor)
    elif gate == engine.BUDGET_GATE:
        task_store.update_state(lambda state: state["approvals"].update(
            {gate: {"at": at, "plan_sha256": state.get("plan_sha256", "")}}))
        _ledger_request(task_store, "budget approved", actor)
        budget_guard.resolve_request_alerts(task_store, _task_config(task_store.repo, task_store))
    elif gate == checks.APPROVAL_GATE:
        pending = task_store.read_state().get(checks.PENDING_KEY)
        if pending:
            task_store.update_state(lambda state: state["approvals"].update(
                {gate: {"at": at, "sha256": pending["sha256"], "commands": pending["commands"],
                        "flows": pending.get("flows", "")}}))
            _ledger_request(task_store, f"{len(pending['commands'])} contract commands approved "
                                        f"({pending['sha256'][:12]})", actor)
        if remember and pending:
            _remember_commands(task_store, pending["commands"], actor)
        if local_sockets:
            _remember_local_sockets(task_store, actor)
    elif gate == engine.BUDGET_WAIT_GATE:
        cfg = _task_config(task_store.repo, task_store)
        budget_guard.resolve_request_alerts(task_store, cfg)
        if budget_guard.release_wait(task_store, budget_settings.load(cfg.values), datetime.now(timezone.utc)):
            _ledger_request(task_store, "budget wait released: the rule that stopped the flow is lifted until the "
                                        "wait's end, the other rules keep applying", actor)
    elif gate == engine.OVERLAP_GATE:
        task_store.update_state(lambda state: state["approvals"].update(
            {gate: {"at": at, "head": (state.get("overlap") or {}).get("head", ""),
                    "revision": (state.get("overlap") or {}).get("revision", "")}}))
        _ledger_request(task_store, "delivery approved despite an overlap with another lane", actor)
    else:
        task_store.update_state(lambda state: state["approvals"].update(
            {gate: {"at": at, "plan_sha256": state.get("plan_sha256", "")}}))
        _ledger_request(task_store, f"{_without_controls(gate)} approved", actor)


def _remember_commands(task_store: store.TaskStore, commands: list[str], actor: str) -> None:
    """Add the approved plain-words commands to the project's remembered list and say what was and was not added."""
    remembered, skipped = checks.remember(task_store.repo, commands)
    _ledger_request(task_store, f"{len(remembered)} contract commands remembered for later contracts", actor)
    print(f"remembered {len(remembered)} commands; later contracts run them without asking")
    if skipped:
        print(f"{len(skipped)} commands use shell syntax and stay approved for this task only")


def _remember_local_sockets(task_store: store.TaskStore, actor: str) -> None:
    """Record that this project's commands may open their own loopback sockets, and say what that opened."""
    checks.remember_local_sockets(task_store.repo)
    _ledger_request(task_store, "local sockets approved for this project's commands", actor)
    print("local sockets approved for this project: its commands may use the system's high loopback ports "
          "(debuggers, dev servers and other tasks' apps listen there) from now on")


def _mode(args: argparse.Namespace) -> int:
    """Show the gate mode, or pin it machine-wide (`auto` clears the pin)."""
    override_file = gates.default_mode_file()
    if args.mode:
        _require_operator()
        gates.set_override(override_file, args.mode)
    mode, source = gates.effective_mode(config.load(_repository()), datetime.now(), override_file)
    print(f"mode: {mode} ({source})")
    return exits.OK


def _wake(args: argparse.Namespace) -> int:
    """Resume the flows whose budget wait is over (run by the usage archive every 15 minutes)."""
    _require_operator()
    for line in wake.wake(resume_detached, server_running=tmux_server_running):
        print(line)
    return exits.OK


def tmux_server_running() -> bool:
    """Whether a tmux server is already up for the surface; a host without tmux has none."""
    try:
        return _new_tmux_client().has_server()
    except TmuxError:
        return False


def resume_detached(repo: Path, task_store: store.TaskStore, actor: str = WAKE_ACTOR) -> None:
    """Resume a task in the `engine` window of its session, as `resume --detach` does; `actor` is who the ledger names."""
    command = ["resume", f"--actor={actor}", "--", task_store.task_id]
    _detach(_task_config(repo, task_store), task_store, command)


def _register(args: argparse.Namespace) -> int:
    """Add an existing repository to the registry the cockpit reads."""
    _require_operator()
    repo = repository.root(Path(args.path))
    registry.remember(repo)
    print(f"registered {_without_controls(str(repo))}")
    return exits.OK


def _status(args: argparse.Namespace) -> int:
    repo = _repository()
    if not args.task:
        result = config.load(repo)
        payload = {"effective": result.values, "layers": result.origins, "relaxations": result.relaxations,
                   "load": _load_snapshot(result), "tasks": _project_tasks(repo)}
        print(json.dumps(payload, indent=2, sort_keys=True) if args.json else _load_status_lines(payload))
        return exits.OK
    payload = _task_status(repo, _existing(repo, args.task))
    print(json.dumps(payload, indent=2, sort_keys=True) if args.json else _status_lines(payload))
    return exits.OK


def _sessions(args: argparse.Namespace) -> int:
    """List the runs of a task with their host, state and the safe command that reopens each."""
    repo = _repository()
    task_store = _existing(repo, args.task)
    state = task_store.read_state()
    records = sorted(state.get("runs", {}).values(), key=lambda run: (run["started_at"], run["name"]))
    common = repository.common_dir(repo)
    agents = agent_identity.identities(task_store, state)
    views = [{**_session_view(task_store, state, run_view.effective(task_store, record), common), **agents[record["name"]]}
             for record in records]
    if args.json:
        print(json.dumps(views, indent=2, sort_keys=True))
    else:
        print("\n".join(_session_line(view) for view in views) or "no runs recorded")
    return exits.OK


def _session_view(task_store: store.TaskStore, state: dict, record: dict, common_dir: Path) -> dict:
    """The operator view of a run (`run_records.describe`) with what it is doing and how a message reaches it."""
    listening, not_listening = run_view.listening(task_store, state, record, common_dir)
    return {**run_records.describe(record, common_dir), "doing": run_view.doing(task_store, record),
            "listening": listening, "not_listening": not_listening}


def _session_line(view: dict) -> str:
    resume = shlex.join(view["resume"]) if view["resume"] else f"not resumable: {view['not_resumable']}"
    parts = (view["name"], view["role"], ":".join(filter(None, (view["provider"], view["model"]))), view["host"], view["state"],
             f"session={view['session_id'] or '-'}", resume)
    return _without_controls("  ".join(str(part) for part in parts))


def _feed(args: argparse.Namespace) -> int:
    """Print one page of the task's feed, or with `--run` of one agent's conversation, as one line of JSON
    (`flow/feed.py`, `flow/agent_feed.py`)."""
    task_store = _existing(_repository(), args.task)
    try:
        page = agent_feed.read(task_store, args.run, args.since) if args.run else feed.read(task_store, args.since)
        print(page.to_json())
    except feed.FeedRefused as refusal:
        raise tasks.TaskStartError(str(refusal)) from None
    return exits.OK


def _changes(args: argparse.Namespace) -> int:
    """Print the changed files of a lane, or the patch of one of them, as one line of JSON (`flow/changes.py`). Every
    refusal is a fixed sentence; what failed goes to stderr."""
    task_store = store.TaskStore(_repository(), args.task) if store.TASK_ID.fullmatch(args.task) else None
    if task_store is None or not task_store.exists():
        raise tasks.TaskStartError("unknown task")
    try:
        document = changes.listing(task_store.read_state()) if args.file is None else \
            changes.patch(task_store.read_state(), args.file)
    except changes.ChangesRefused as refusal:
        if refusal.__cause__:
            print(f"changes: {_without_controls(str(refusal.__cause__))}", file=sys.stderr)
        raise tasks.TaskStartError(str(refusal)) from None
    print(json.dumps({"task": args.task, "note": changes.DATA_NOTE, **document}))
    return exits.OK


def _watch(args: argparse.Namespace) -> int:
    """Render one run's log live until its record has ended."""
    task_store = _existing(_repository(), args.task)
    try:
        render.follow(task_store, args.run, sys.stdout)
    except run_records.RunRecordError as error:
        raise tasks.TaskStartError(str(error)) from error
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    return exits.OK


def _events(args: argparse.Namespace) -> int:
    """Follow the task ledger until the window or the terminal is closed; `--wait` first waits for the task."""
    repo = _repository()
    task_store = _await_task(repo, args.task) if args.wait else _existing(repo, args.task)
    try:
        render.follow_ledger(task_store, sys.stdout)
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    return exits.OK


def _await_task(repo: Path, task: str) -> store.TaskStore:
    """Return the task once it exists; a detached `start` shows its ledger window before the task is created."""
    if not store.TASK_ID.fullmatch(task):
        return _existing(repo, task)  # refuses with the unknown-task sentence, not a state error
    task_store = store.TaskStore(repo, task)
    deadline = time.monotonic() + EVENTS_WAIT_SECONDS
    while not task_store.exists() and time.monotonic() < deadline:
        time.sleep(EVENTS_POLL_SECONDS)
    return _existing(repo, task)


def _new_tmux_client() -> TmuxClient:
    return tmux.surface_client()


def _enter_session(argv: list[str]) -> int:
    """Bring this terminal to the session (switch-client inside tmux, attach-session outside) and return the exit code."""
    executable = programs.path(tmux.TMUX)
    return subprocess.run([executable, *argv[1:]], check=False, env=programs.environment("tmux", executable)).returncode


def _task_config(repo: Path, task_store: store.TaskStore) -> config.FlowConfig:
    return config.load(repo, overrides=task_store.read_state().get("overrides") or None)


def _tmux_surface(cfg: config.FlowConfig) -> surface.TmuxSurface:
    return surface.require_tmux_surface(cfg, _new_tmux_client())


def _detach(cfg: config.FlowConfig, task_store: store.TaskStore, command: list[str],
            before: Callable[[], None] = lambda: None) -> int:
    """Run `sakura flow <command>` in the `engine` window of the task session instead of this terminal.

    `before` is a refusal check that runs once tmux is known to be usable, so a missing tmux is reported first.
    """
    tmux = _tmux_surface(cfg)
    before()
    if task_store.exists():
        with task_store.lock():
            pass  # a foreground driver holds the lock: refuse now instead of showing a failing window
    argv = [str(surface.KIT_BIN), "flow", *command]
    env = {**surface.driver_environment(os.environ), **interpreter.environment()}
    session = tmux.start_engine(task_store, argv, env)
    print(f"detached: {session}; sakura flow attach {task_store.task_id}")
    return exits.OK


def _attach(args: argparse.Namespace) -> int:
    """Rebuild the session of a task (ledger window, a watch window per unended run) and go there."""
    _require_operator()
    repo = _repository()
    task_store = _existing(repo, args.task)
    tmux = _tmux_surface(_task_config(repo, task_store))
    tmux.rebuild(task_store)
    return exits.OK if _enter_session(tmux.attach_command(task_store)) == 0 else exits.FAILED


def _open(args: argparse.Namespace) -> int:
    """Reopen a run in tmux or send a message to its Goose conversation."""
    _require_operator()
    repo = _repository()
    task_store = _existing(repo, args.task)
    cfg = _task_config(repo, task_store)
    state = task_store.read_state()
    runs = state.get("runs", {})
    record = runs.get(args.run)
    message = args.message
    terminal = None
    # A bare open reports unavailable tmux before refusing an unknown run.
    if message is None and not args.fresh and (record or {}).get("provider") != "goose":
        terminal = _tmux_surface(cfg)
    if record is None:
        raise tasks.TaskStartError(f"unknown run {_without_controls(args.run)}; runs: {', '.join(runs) or 'none'}")
    if args.fresh:
        if not message or not message.strip():
            raise conversations.ReopenRefused("Pass --message for a fresh Goose attempt.")
        with task_store.lock():
            result = agent_resume.fresh(engine.open_context(repo, task_store), args.run, message)
        print(f"recorded {result.log_path.stem}")
        return exits.OK if result.exit_code == 0 else exits.FAILED
    common = repository.common_dir(repo)
    plan = conversations.plan_reopen(record, task_store, state, common)
    if plan.acp:
        if not message or not message.strip():
            raise conversations.ReopenRefused("Pass --message to continue the Goose conversation.")
        outcome = agent_steer.send(task_store, cfg, common, args.run, message, "cli:person")
        print(_without_controls(outcome.words))
        return exits.FAILED if outcome.outcome == agent_steer.REFUSED else exits.OK
    if terminal is None:
        raise conversations.ReopenRefused("--message requires a Goose conversation.")
    return _open_terminal(task_store, args.run, record, plan, terminal)


def _open_terminal(task_store: store.TaskStore, run: str, record: dict, plan: conversations.Reopening,
                   terminal: surface.TmuxSurface) -> int:
    session = terminal.session_for(task_store)
    attach = f"sakura flow attach {task_store.task_id}"
    if terminal.open_conversation(task_store, run, plan.argv, plan.cwd, plan.env):
        opened = f"owner opened {run} ({record['provider']} {record['session_id']})"
        task_store.append_ledger("operator", _without_controls(opened))
        print(f"opened {run}-open in {session}; {attach}")
    else:
        print(f"{run}-open is already open in {session}; {attach}")
    return exits.OK


def _report(args: argparse.Namespace) -> int:
    """Print a final comprehension report or write a clearly local status report."""
    task_store = _existing(_repository(), args.task)
    with task_store.lock():
        if task_store.report.is_file():
            report_text = task_store.report.read_text()
            print(report_text, end="")
            return exits.OK
        state = task_store.read_state()
        if state["status"] not in local_report.LOCAL_STATUSES:
            if state["status"] == "running":
                raise tasks.TaskStartError(f"task {args.task} is still running; no local report was written")
            raise tasks.TaskStartError(f"final report is unavailable for task {args.task} ({state['status']})")
        print(local_report.write(task_store), end="")
    return exits.OK


def _cleanup(args: argparse.Namespace) -> int:
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    task_store = _existing(repo, args.task)
    if args.discard:
        return _discard(repo, task_store, args.task, actor, args.revision)
    if args.revision is not None:
        raise tasks.TaskStartError("--revision requires --discard")
    state = task_store.read_state()
    if state["phase"] != "cleanup" and state["status"] != "cleaned":
        raise tasks.TaskStartError(f"task {args.task} is not ready for cleanup")
    return _drive(repo, task_store, "cleanup requested", actor)


def _discard(repo: Path, task_store: store.TaskStore, task: str, actor: str,
             expected: str | None = None) -> int:
    """Discard a failed or stopped lane under its lock: nothing runs, nothing is delivered, the lane is cleaned."""
    from sakura.flow.phases import cleanup

    with task_store.lock():
        state = task_store.read_state()
        if expected is not None:
            _require_revision(expected, resume.revision(state), task)
        if state["status"] not in cleanup.DISCARDABLE:
            raise tasks.TaskStartError(f"cannot discard {task}: only a failed or stopped lane can be discarded "
                                       f"(it is {state['status']})")
        if state.get("active_run") or state.get("active_runs"):
            raise tasks.TaskStartError(f"cannot discard {task} while runs are active")
        blocked = decided.find_wait(repo, [task_store], waits.BLOCKED, decided.ANY_SUBJECT)
        _ledger_request(task_store, "discard requested", actor)
        failure = cleanup.discard(engine.open_context(repo, task_store))
    if failure:
        raise tasks.TaskStartError(failure)
    if blocked:
        decided.record_now(blocked, "close", actor)
    print(f"discarded {task}")
    return exits.OK


def _run_row(args: argparse.Namespace) -> int:
    """Run a contract row for an evaluator task as the engine runs a check row (see `evaluate.run_row`)."""
    from sakura.flow import contract
    from sakura.flow.phases import evaluate

    task_id = os.environ.get("SAKURA_FLOW_TASK", "")
    if not task_id:
        raise tasks.TaskStartError("run-row is available only inside an evaluator task")
    # The workspace is a role's: its `.git` never says which repository or task this is (review 10, finding 1).
    try:
        task_store, _workspace = task_repository.open_for_helper(os.environ, task_id, Path.cwd())
    except task_repository.TaskRepositoryError as error:
        raise tasks.TaskStartError(f"run-row refused: {error}") from None
    state = task_store.read_state()
    try:
        rows = contract.load(task_store, state["contract_sha256"])
    except contract.ContractError as error:
        raise tasks.TaskStartError(str(error)) from error
    row = next((entry for entry in rows if entry.id == args.row_id and args.viewport in (None, entry.viewport)), None)
    if row is None:
        raise tasks.TaskStartError(f"unknown contract row: {args.row_id}")
    try:
        return evaluate.run_row(engine.open_context(task_store.repo, task_store), row)
    except contract.ContractError as error:
        raise tasks.TaskStartError(str(error)) from error


def _mark(args: argparse.Namespace) -> int:
    """Record the person's ok or ko on an owner row; refused inside an engine role and for rows the engine proves."""
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    if args.detach and not args.continue_after:
        raise tasks.TaskStartError("--detach needs --continue")
    task_store = _existing(repo, args.task)
    asked = _ask_of(repo, task_store, waits.OWNER_ROW, args.row)
    with task_store.lock():
        cfg = _task_config(repo, task_store)
        steps = config.steps(cfg.values)
        _require_revision(args.revision, waits.owner_revision(task_store.read_state(), args.row), args.task)
        owner_marks.mark(task_store, steps, args.row, args.verdict, args.note, actor)
        proceed = args.continue_after and owner_marks.all_marked(task_store, task_store.read_state(), steps)
    decided.record_now(asked, f"mark_{args.verdict}", actor, text=args.note or None)
    if proceed:
        print(f"row {args.row} marked {args.verdict}")
        return _detach_resume(cfg, task_store, actor)
    print(f"row {args.row} marked {args.verdict}; run sakura flow resume {args.task}")
    return exits.OK


def _reply(args: argparse.Namespace) -> int:
    """Record the person's answer to a question a role asked and hand it to the lane as a steer (queued for the next
    safe boundary); refused inside an engine role, for a question that is not waiting and for an answer that is none."""
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    task_store = _existing(repo, args.task)
    asked = _ask_of(repo, task_store, waits.QUESTION, args.question)
    try:
        lane_questions.answer(task_store, args.question, args.digest, args.choice, args.text, actor,
                              datetime.now(timezone.utc), lambda line: inbox.enqueue(task_store, "reply", line, "", actor))
    except lane_questions.ReplyRefused as refusal:
        raise tasks.TaskStartError(str(refusal)) from None
    decided.record_now(asked, "choice" if args.choice else "reply", actor, choice=args.choice or None, text=args.text or None)
    print(f"answered {args.question}; the lane gets it as a message {QUEUED_NOTICE}")
    return exits.OK


def _restore_head(args: argparse.Namespace) -> int:
    """Put workspace 0 back on the engine's recorded head under the task lock (`final_head.show`: Git refuses when a
    change there would be lost), so the person can mark an owner row again; refused inside an engine role."""
    _require_operator()
    actor = _actor(args)
    repo = _repository()
    task_store = _existing(repo, args.task)
    with task_store.lock():
        state = task_store.read_state()
        workspace = (state.get("workspaces") or {}).get("0", {}).get("path")
        if not workspace or not Path(workspace).is_dir():
            raise tasks.TaskStartError(f"cannot restore {args.task}: it has no workspace 0")
        _require_restorable(task_store, state, Path(workspace), args)
        ctx = engine.open_context(task_store.repo, task_store)
        try:
            final_head.restore(ctx, Path(workspace), state["branch"],
                               args.expect_workspace or _workspace_head(Path(workspace)))
        except final_head.WorkspaceMoved as error:
            raise tasks.TaskStartError(f"no longer waiting: {args.task} moved on while it was restored; nothing was "
                                       f"moved ({_without_controls(str(error))})") from error
        except gitutil.GitError as error:
            raise tasks.TaskStartError(f"cannot restore {args.task}: {_without_controls(str(error))}") from error
        task_store.append_ledger("owner", f"workspace 0 put back on the engine's head by {actor}")
    print(f"workspace 0 is on the engine's head; mark the row again: sakura flow mark {args.task} <row> ok|ko")
    return exits.OK


def _require_restorable(task_store: store.TaskStore, state: dict, workspace: Path, args: argparse.Namespace) -> None:
    """Raise TaskStartError (`no longer waiting`, nothing moved) unless workspace 0, the sealed head and the owner row
    are still what the person was shown. Runs under the task lock, in the process that moves the branch."""
    _require_revision(args.revision, waits.owner_revision(state, args.row or ""), args.task)
    for given, current in ((args.expect_workspace, _workspace_head(workspace)), (args.expect_head, final_head.recorded(state))):
        _require_revision(given, current, args.task)


def _workspace_head(workspace: Path) -> str:
    """The head workspace 0 is at."""
    return gitutil.git(workspace, "rev-parse", "HEAD").strip()


def _task_status(repo: Path, task_store: store.TaskStore) -> dict:
    state = task_store.read_state()
    cfg = config.load(repo, overrides=state.get("overrides") or None)
    return {
        "task": state["task"], "phase": state["phase"],
        "status": lane_ceiling.state_word(state["status"], delivery.of(state) is not None),
        "engine_status": state["status"], "delivery": delivery.of(state), "reason": state["reason"],
        "stop_requested": task_store.stop_file.exists(), "approvals": sorted(state.get("approvals", {})),
        "preset": cfg.values["preset"], "dial": cfg.values["dial"], "contract": _contract_counts(task_store),
        "roles": cfg.values["roles"], "relaxations": cfg.relaxations,
        "load": _load_snapshot(cfg), "pending_commands": _pending_commands(task_store),
        "waiting": owner_marks.waiting(task_store, config.steps(cfg.values)),
        "cage_denials": owner_marks.denials(state),
    }


def _pending_commands(task_store: store.TaskStore) -> list[dict]:
    """The queued operator commands, oldest first; an unregistered file shows as kind `unauthenticated`."""
    return [_pending_view(task_store, path, command) for path, command in inbox.pending(task_store)]


def _pending_view(task_store: store.TaskStore, path: Path, command: dict) -> dict:
    if not inbox.is_registered(task_store, path):
        return {"kind": "unauthenticated"}
    return command if inbox.is_well_formed(command) else {"kind": "unreadable"}


def _contract_counts(task_store: store.TaskStore) -> dict:
    if not task_store.contract.exists():
        return {"passed": 0, "total": 0}
    try:
        rows = json.loads(task_store.contract.read_text())["rows"]
    except (OSError, ValueError, KeyError) as error:
        raise store.StateError(f"cannot read {task_store.contract}: {error}") from error
    return {"passed": sum(1 for row in rows if row.get("passes")), "total": len(rows)}


def _status_lines(payload: dict) -> str:
    lines = [f"task: {payload['task']}", f"phase: {payload['phase']}", f"status: {payload['status']}"]
    if payload["stop_requested"]:
        lines.append("stop requested: yes")
    if payload.get("delivery"):
        lines.append(f"delivered: {delivery.describe(payload['delivery'])}")
    if payload["approvals"]:
        lines.append(f"approved: {', '.join(payload['approvals'])}")
    if payload["reason"]:
        lines.append(f"reason: {payload['reason']}")
    contract = payload["contract"]
    lines += [f"preset: {payload['preset']}", f"dial: {payload['dial']}", f"contract: {contract['passed']}/{contract['total']}"]
    lines += [f"role {name}: {value}" for name, value in payload["roles"].items()]
    lines += [_pending_line(command) for command in payload["pending_commands"]]
    lines += [f"waiting on you: {_without_controls(item)}" for item in payload.get("waiting", [])]
    lines += [f"cage denied: {_without_controls(item)}" for item in payload.get("cage_denials", [])]
    lines += _load_lines(payload["load"])
    lines += [f"relaxed: {item}" for item in payload["relaxations"]]
    return "\n".join(lines)


def _pending_line(command: dict) -> str:
    """One status line for a queued command; steer text is never echoed (it may hold control characters)."""
    kind = command.get("kind", "unreadable")
    if kind in inbox.STEER_KINDS:
        return f"pending: {kind} ({len(str(command.get('value', '')))} chars)"
    detail = " ".join(part for part in (command.get("role", ""), command.get("value", "")) if part)
    return f"pending: {kind} {_without_controls(detail)}".rstrip()


def _without_controls(text: str) -> str:
    """Remove control sequences, turn newlines and tabs into spaces, and collapse the whitespace."""
    return " ".join(render.strip_controls(text.replace("\n", " ").replace("\t", " ")).split())


def _load_snapshot(cfg: config.FlowConfig) -> dict:
    return loadinfo.current_load(cfg.values["load"]["slots"])


def _project_tasks(repo: Path) -> list[dict]:
    """Every task of the project with its phase and what it waits for (the stop reason, then the person's open items)."""
    views = []
    for task in tasks.known_tasks(repo):
        task_store = store.TaskStore(repo, task)
        try:
            state = task_store.read_state()
            steps = config.steps(_task_config(repo, task_store).values)
            waiting = [state["reason"]] if state["status"] in WAITING_STATUSES and state["reason"] else []
            views.append({"task": task, "phase": state["phase"],
                          "status": lane_ceiling.state_word(state["status"], delivery.of(state) is not None),
                          "engine_status": state["status"],
                          "waiting": waiting + owner_marks.waiting(task_store, steps)})
        except (store.StateError, KeyError, config.FlowConfigError) as error:
            views.append({"task": task, "phase": "?", "status": "unreadable", "waiting": [str(error)]})
    return views


def _task_list_lines(views: list[dict]) -> list[str]:
    if not views:
        return ["tasks: none"]
    lines = ["tasks:"]
    for view in views:
        lines.append(f"  {view['task']}  phase: {view['phase']}  status: {view['status']}")
        lines += [f"    waiting: {_without_controls(item)}" for item in view["waiting"]]
    return lines


def _load_status_lines(payload: dict) -> str:
    settings = payload["effective"]
    lines = [f"preset: {settings['preset']}", f"dial: {settings['dial']}"]
    lines += _task_list_lines(payload["tasks"])
    lines += _load_lines(payload["load"])
    lines += [f"relaxed: {item}" for item in payload["relaxations"]]
    return "\n".join(lines)


def _load_lines(load: dict) -> list[str]:
    lines = [f"load slots: {load['slots_in_use']}/{load['slots_total']}",
             f"load queue: {len(load['queue'])}",
             f"load average: {', '.join(str(value) for value in load['loadavg'])}"]
    lines += [f"waiting: {item['command']} ({item['waiting_seconds']} s)" for item in load["queue"]]
    return lines


def _repository() -> Path:
    """The person's repository this command works on, also when it runs in a task's workspace (review 10, finding 4)."""
    try:
        return task_repository.operator_repository(Path("."))
    except task_repository.TaskRepositoryError as error:
        raise tasks.TaskStartError(str(error)) from None


def _existing(repo: Path, task: str) -> store.TaskStore:
    task_store = store.TaskStore(repo, task) if store.TASK_ID.fullmatch(task) else None
    if task_store is None or not task_store.exists():
        listed = ", ".join(tasks.known_tasks(repo)) or "none"
        raise tasks.TaskStartError(f"unknown task {_without_controls(task)}; tasks: {_without_controls(listed)}")
    return task_store


def _migrate_state(args: argparse.Namespace) -> int:
    """Copy agentkit-era reports and logs into the engine's archive, nothing that carries authority; prints the plan
    first, deletes nothing."""
    _require_operator()
    found = migrate.plan(repository.root(Path(args.repo)), Path.home())
    for copy in found.copies:
        print(f"will copy {copy.source} -> {copy.target}")
    for source, reason in found.refused:
        print(f"refused {source}: {reason}")
    for target in found.kept:
        print(f"kept {target} (already there)")
    if not (found.copies or found.refused or found.kept):
        print("nothing to migrate")
        return exits.OK
    changed = migrate.apply(found)
    for source, reason in changed:
        print(f"refused {source}: {reason}")
    print(f"copied {len(found.copies) - len(changed)} files")
    return exits.OK


COMMANDS = {"start": _start, "resume": _resume, "recontract": _recontract, "stop": _stop, "steer": _steer, "agent-worker": _agent_worker,
            "dial": _dial, "pace": _pace, "preset": _preset, "step": _step, "role": _role, "approve": _approve, "status": _status, "sessions": _sessions, "feed": _feed, "changes": _changes, "watch": _watch,
            "events": _events, "attach": _attach, "open": _open, "report": _report,
            "cleanup": _cleanup, "run-row": _run_row, "mode": _mode, "register": _register, "wake": _wake,
            "migrate-state": _migrate_state, "mark": _mark, "reply": _reply,
            "restore-head": _restore_head}
