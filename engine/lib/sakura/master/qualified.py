"""Prepare a qualified Master launch and an immutable policy for its slot."""

from __future__ import annotations

import json
import os
from dataclasses import asdict, dataclass, replace
from pathlib import Path

from sakura import beneath, programs
from sakura.accounts import goose
from sakura.accounts.providers import GOOSE
from sakura.accounts.registry import Account
from sakura.flow import agent_accounts, cage, config, goose_cage, pinned_roots, presets
from sakura.master import history, spec
from sakura.setup import harnesses

MAX_FILE = 1024 * 1024
CODEX_MASTER_MODE = "permission-profile:sakura-work"
MASTER_SLOT_ENV = "SAKURA_MASTER_SLOT"
MASTER_LAUNCH_ENV = "SAKURA_MASTER_LAUNCH"
NATIVE_PROOF = "master_release_proof_required"


@dataclass(frozen=True)
class Selection:
    """The account and exact adapter installation selected for this launch."""

    account: Account
    binding: history.Binding
    installed: harnesses.Installation
    model: str
    effort: str
    configured_model: str = ""

    @property
    def harness(self) -> str:
        return self.binding.harness

    @property
    def artifact(self) -> dict:
        installed = self.installed
        return {name: asdict(program) for name in ("adapter", "native", "runtime")
                if (program := getattr(installed, name)) is not None}


@dataclass(frozen=True)
class Files:
    """The per-launch cage, settings and grant digest."""

    site: cage.RunSite
    cage_policy: cage.Policy
    worker: cage.RunCage
    tools: tuple[str, ...]
    scope: str


def prepare(home: Path, preset: presets.Preset, *, harness: str, account_id: str | None, slot: str,
            model: str = "", effort: str = "", fresh: bool = False) -> dict:
    """Issue a slot-bound Master descriptor or a typed readiness reason."""
    response = {"protocol_version": harnesses.ACP_VERSION, "harness": harness, "slot": slot, "argv": []}
    try:
        if not history.SLOT.fullmatch(slot):
            raise history.HistoryError("master_invalid_slot")
        _check_choices(model, effort)
        selected = select(harness, account_id, model, effort)
        launch = history.launch_id()
        files = prepare_files(home, preset, slot, launch, harness)
        parameters = _parameters(selected, preset, files)
        row = history.issue(home, slot, selected.binding, files.scope, fresh=fresh, launch=launch, parameters=parameters)
    except (history.HistoryError, programs.ProgramError) as error:
        return {**response, "readiness": "unavailable", "reason": str(error)}
    except (OSError, cage.CageError, pinned_roots.RootReplaced, ValueError):
        return {**response, "readiness": "unavailable", "reason": "master_policy_unavailable"}
    return {**response, **_descriptor(selected, files, slot, launch, row)}


def select(harness: str, account_id: str | None, model: str, effort: str) -> Selection:
    """Verify account backing, installation and provider prerequisites."""
    account = account_for(harness, account_id)
    installed = harnesses.qualify(harness)
    if not installed.master_safe:
        raise programs.ProgramError(NATIVE_PROOF)
    configuration = None
    configured = ""
    if harness == GOOSE:
        _, configured = goose_cage.qualify(installed, account.goose_provider, "")
        configuration = goose.lane_identity(account)
        if not configuration:
            raise programs.ProgramError("goose_account_configuration_changed")
        model = model or configured
    binding = history.Binding.for_account(harness, account.id, account.config_dir, configuration=configuration)
    return Selection(account, binding, installed, model, effort, configured)


def account_for(harness: str, account_id: str | None) -> Account:
    """Resolve a Master account only within the requested harness."""
    if harness == GOOSE:
        return goose.lane_account(account_id)
    if harness not in ("claude", "codex"):
        raise programs.ProgramError("unsupported_harness")
    candidates = [row for row in agent_accounts.usable() if row.enabled and row.provider == harness]
    exact = [row for row in candidates if row.id == account_id] if account_id else candidates
    selected = exact or [row for row in candidates if row.label == account_id]
    if len(selected) != 1:
        raise programs.ProgramError("master_account_unavailable")
    return selected[0]


def legacy_binding(harness: str, account_id: str | None) -> history.Binding:
    """Resolve transcript storage independently of adapter qualification."""
    account = account_for(harness, account_id)
    return history.Binding.for_account(harness, account.id, account.config_dir)


def _check_choices(model: str, effort: str) -> None:
    if model and (not model.isprintable() or len(model) > 256):
        raise history.HistoryError("master_invalid_model")
    if effort and not config.EFFORT.fullmatch(effort):
        raise history.HistoryError("master_invalid_effort")


def prepare_files(home: Path, preset: presets.Preset, slot: str, launch: str, harness: str) -> Files:
    """Pin the Master workspace and generate settings and MCP grants unique to this launch."""
    folder, temporary = spec._folders(home)
    tmp = pinned_roots.pin(pinned_roots.make_folder(temporary, launch))
    values = config.load(None).values
    account_home = cage.account_home()
    knowledge = spec._knowledge(values, account_home, folder)
    instructions = home / "master" / "cage" / launch / spec.INSTRUCTIONS_NAME
    site = spec._site(home, folder, tmp, instructions, knowledge, os.environ)
    site = replace(site, run=launch, server_env={**site.server_env, MASTER_SLOT_ENV: slot, MASTER_LAUNCH_ENV: launch})
    policy = spec._policy(preset, account_home)
    worker = cage.prepare(site, policy, preset.subagents, account_home)
    if not instructions.exists():
        if harness == GOOSE:
            source = spec.BASE_INSTRUCTIONS.read_text() + (
                "\n## Access\n\nUse the engine's Master tools to inspect projects and lanes, read and post to boards, "
                "and propose knowledge or scheduled work. You have no direct filesystem or shell tools.\n"
            )
            source += spec._time_zone_section() + spec._host_instructions(home)
        else:
            source = spec._instructions_text(home, folder, knowledge)
        beneath.write_bytes(worker.folder, spec.INSTRUCTIONS_NAME, source.encode(), spec.INSTRUCTIONS_MODE)
    served = read_launch_file(home, launch, "policy.json")
    scope = _scope(policy, site, preset, served)
    return Files(site, policy, worker, tuple(served["tools"]), scope)


def _scope(policy: cage.Policy, site: cage.RunSite, preset: presets.Preset, served: dict) -> str:
    roots = [(str(path), path.stat().st_dev, path.stat().st_ino) for path in (site.workspace, *(site.read_roots or ()))]
    grants = {key: served.get(key) for key in ("role", "tools", "connectors")}
    return history.digest([asdict(policy), roots, site.knowledge, preset.subagents, grants])


def read_launch_file(home: Path, launch: str, name: str) -> dict:
    """Read the exact engine-owned launch's bounded settings or policy."""
    if not history.LAUNCH.fullmatch(launch) or name not in ("settings.json", "policy.json", "mcp.json"):
        raise history.HistoryError("master_launch_changed")
    return json.loads(beneath.read_capped(home, f"master/cage/{launch}/{name}", MAX_FILE))


def instructions_text(home: Path, launch: str) -> str:
    """Read the instructions captured when this launch was issued."""
    return beneath.read_capped(home, f"master/cage/{launch}/{spec.INSTRUCTIONS_NAME}", MAX_FILE).decode()


def _parameters(selected: Selection, preset: presets.Preset, files: Files) -> dict:
    return {"artifact": history.digest(selected.artifact), "model": selected.model, "effort": selected.effort,
            "preset": preset.name, "cage": preset.cage, "subagents": preset.subagents,
            "cwd": str(files.site.workspace), "tools": list(files.tools)}


def _descriptor(selected: Selection, files: Files, slot: str, launch: str, row: dict) -> dict:
    harness = selected.harness
    requirements = {"load_session": True, "mcp_transport": "http" if harness == GOOSE else "stdio"}
    instruction = "session_metadata" if harness == "claude" else "system_prompt_or_first_prompt"
    argv = [str(cage.KIT_BIN), "master", "acp", "--json", "--slot", slot, "--launch-id", launch]
    versions = {name: program["version"] for name, program in selected.artifact.items()}
    choices = {"model": selected.model, "effort": selected.effort, "mode": mode_for(harness)}
    return {"readiness": "ready", "reason": None, "argv": argv, "cwd": str(files.site.workspace),
            "env": spec._host_env(os.environ), "env_allow": [], "launch_id": launch, "binding": selected.binding.document(),
            "start_choices": choices, "mcp_tools": list(files.tools), "versions": versions, "requires": requirements,
            "instruction_delivery": instruction, "conversation": history.public(row) if row["session_id"] else None}


def mode_for(harness: str) -> str:
    """Require the qualified subscription policy mode before any prompt."""
    return {"claude": spec.START_MODE, "codex": CODEX_MASTER_MODE}.get(harness, "")
