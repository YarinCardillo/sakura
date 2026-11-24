"""Runs one role as a monitored agent process and reads its verdict."""

from __future__ import annotations

import json
import os
import shlex
import shutil
import string
import threading
import time
from collections.abc import Callable
from contextlib import ExitStack
from dataclasses import dataclass, field, replace
from importlib import resources
from pathlib import Path
from urllib.parse import urlparse

from sakura import engine_home, grants, interpreter, knowledge, programs
from sakura.budget import account as budget_account
from sakura.budget import routing
from sakura.flow import (
    acp,
    budget_guard,
    cage,
    checks,
    exchange,
    gitutil,
    goose_cage,
    goose_lane,
    inbox,
    knowledge_folders,
    lane_ceiling,
    launchers,
    live_input,
    logread,
    owner_marks,
    pinned_roots,
    process,
    provider_fallback,
    registry,
    resume_account,
    roles,
    rule_agents,
    run_log,
    run_records,
    run_times,
    stacks,
    step_accounts,
    task_repository,
)
from sakura.flow import config as flow_config
from sakura.flow.config import DIAL, DialStep, FlowConfig
from sakura.flow.launchers import Launcher, LaunchRequest
from sakura.flow.render import strip_controls
from sakura.flow.store import TaskStore
from sakura.flow.surface import NullSurface, Surface
from sakura.usage import leeway
from sakura.usage.archive import settings as usage_settings

PROMPT_PACKAGE = "sakura.flow"
PROMPT_DIR = "prompts"
PROMPT_SUFFIX = ".md"
STEER_HEADING = inbox.STEER_HEADING
# One short paragraph, added to the prompt of a Claude role whose class is granted `board_post` (never to a role that is
# not): when to post, and that one post per phase is the most.
BOARD_NOTE = ("Board: the other lanes and the person read this project's board, where you post with board_post. Post at most "
              "once per phase, one short note near its end, saying what you started, what you changed that other lanes may "
              "touch (files, interfaces), and that you finished; post nothing when none of that applies.")
RUNNER_BOUNDARY = "runner"
BUDGET_GUARDED_PROVIDERS = ("claude", "codex")
CAGED_PROVIDERS = ("claude", "codex", "exec")
EXCHANGE_ENV = "SAKURA_FLOW_EXCHANGE"
TESTS_FILE = "tests.json"
NICE_ROLES = {"planner", "builder", "plan_reviewer", "code_reviewer", "reporter"}
EVALUATOR_ROLE = "evaluator"
# The tmp folders of the isolated runs (`RunRequest.repository`), denied to every later isolated run.
ISOLATED_TMP_KEY = "isolated_tmp_folders"
KEPT_REQUESTS = 50
MAX_PORT = 65535


@dataclass(frozen=True)
class ResumeOf:
    """A run that continues a conversation: its record `name` (already begun), the `session_id` it continues, the `model`
    and `effort` it ran with and the account it ran on (`flow/resume_account.py`), which the guard judges and the agent is
    launched on."""

    name: str
    session_id: str
    model: str
    effort: str
    account: resume_account.ResumeAccount | None = None


@dataclass(frozen=True)
class RunRequest:
    """What to run: the role, the phase it serves, its prompt and where it runs.

    `task_class` and `failures` (failed review rounds of the build task) steer the model route of a builder;
    `opus_approved` says the owner approved an Opus run for this task. `repository`: the Git folder of a repository
    of the role's own (the reporter's, a reviewer's: `task_repository.make_role_repository`), whose worktree `cwd`
    is; the run is then isolated from the task: its cage reads and writes nothing of the task's workspaces,
    repository or earlier runs' tmp folder. None: a workspace of the task's own repository.
    `step`: the pipeline step the run serves when it is not the role's own (the final review runs the code reviewer),
    so the launcher and the budget guard use that step's account; empty: the role's step.
    `resume`: the run continues a conversation (`flow/agent_resume.py`); `template` is then the person's message as
    written (no prompt template, no steers of the lane), the model and effort are the conversation's own, and a hung
    run is not started again.
    """

    role: str
    phase: str
    template: str
    values: dict[str, str]
    cwd: Path
    extra_env: dict[str, str] = field(default_factory=dict)
    task_class: str | None = None
    failures: int = 0
    opus_approved: bool = False
    repository: Path | None = None
    step: str = ""
    resume: ResumeOf | None = None
    # The Leeway row of the account the run uses because its provider has none (`flow/provider_fallback.py`); the
    # runner sets it, a caller leaves it empty.
    fallback_account: dict | None = None
    fresh_from: str = ""


@dataclass(frozen=True)
class RunResult:
    """What came back from a run."""

    exit_code: int | None
    final: str
    result: dict | None
    log_path: Path
    stalled: bool
    stopped: bool
    timed_out: bool = False
    overflowed: bool = False
    log_replaced: bool = False
    left_running: bool = False


def _read_log(log_path: Path) -> str:
    """The run's log, bounded; a log that is not a regular file fails the run."""
    try:
        return logread.read_log(log_path)
    except logread.UnreadableLog as error:
        raise roles.RoleRefused(str(error)) from None


def _knowledge(cfg: FlowConfig, request: RunRequest, git_folder: Path) -> dict[str, str]:
    """The knowledge folders the run's cage may read, by name; a folder a role could use to reach more fails the run."""
    try:
        found = knowledge_folders.of(cfg.values, cage.account_home(), request.cwd, (git_folder,))
    except knowledge.KnowledgeRefused as error:
        raise roles.RoleRefused(str(error)) from error
    return {name: str(folder) for name, folder in found.items()}


def _is_hung(outcome: process.ProcessOutcome) -> bool:
    return outcome.stalled or outcome.timed_out


def _with_steer(prompt: str, texts: list[str]) -> str:
    return f"{prompt}\n\n" + "\n\n".join(texts) + "\n" if texts else prompt


def load_template(name: str) -> str:
    """Return the text of the packaged prompt `name`."""
    return (resources.files(PROMPT_PACKAGE) / PROMPT_DIR / f"{name}{PROMPT_SUFFIX}").read_text()


class AgentRunner:
    """Launches roles through a `Launcher`, recording each run and its active process in the task state.

    Safe to call from several threads: log numbers, steer delivery and the
    active-run list are serialized.
    """

    def __init__(self, store: TaskStore, config: FlowConfig, launcher: Launcher | None = None,
                 surface: Surface | None = None) -> None:
        self._store = store
        self._config = config
        self._overrides_seen = store.read_state().get("overrides") or None
        self._launcher = launcher or launchers.LocalLauncher()
        self._surface = surface or NullSurface()
        self._lock = threading.Lock()
        self._awaiting_replay: set[str] = set()  # continuations whose first message has not been seen read yet

    def run(self, request: RunRequest) -> RunResult:
        """Run the request to completion; raises RoleRefused when the role may not launch, BudgetWait when the budget says wait.

        A stalled or timed-out run is killed and restarted once with the same prompt.
        Every attempt is checked for engine hook calls, so a failing first attempt blocks the phase.
        A Claude or Codex role asks the budget guard first (after the mailbox is applied); BudgetWait leaves the flow's state as it is.
        """
        cfg = self._effective_config()
        route = self._resume_route(request.resume) if request.resume else self._route(cfg, request)
        spec = roles.parse_role(cfg.values["roles"][request.role])
        if request.fresh_from and spec.provider != "goose":
            raise roles.RoleRefused("The lane no longer runs this role on Goose.")
        spec, request = self._on_an_existing_account(spec, request, cfg)
        if spec.provider in BUDGET_GUARDED_PROVIDERS:
            budget_guard.require(self._store, cfg, provider=spec.provider, role=request.role, step=request.step,
                                 on_account=request.resume.account.account if request.resume and request.resume.account else None)
        spec = replace(spec, model=route.model) if route.model else spec
        prepared = exchange.prepare(self._store, request, self._others())
        # A caged `sakura test` cannot read the engine config, so the run's folder carries its test commands.
        tests = {suite: cfg.values["tests"][suite] for suite in ("focused", "full")}
        exchange.add_input(prepared, TESTS_FILE, json.dumps(tests).encode())
        request = replace(request, extra_env={**request.extra_env, EXCHANGE_ENV: str(prepared.folder)})
        evidence: tuple[str, ...] = ()
        try:
            prompt = request.template if request.resume or request.fresh_from else string.Template(request.template).substitute(prepared.values)
            if not request.resume and self._has_board_post(spec, request.role):
                prompt = f"{prompt}\n\n{BOARD_NOTE}"
            steer: list[str] = []
            outcome, log_path, last_message = self._attempt(cfg, spec, route, request, prompt, steer)
            if _is_hung(outcome) and not request.resume and spec.provider != "goose":
                self._store.append_ledger(request.phase, f"stall restart {request.phase}")
                outcome, log_path, last_message = self._attempt(cfg, spec, route, request, prompt, steer)
            if outcome.overflowed:
                self._store.append_ledger(request.phase, f"output cap exceeded: {log_path.stem} was terminated")
            if outcome.log_replaced:
                self._store.append_ledger(request.phase, f"log replaced during the run: {log_path.stem} was terminated")
            if outcome.left_running:
                self._store.append_ledger(request.phase, f"{log_path.stem} left processes running after it exited; "
                                                         "they were terminated")
            final = (outcome.final if spec.provider == "goose" else
                     roles.final_message(spec, _read_log(log_path), last_message))
            parsed = roles.parse_result(final)
            if request.phase in exchange.EVIDENCE_PHASES:
                evidence = exchange.evidence_names(parsed)
        finally:
            for name, reason in exchange.collect(prepared, evidence):
                self._store.append_ledger(request.phase, strip_controls(f"role output {name} ignored: {reason}"))
        return RunResult(outcome.exit_code, final, parsed, log_path, _is_hung(outcome), outcome.stopped, outcome.timed_out,
                         outcome.overflowed, outcome.log_replaced, outcome.left_running)

    def _on_an_existing_account(self, spec, request: RunRequest, cfg: FlowConfig):
        """The role's spec and request when its provider has no account: it runs on the first account there is, and the
        ledger says so (`flow/provider_fallback.py`). A conversation continues on its own account, and a step the person
        pinned to an account keeps it (a missing one is that step's own question)."""
        if request.resume or spec.provider not in provider_fallback.AGENT_PROVIDERS:
            return spec, request
        state = self._store.read_state()
        if step_accounts.account_of(cfg.values, state, request.role, request.step) != step_accounts.PROJECT_ACCOUNT:
            return spec, request
        try:
            rows = list(leeway.status()["accounts"])
        except leeway.LeewayError:
            return spec, request
        known = [rule_agents.AgentAccount(row["id"], row.get("label") or row["id"], row["provider"]) for row in rows]
        found = provider_fallback.substitute(spec.provider, request.role, known)
        if found is None:
            return spec, request
        self._store.append_ledger(request.phase, found.words)
        row = next(row for row in rows if row["id"] == found.account.id)
        return roles.RoleSpec(found.provider), replace(request, fallback_account=row)

    @staticmethod
    def _has_board_post(spec, role: str) -> bool:
        """Whether this role's class is granted the `board_post` tool (the host's grants decide)."""
        if spec.provider not in ("claude", "codex", "goose") or role not in grants.CLASSES:
            return False
        try:
            loaded = grants.load(engine_home.root(), cage.writable_roots())
        except (grants.GrantsError, flow_config.FlowConfigError, registry.RegistryError, OSError):
            return False
        return "board_post" in loaded.for_class(role).tools

    def _others(self) -> str:
        """The text of the run's `others.md`: the engine's record of every other open lane."""
        from sakura.flow import (
            others,  # imported here: `others` reads progress, which imports the build phase, which imports this module
        )

        try:
            lanes = registry.repos()
        except registry.RegistryError:
            lanes = []
        return others.summary(lanes, lane_ceiling.lane_name(self._store.repo, self._store.task_id),
                              engine_home.project_dir(self._store.repo))

    def _task_git_folder(self, cwd: Path, repository: Path | None = None) -> Path:
        """The Git folder a role's cage is given: the task's own repository's (`flow/task_repository.py`), or the
        role's own repository the state records (`repository`), and only for a run in one of its workspaces; the
        person's repository is never inside a cage."""
        refusal = f"no role runs in {cwd}: it is not a workspace of the task's own repository"
        try:
            folder = task_repository.common_dir(self._store)
            if repository is not None:
                state = self._store.read_state()
                if os.path.realpath(repository) not in task_repository.recorded_repositories(state):
                    raise task_repository.TaskRepositoryError(f"{repository} is not a repository the task records")
                task_repository.pin(state)
                folder = repository
        except task_repository.TaskRepositoryError as error:
            raise roles.RoleRefused(f"no role runs in {cwd}: {error}") from error
        try:
            found = gitutil.verified_common_dir(cwd)
        except gitutil.GitError as error:
            raise roles.RoleRefused(f"{refusal} ({error})") from error
        if os.path.realpath(found) != os.path.realpath(folder):
            raise roles.RoleRefused(refusal)
        return folder

    @staticmethod
    def _resume_route(resume: ResumeOf) -> routing.Route:
        """A conversation keeps the model and effort it began with; it needs no approval it did not already have."""
        return routing.Route(resume.model, resume.effort, "continues a conversation", False, "resume")

    def _effective_config(self) -> FlowConfig:
        """Apply waiting operator commands and return the config this call runs with; a running call keeps it."""
        with self._lock:
            consumed = inbox.consume(self._store, self._store.repo, RUNNER_BOUNDARY)
            overrides = self._store.read_state().get("overrides") or None
            # The engine may have taken a command in between two runs (`engine.current_steps`): the stored overrides
            # are the truth, not only what this call consumed.
            if consumed or overrides != self._overrides_seen:
                self._config = flow_config.load(self._store.repo, overrides=overrides)
                self._overrides_seen = overrides
            return self._config

    def _route(self, cfg: FlowConfig, request: RunRequest) -> routing.Route:
        """Ask the router for the model of this run; an Opus route the owner has not approved is refused."""
        role = routing.INTAKE if request.phase == "intake" else request.role
        route = routing.route(role, request.task_class, request.failures, cfg)
        if route.needs_approval and not request.opus_approved:
            message = f"{request.role} run needs the owner approval to use {route.model}: {route.reason}"
            self._store.append_ledger(request.phase, strip_controls(message))
            raise roles.RoleRefused(message)
        return route

    @staticmethod
    def _dial_for(cfg: FlowConfig, route: routing.Route, role: str = "") -> DialStep:
        """The dial position of the flow, with the effort the person chose for the step of `role` (`efforts.<role>`) when there is
        one, else the route's when the route sets one."""
        dial = DIAL[cfg.values["dial"]]
        effort = cfg.values["efforts"].get(role) or route.effort
        return replace(dial, effort=effort) if effort else dial

    def _attempt(self, cfg: FlowConfig, spec, route: routing.Route, request: RunRequest, prompt: str,
                 steer: list[str]) -> tuple[process.ProcessOutcome, Path, Path]:
        """Record and launch one run; `steer` collects every steer text delivered so far, so a restart repeats them."""
        if spec.provider == "goose":
            return self._attempt_goose(cfg, spec, request, prompt)
        values = cfg.values
        log_path = self._resume_log(request.resume.name) if request.resume else self._next_log(request.phase)
        last_message = log_path.with_suffix(".last.txt")
        # A role that is not caged (`none`) is refused by `argv_for` itself.
        git_folder = (self._task_git_folder(request.cwd, request.repository) if spec.provider in CAGED_PROVIDERS
                      else request.cwd)
        run_cage = self._cage(cfg, spec, request, log_path.stem, git_folder)
        argv = roles.argv_for(spec, request.role, self._dial_for(cfg, route, request.role), request.cwd, git_folder, last_message,
                              max_budget_usd=float(values["limits"]["claude_max_budget_usd"]), cage=run_cage,
                              resume_session=request.resume.session_id if request.resume else "")
        if spec.provider in {"claude", "codex"}:
            with log_path.open("a", encoding="utf-8") as log:
                log.write(f"$ {shlex.join(argv)}\n")
                if request.resume and spec.provider == "codex":
                    log.write(json.dumps({"type": run_log.MESSAGE_EVENT, "text": prompt}) + "\n")
            program, source = programs.resolution(spec.provider, request.cwd)
            self._store.append_ledger(request.phase, f"{spec.provider} program {program} (found in {source})")
        base_env, account_ref = self._launch_environment(request, log_path.stem, cfg, spec)
        env = {**base_env, **(run_cage.env if run_cage else {})}
        if request.resume:
            self._awaiting_replay.add(log_path.stem)
            env = resume_account.apply(env, spec.provider, request.resume.account) if request.resume.account else env
        else:
            steer.extend(self._begin_run(log_path.stem, cfg, spec, route, request, env, account_ref))
        self._surface.run_started(self._store, log_path.stem, request.role)
        live = live_input.LiveChannel() if spec.provider == "claude" else None
        marks = run_times.Marker(log_path)
        launch = LaunchRequest(
            argv, request.cwd, env, log_path, values["timeouts"][request.phase],
            values["timeouts"]["idle_seconds"], self._store.stop_file, _with_steer(prompt, steer),
            values["load"]["nice"] if request.role in NICE_ROLES else None,
            on_gap=self._record_gap, on_poll=lambda: self._poll(live, log_path, request.role, marks), live=live)
        started = time.time()
        try:
            outcome = self._launch_recorded(log_path.stem, launch)
        finally:
            self._awaiting_replay.discard(log_path.stem)
            marks.flush()
            self._end_live(live, log_path)
            if run_cage:
                self._ingest_requests(run_cage, request)
            if run_cage and request.repository is not None:
                shutil.rmtree(run_cage.tmp, ignore_errors=True)  # an isolated run's own folder, never the shared one
                pinned_roots.release(run_cage.tmp)
        self._require_hooks_ran(spec, request, log_path, started)
        return outcome, log_path, last_message


    def _attempt_goose(self, cfg: FlowConfig, spec, request: RunRequest, prompt: str):
        if self._launcher.host != launchers.LOCAL_HOST:
            raise roles.RoleRefused("Goose requires a qualified local Seatbelt host")
        log = self._resume_log(request.resume.name) if request.resume else self._next_log(request.phase)
        git_folder = self._task_git_folder(request.cwd, request.repository)
        site = self._site(cfg, spec, request, log.stem, git_folder)
        if site.tmp_dir is None:
            site = replace(site, tmp_dir=cage.new_isolated_tmp_dir(self._store.root))
        worker = None
        marks = run_times.Marker(log)
        try:
            policy = cage.load_policy(cfg.values["knobs"]["cage"])
            worker = cage.prepare(site, policy, 0)
            lane = self._goose_lane(cfg, spec, request, site, policy, worker)
            lane.recheck(self._store, site)
            with ExitStack() as resources:
                model = resources.enter_context(goose_cage.open_launch(
                    lane.installation, lane.provider, lane.route.provider, lane.configured_model, lane.root, site, policy, worker))
                lane.recheck(self._store, site)
                launch = self._goose_request(cfg, request, log, marks, prompt, lane, model, site, worker)
                outcome = self._launch_recorded(log.stem, launch, release=resources.close)
        except (cage.CageError, programs.ProgramError, OSError) as error:
            raise goose_lane.startup_error(error) from None
        finally:
            marks.flush()
            if worker:
                self._ingest_requests(worker, request)
            shutil.rmtree(site.tmp_dir, ignore_errors=True)
            pinned_roots.release(site.tmp_dir)
        if outcome.reason:
            detail = f"Goose turn ended: {outcome.reason}"
            if outcome.reason == "adapter_authentication_required":
                detail += ". Check the selected Goose account's connection and provider access."
            self._store.append_ledger(request.phase, detail)
        return outcome, log, log.with_suffix(".last.txt")

    def _goose_lane(self, cfg, spec, request, site, policy, worker):
        state = self._store.read_state()
        if request.resume:
            record = state["runs"][request.resume.name]
            ref = record.get("account_ref")
            if not isinstance(ref, dict) or ref.get("kind") != "goose" or request.resume.session_id != record.get("session_id"):
                raise cage.CageError("goose_conversation_binding_changed")
            return goose_lane.prepare(self._store, site, policy, worker, ref.get("id"), request.resume.model, record,
                                      effort=request.resume.effort)
        if request.fresh_from:
            previous = state["runs"][request.fresh_from]
            binding = previous["acp_binding"]
            return goose_lane.prepare(self._store, site, policy, worker, binding["account"], previous["model"], None,
                                      effort=previous.get("effort", ""))
        chosen = step_accounts.account_of(cfg.values, state, request.role, request.step)
        account = None if chosen == step_accounts.PROJECT_ACCOUNT else chosen
        effort = cfg.values["efforts"].get(request.role, "")
        return goose_lane.prepare(self._store, site, policy, worker, account, spec.model, None, effort=effort)

    def _goose_request(self, cfg, request, log, marks, prompt, lane, model, site, worker):
        if request.resume is None:
            route = lane.route
            account = route.account
            ref = {"kind": "goose", "id": account.id}
            with self._lock:
                taken = run_records.begin(self._store, log.stem, request.role, request.phase, "goose", route.model,
                                          route.effort, self._launcher.host, request.cwd, request.extra_env, take_steer=True,
                                          account=account.label, account_ref=ref, stored_in=str(account.config_dir),
                                          step=request.step or run_records.STEP_OF_ROLE.get(request.role, ""),
                                          acp_binding=lane.binding)
            prompt = _with_steer(prompt, [inbox.present(entry) for entry in taken])
        installed = lane.installation
        turn = acp.Turn(installed.agent_name, installed.adapter.version, prompt,
                        session_id=request.resume.session_id if request.resume else "",
                        mcp_servers=(model.mcp,), log_prompt=request.resume is not None,
                        model=lane.route.model, effort=lane.route.effort, configured_model=lane.configured_model,
                        on_accept=lambda: run_records.acknowledge_acp_prompt(self._store, log.stem),
                        permission=lambda message: goose_lane.permission(self._store, site, worker, model.tools, message))
        self._surface.run_started(self._store, log.stem, request.role)
        timeouts = cfg.values["timeouts"]
        return LaunchRequest(list(model.argv), model.cwd, model.env, log, timeouts[request.phase],
                             timeouts["idle_seconds"], self._store.stop_file,
                             on_gap=self._record_gap, on_poll=lambda: self._poll(None, log, request.role, marks), acp=turn)

    def _poll(self, live: live_input.LiveChannel | None, log_path: Path, role: str = "",
              marks: run_times.Marker | None = None) -> None:
        """One supervision tick: mark when the log grew, sweep the task's stacks, and for a role that takes steers live,
        give it the waiting ones."""
        if marks is not None:
            marks.tick()
        if log_path.stem in self._awaiting_replay and run_records.settle_pending(self._store, log_path.stem):
            self._awaiting_replay.discard(log_path.stem)
        stacks.sweep_store(self._store, self._config)
        if live is None:
            return
        live.observe(log_path)
        if live.is_open:
            if not inbox.is_live(self._store, log_path.stem):
                inbox.mark_live(self._store, log_path.stem, role)
            inbox.take_live(self._store, self._store.repo, live, log_path.stem, role)

    def _end_live(self, live: live_input.LiveChannel | None, log_path: Path) -> None:
        """The role's run ended: no steer reaches it any more, and one it was handed but never took up goes to the next role."""
        if live is None:
            return
        live.finish()
        inbox.clear_live(self._store, log_path.stem)
        live.observe(log_path)
        inbox.settle_uncertain(self._store, log_path.stem, set(live.uncertain_taken()))
        for command, entry in live.unconsumed():
            inbox.queue_again(self._store, command, entry)

    def _cage(self, cfg: FlowConfig, spec, request: RunRequest, run: str, git_folder: Path) -> cage.RunCage | None:
        """Write the cage of a run from the preset's cage policy; a `none` role has none."""
        if spec.provider not in CAGED_PROVIDERS:
            return None
        knobs = cfg.values["knobs"]
        site = self._site(cfg, spec, request, run, git_folder)
        try:
            return cage.prepare(site, cage.load_policy(knobs["cage"]), knobs["subagents"])
        except cage.CageError as error:
            raise roles.RoleRefused(str(error)) from error

    def _site(self, cfg, spec, request, run, git_folder) -> cage.RunSite:
        roots = tuple(dict.fromkeys((engine_home.root(), engine_home.root_for_trust(), engine_home.account_root())))
        worker_env = {}
        if spec.provider == "goose":
            names = (EXCHANGE_ENV, "SAKURA_FLOW_BASE_URL", "SAKURA_FLOW_PORTS", "SAKURA_FLOW_BUILD_TASK")
            worker_env = {key: request.extra_env[key] for key in names if key in request.extra_env}
            worker_env.update(SAKURA_FLOW_TASK=self._store.task_id, SAKURA_FLOW_RUN=run, SAKURA_FLOW_ROLE=request.role,
                              SAKURA_FLOW_PHASE=request.phase)
        return cage.RunSite(self._store.root, run, request.role, request.cwd, git_folder,
                            roots, self._store.repo, self._row_env(request, run), self._loopback_ports(cfg, request),
                            program_paths=_program_paths(spec), knowledge=_knowledge(cfg, request, git_folder),
                            worker_env=worker_env, **self._isolation(request))

    def _isolation(self, request: RunRequest) -> dict:
        """For a run in a repository of its own: a new tmp folder of its own, recorded in the state (and removed when
        the run ends, `_attempt`), and every folder of the task's other runs denied: its workspaces, its repositories,
        the tmp folder they share, and every role repository, worktree and tmp folder made for another isolated run,
        also once it is removed and forgotten (a detached survivor of that run can make it again and write there);
        nothing for any other run."""
        if request.repository is None:
            return {}
        own = cage.new_isolated_tmp_dir(self._store.root)
        state = self._store.update_state(lambda current: current.update(
            {ISOLATED_TMP_KEY: [*current.get(ISOLATED_TMP_KEY, []), str(own)]}))
        mine = {os.path.realpath(request.repository), os.path.realpath(request.cwd), str(own)}
        task = [folder for repository, workspaces in task_repository.recorded_repositories(state).items()
                for folder in (repository, *workspaces)]
        others = [*task, *state.get(task_repository.ROLE_PATHS_KEY, []), *state[ISOLATED_TMP_KEY]]
        denied = [Path(folder) for folder in dict.fromkeys(others) if folder not in mine]
        return {"tmp_dir": own, "denied_reads": (*denied, cage.task_tmp_dir(self._store.root))}

    @staticmethod
    def _loopback_ports(cfg: FlowConfig, request: RunRequest) -> tuple[int, ...]:
        """The loopback ports a role may reach: the application's (from its base URL), the port block the engine
        allocated to its workspace (a builder starts and tests its app there) and the host's approved ones."""
        port = urlparse(request.extra_env.get("SAKURA_FLOW_BASE_URL", "")).port
        block = _port_block(request.extra_env.get("SAKURA_FLOW_PORTS", ""))
        return tuple(dict.fromkeys((*checks.loopback_ports(cfg, port or 0), *block)))

    def _row_env(self, request: RunRequest, run: str) -> dict[str, str]:
        """What the evaluator's `run_row` tool needs to run a row as this run's task; no other role has the tool."""
        if request.role != EVALUATOR_ROLE:
            return {}
        return {"SAKURA_FLOW_RUN": run, "SAKURA_FLOW_TASK": self._store.task_id, "SAKURA_FLOW_ROLE": request.role,
                "SAKURA_FLOW_PHASE": request.phase, **request.extra_env,
                **task_repository.helper_environment(self._store, request.repository)}

    def _ingest_requests(self, run_cage: cage.RunCage, request: RunRequest) -> None:
        """Move what the cage refused during the run into the task state and the ledger, where the person sees it."""
        entries = cage.collect(run_cage)
        if not entries:
            return

        def change(state: dict) -> None:
            kept = [*state.get(owner_marks.REQUESTS_KEY, []), *entries]
            state[owner_marks.REQUESTS_KEY] = kept[-KEPT_REQUESTS:]

        self._store.update_state(change)
        for entry in entries:
            self._store.append_ledger(request.phase, f"cage denied a request: {entry['reason']}",
                                      agent_values=[entry["tool"], entry["detail"]])

    def _begin_run(self, name: str, cfg: FlowConfig, spec, route: routing.Route, request: RunRequest,
                   env: dict, account: dict | None = None) -> list[str]:
        """Write the `starting` record (naming the `account` the run launches under) and take the pending operator
        steers, so exactly one run delivers each."""
        dial = self._dial_for(cfg, route, request.role)
        is_agent = spec.provider in {"claude", "codex"}
        model = spec.model or (dial.claude_model if spec.provider == "claude" else "")
        with self._lock:
            taken = run_records.begin(self._store, name, request.role, request.phase, spec.provider, model,
                                      dial.effort if is_agent else "", self._launcher.host, request.cwd,
                                      env, take_steer=True, route=route.key, reason=route.reason,
                                      step=request.step or run_records.STEP_OF_ROLE.get(request.role, ""),
                                      account=account["label"] if account else None,
                                      account_ref={key: value for key, value in account.items() if key != "label"} if account else None)
            file_texts = [f"{STEER_HEADING}\n{text}" for text in self._take_steer_file()]
        for entry in taken:
            self._store.append_ledger("runner", f"steer delivered ({entry['id']})")
        return [inbox.present(entry) for entry in taken] + file_texts

    def _launch_recorded(self, name: str, launch: LaunchRequest, *,
                         release: Callable[[], None] | None = None) -> process.ProcessOutcome:
        """Launch and always finalize the record: a raised error or a run killed by the engine is `aborted`."""
        outcome = None
        try:
            cage.require_program(launch.argv)
            outcome = self._launcher.run(launch, lambda identity: self._record_started(name, identity))
            return outcome
        finally:
            try:
                if release:
                    release()
            except OSError:
                outcome = None
                raise
            finally:
                self._clear_active(name)
                exit_code = outcome.exit_code if outcome else None
                if launch.acp:
                    run_records.finish(self._store, name, exit_code, aborted=exit_code != 0,
                                       acp_failure=outcome.reason if outcome else "adapter_launch_interrupted")
                else:
                    run_records.finish(self._store, name, exit_code, aborted=exit_code is None)

    def _take_steer_file(self) -> list[str]:
        """Deliver the pre-mailbox `steer.txt` once more for compatibility, then move it away."""
        steer_file = self._store.steer_file
        text = steer_file.read_text().strip() if steer_file.exists() else ""
        if not text:
            return []
        self._store.runs_dir.mkdir(parents=True, exist_ok=True)
        sequence = len(list(self._store.runs_dir.glob("steer-*.txt"))) + 1
        steer_file.replace(self._store.runs_dir / f"steer-{sequence}.txt")
        self._store.append_ledger("runner", f"steer delivered (steer-{sequence}.txt)")
        return [text]

    def _record_gap(self, seconds: int) -> None:
        self._store.append_ledger("runner", f"wall-clock gap of {seconds} s")

    def _resume_log(self, name: str) -> Path:
        """The log of a run that continues a conversation: its record was made before it launched."""
        self._store.runs_dir.mkdir(parents=True, exist_ok=True)
        path = self._store.runs_dir / f"{name}.log"
        path.touch()
        return path

    def _next_log(self, phase: str) -> Path:
        self._store.runs_dir.mkdir(parents=True, exist_ok=True)
        with self._lock:
            own_logs = [log for log in self._store.runs_dir.glob(f"{phase}-*.log") if log.stem[len(phase) + 1:].isdigit()]
            sequence = len(own_logs) + 1
            path = self._store.runs_dir / f"{phase}-{sequence}.log"
            path.touch()
        return path

    def _require_hooks_ran(self, spec, request: RunRequest, log_path: Path, started: float) -> None:
        """Fail closed: a run that used a tool without one engine hook call since it started had no brakes."""
        if not roles.hooks_expected(spec):
            return
        if not roles.ran_tools(spec, _read_log(log_path)) or self._store.hook_ran_since(log_path.stem, started):
            return
        message = f"{spec.provider} hooks did not run ({log_path.stem})"
        self._store.append_ledger(request.phase, strip_controls(message))
        raise roles.RoleRefused(message)

    def _environment(self, request: RunRequest, run: str, cfg: FlowConfig, spec) -> dict:
        return self._launch_environment(request, run, cfg, spec)[0]

    def _launch_environment(self, request: RunRequest, run: str, cfg: FlowConfig, spec) -> tuple[dict, dict | None]:
        """The environment of a run and the account it runs under, `{"kind", "id", "label"}` (None: the default account)."""
        # No path into the engine home: a role reads and writes its exchange folder (`SAKURA_FLOW_EXCHANGE`) only.
        flow_env = {
            "SAKURA_FLOW_RUN": run,
            "SAKURA_FLOW_TASK": self._store.task_id,
            "SAKURA_FLOW_ROLE": request.role,
            "SAKURA_FLOW_PHASE": request.phase,
        }
        env = {**process.scrubbed_environment(os.environ, spec.provider), **interpreter.environment(), **flow_env,
               **request.extra_env, **task_repository.helper_environment(self._store, request.repository)}
        return self._with_account(env, cfg, spec, request)

    def _with_budget_account(self, env: dict, cfg: FlowConfig, spec, request: RunRequest) -> dict:
        return self._with_account(env, cfg, spec, request)[0]

    def _with_account(self, env: dict, cfg: FlowConfig, spec, request: RunRequest) -> tuple[dict, dict | None]:
        """Point a claude role at the `budget.account` config directory and a codex role at the paired Codex account's;
        other providers keep `env`. Also the account the role then runs under (`kind`, `id`, `label`), None for the default."""
        if (named := self._with_step_account(env, cfg, spec, request)) is not None:
            return named
        account = cfg.values["budget"]["account"]
        if spec.provider not in (budget_account.CLAUDE, budget_account.CODEX) or not account:
            return env, None
        try:
            usage = usage_settings.load()
            if spec.provider == budget_account.CODEX:
                return self._with_codex_account(env, account, usage.sources)
            return (budget_account.apply(env, account, usage.sources, usage.disabled),
                    _account_ref(resume_account.BUDGET, account, account))
        except (budget_account.UnknownAccountError, usage_settings.UsageConfigError) as error:
            self._store.append_ledger(request.phase, strip_controls(str(error)))
            raise roles.RoleRefused(str(error)) from error

    @staticmethod
    def _with_codex_account(env: dict, account: str, sources) -> tuple[dict, dict | None]:
        """The environment of a Codex role under the budget `account`, and the Codex account it actually runs on (the one
        paired with it); a role with none paired runs on the default account and says so (no reference)."""
        paired = budget_account.codex_pair(account, sources)
        if paired is None or paired.config_dir is None:
            return budget_account.apply_codex(env, account, sources), None
        ref = {**_account_ref(resume_account.BUDGET, account, paired.account), "codex": paired.account}
        return budget_account.apply_codex(env, account, sources), ref

    def _with_step_account(self, env: dict, cfg: FlowConfig, spec, request: RunRequest) -> tuple[dict, dict] | None:
        """The environment of the account the preset names for this step (`flow/step_accounts.py`) and that account's
        label; None when the step runs on the project's account. An account that cannot be used refuses the run, and the
        ledger says why."""
        state = self._store.read_state()
        if request.fallback_account is not None:
            row = request.fallback_account
            return (step_accounts.point_at(env, row["config_dir"], spec.provider),
                    _account_ref(resume_account.STEP, row["id"], row.get("label") or row["id"]))
        try:
            row = step_accounts.chosen_row(cfg.values, state, request.role, spec.provider, request.step)
            if row is None:
                return None
            return (step_accounts.apply(env, cfg.values, state, request.role, spec.provider, request.step),
                    _account_ref(resume_account.STEP, row["id"], row.get("label") or row["id"]))
        except step_accounts.StepAccountProblem as error:
            self._store.append_ledger(request.phase, strip_controls(str(error)))
            raise roles.RoleRefused(str(error)) from error

    def _record_started(self, name: str, identity: dict) -> None:
        run_records.mark_running(self._store, name, identity)
        active = {**identity, "run": name, "host": self._launcher.host}

        def change(state: dict) -> None:
            state["active_runs"] = [*state.get("active_runs", []), active]
            state["active_run"] = active

        self._store.update_state(change)

    def _clear_active(self, name: str) -> None:
        def change(state: dict) -> None:
            remaining = [run for run in state.get("active_runs", []) if run.get("run") != name]
            state["active_runs"] = remaining
            state["active_run"] = remaining[-1] if remaining else None

        self._store.update_state(change)


def _account_ref(kind: str, identity: str, label: str) -> dict:
    return {"kind": kind, "id": identity, "label": label}


def _program_paths(spec) -> tuple[str, ...]:
    """The existing absolute paths an exec role's host-configured command names; other roles name none."""
    if spec.provider != "exec":
        return ()
    return tuple(word for word in shlex.split(spec.command) if os.path.isabs(word) and os.path.exists(word))


def _port_block(text: str) -> tuple[int, ...]:
    """The ports of an allocated block written `first-last` (`SAKURA_FLOW_PORTS`); empty for anything else."""
    first, _, last = text.partition("-")
    if not (first.isdigit() and last.isdigit()) or not 0 < int(first) <= int(last) <= MAX_PORT:
        return ()
    return tuple(range(int(first), int(last) + 1))
