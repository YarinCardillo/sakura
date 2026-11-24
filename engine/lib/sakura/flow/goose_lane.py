"""Resolve a Goose lane and verify the binding before continuing its conversation."""

from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass
from pathlib import Path

from sakura import programs
from sakura.accounts import goose
from sakura.budget import routing
from sakura.flow import cage, cage_server, goose_cage, roles, run_records
from sakura.flow.store import TaskStore
from sakura.setup import harnesses


@dataclass(frozen=True)
class Lane:
    """A qualified account route, its storage and the data that must match on continuation."""

    route: routing.GooseRoute
    installation: harnesses.Installation
    provider: goose_cage.Provider
    root: Path
    binding: dict
    configured_model: str

    def recheck(self, store: TaskStore, site: cage.RunSite) -> None:
        """Recheck trusted executable and pinned storage immediately before launching."""
        self.installation.recheck(site.workspace)
        goose_cage.model_storage(_parent(store), self.binding["storage_run"], self.binding["storage_identity"])
        account = goose.lane_account(self.route.account.id)
        if goose.lane_identity(account) != self.binding["configuration"]:
            raise cage.CageError("goose_account_configuration_changed")
        if workspace_identity(site.workspace) != self.binding["workspace"]:
            raise cage.CageError("goose_workspace_changed")


def prepare(store: TaskStore, site: cage.RunSite, policy: cage.Policy, worker: cage.RunCage, account_id: str | None,
            model: str, resumed: dict | None, *, effort: str = "") -> Lane:
    """Pin a new conversation or require the precise route, workspace, policy and storage already recorded."""
    route = routing.goose_route(account_id, model)
    installed = harnesses.qualify("goose", site.workspace)
    provider, model = goose_cage.qualify(installed, route.provider, route.model)
    configured = dict(installed.lane_models)[route.provider]
    route = routing.GooseRoute(route.account, route.provider, model, effort)
    expected = _binding(route, installed, site, policy, worker)
    saved = resumed.get("acp_binding") if resumed else None
    if resumed and (not isinstance(saved, dict) or any(saved.get(key) != value for key, value in expected.items())):
        raise cage.CageError("goose_conversation_binding_changed")
    name, identity = _storage_binding(site, saved)
    root, identity = goose_cage.model_storage(_parent(store), name, identity)
    return Lane(route, installed, provider, root, {**expected, "storage_run": name, "storage_identity": identity}, configured)


def _binding(route, installed, site, policy, worker) -> dict:
    configuration = goose.lane_identity(route.account)
    if not configuration:
        raise cage.CageError("goose_account_configuration_changed")
    scope = _scope(site, policy, worker)
    artifact = installed.adapter
    return {"harness": "goose", "account": route.account.id, "configuration": configuration,
            "provider": route.provider, "model": route.model, **({"effort": route.effort} if route.effort else {}),
            "role": site.role,
            "workspace": workspace_identity(site.workspace), "policy": scope,
            "artifact": [artifact.path, artifact.version, list(artifact.identity)]}


def _scope(site, policy, worker) -> str:
    served = json.loads(worker.policy.read_text())
    grants = sorted(served.get("tools", []))
    document = [asdict(policy), site.loopback_ports, site.local_sockets, site.knowledge, grants,
                str(site.common_dir), str(site.repo)]
    return hashlib.sha256(json.dumps(document, sort_keys=True).encode()).hexdigest()


def _storage_binding(site: cage.RunSite, saved: dict | None) -> tuple[str, list[int] | None]:
    if saved is None:
        return site.run, None
    name, identity = saved.get("storage_run"), saved.get("storage_identity")
    if not isinstance(name, str) or not run_records.RUN_NAME.fullmatch(name) or not isinstance(identity, list)             or len(identity) != 2 or any(type(value) is not int for value in identity):
        raise cage.CageError("session_storage_changed")
    return name, identity


def _parent(store: TaskStore) -> Path:
    return store.root.resolve() / cage.CAGE_DIR / "goose-sessions"


def workspace_identity(workspace: Path) -> list:
    """Bind a conversation to the original directory, including replacement at the same path."""
    if workspace.resolve() != workspace or not workspace.is_dir():
        raise cage.CageError("goose_workspace_changed")
    metadata = workspace.stat()
    return [str(workspace), metadata.st_dev, metadata.st_ino]


def resume_problem(store: TaskStore, record: dict, workspace: Path) -> str:
    """Check saved workspace and model storage without starting an adapter."""
    binding = record.get("acp_binding")
    if not isinstance(binding, dict):
        return "The Goose conversation has no verified binding. Start a fresh lane attempt."
    failure = record.get("acp_failure")
    if failure in ("session_load_refused", "session_load_unsupported"):
        return "The Goose adapter cannot load this conversation. Start a fresh lane attempt."
    if failure in ("goose_conversation_binding_changed", "goose_account_configuration_changed", "session_storage_changed",
                   "goose_workspace_changed"):
        return "The Goose conversation binding changed. Start a fresh lane attempt."
    try:
        if workspace_identity(workspace) != binding.get("workspace"):
            return "The Goose workspace changed. Start a fresh lane attempt."
        name = binding.get("storage_run")
        identity = binding.get("storage_identity")
        if not isinstance(name, str) or not run_records.RUN_NAME.fullmatch(name) or not isinstance(identity, list):
            return "The Goose conversation storage is unavailable. Start a fresh lane attempt."
        goose_cage.model_storage(_parent(store), name, identity)
    except (OSError, cage.CageError):
        return "The Goose conversation storage is unavailable. Start a fresh lane attempt."
    return ""


def permission(store: TaskStore, site: cage.RunSite, worker: cage.RunCage, names: frozenset[str], message: dict) -> bool:
    """Expose the transient permission check and decide within the worker's existing grants."""
    run_records.set_acp_permission(store, site.run, True)
    try:
        if store.stop_file.exists():
            return False
        call = message["params"].get("toolCall")
        name = call.get("title") if isinstance(call, dict) else None
        if isinstance(name, str) and name in names:
            return True
        cage_server._record({"run": site.run, "role": site.role, "requests": str(worker.requests)},
                            "ACP permission", "the tool is outside the run's grants")
        return False
    finally:
        run_records.set_acp_permission(store, site.run, False)


def startup_error(error: Exception) -> roles.RoleRefused:
    """Return a safe launch refusal even when the OS error includes environment details."""
    if isinstance(error, (cage.CageError, programs.ProgramError)):
        return roles.RoleRefused(str(error))
    return roles.RoleRefused("Goose confinement could not be prepared")
