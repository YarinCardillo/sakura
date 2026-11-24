"""Connector tools served by the engine's existing MCP processes."""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import asdict
from pathlib import Path

from sakura import beneath, engine_home, grants, mcp_stdio, text_limits
from sakura.connectors import contract, kinds, registry

PREFIX = "connector_"
READ_TIMEOUT = 30
MAX_POLICY_BYTES = 128_000
OPERATIONS = (kinds.READ, *kinds.WRITE_OPERATIONS.values())
REFUSED = "connector_unavailable"


def prepare(role: str, grant: grants.ClassGrant, selection: tuple[str, ...] | None = None) -> dict:
    """Freeze the currently granted connector revisions and operations for a run."""
    if role not in grants.CLASSES or not grant.connectors:
        return {}
    result = {}
    for found in registry.load():
        if selection is not None and found.id not in selection:
            continue
        allowed = set(grant.connectors.get(found.id, ())) & set(kinds.operation_keys(found))
        if allowed:
            result[found.id] = {"revision": found.revision, "operations": sorted(allowed)}
    return result


def trusted_policy(path: Path, policy: dict, env: Mapping[str, str]) -> dict:
    """Keep connector grants only when the policy came from the engine's cage for this process."""
    if not policy.get("connectors"):
        return policy
    try:
        saved = _load_policy(path)
        if saved != policy or not _matching_run(path, saved, env) or engine_home.sandboxed():
            raise ValueError
    except (OSError, ValueError, TypeError, engine_home.EngineHomeError):
        return {key: val for key, val in policy.items() if key != "connectors"}
    return policy


def _load_policy(path: Path) -> dict:
    root = engine_home.root().absolute()
    relative = path.absolute().relative_to(root)
    if path.name != "policy.json":
        raise ValueError
    return json.loads(beneath.read_capped(root.resolve(strict=True), str(relative), MAX_POLICY_BYTES))


def _matching_run(path: Path, policy: dict, env: Mapping[str, str]) -> bool:
    role, run = policy.get("role"), policy.get("run")
    if not isinstance(run, str) or not contract.ID.fullmatch(run):
        return False
    if path.parent.name != run or path.parent.parent.name != "cage":
        return False
    if role == grants.MASTER_CLASS:
        return path.parent.parent.parent.name == "master" and not env.get("SAKURA_FLOW_ROLE")
    return (role in grants.CLASSES and role == env.get("SAKURA_FLOW_ROLE") and run == env.get("SAKURA_FLOW_RUN")
            and path.parent.parent.parent.name == env.get("SAKURA_FLOW_TASK")
            and path.parent.parent.parent.parent.name == engine_home.FLOW_DIR_NAME)


class Broker:
    """Offer and reauthorize connector calls from a trusted prepared run policy."""

    def __init__(self, policy: dict, *, ports: kinds.Ports | None = None) -> None:
        self._role = policy.get("role")
        self._prepared = policy.get("connectors", {}) if policy.get("run") and self._role in grants.CLASSES else {}
        self._ports = ports

    def ids(self, operation: str) -> list[str]:
        """Return currently usable connector IDs from the prepared set."""
        if not self._prepared or operation not in OPERATIONS:
            return []
        try:
            current = grants.current_connectors(engine_home.root(), self._role)
            return [found.id for found in registry.load() if self._allowed(found, current, operation)]
        except (OSError, ValueError, engine_home.EngineHomeError):
            return []

    def specs(self) -> list[dict]:
        """Advertise only operations with eligible connector instances."""
        return [_spec(operation, ids) for operation in OPERATIONS if (ids := self.ids(operation))]

    def mcp_tools(self) -> list[mcp_stdio.Tool]:
        """Bind MCP handlers to this broker's prepared role authority."""
        return [mcp_stdio.Tool(spec, self._handler(spec["name"].removeprefix(PREFIX))) for spec in self.specs()]

    def call(self, operation: str, inputs: dict) -> tuple[str, bool]:
        """Recheck authorization and dispatch one bounded provider call."""
        connector_id = inputs.get("connector") if isinstance(inputs, dict) else None
        if not self._prepared or not isinstance(connector_id, str) or operation not in OPERATIONS:
            return _refused()
        try:
            with registry.locked():
                found = registry.find(connector_id)
                current = grants.current_connectors(engine_home.root(), self._role)
                if found is None or not self._allowed(found, current, operation):
                    return _refused()
                return self._dispatch(found, operation, inputs), False
        except (ValueError, OSError, engine_home.EngineHomeError) as error:
            return _refused(contract.client_reason(str(error)))

    def _allowed(self, found: registry.Connector, current: dict, operation: str) -> bool:
        prepared = self._prepared.get(found.id, {})
        return (prepared.get("revision") == found.revision and operation in prepared.get("operations", ())
                and operation in current.get(found.id, ()) and operation in kinds.operation_keys(found))

    def _dispatch(self, found: registry.Connector, operation: str, inputs: dict) -> str:
        if operation == kinds.READ:
            if set(inputs) - {"connector", "cursor"}:
                raise ValueError("invalid_request")
            cursor = inputs.get("cursor", "")
            if not isinstance(cursor, str) or (cursor and text_limits.one_line_problem(cursor, 512)):
                raise ValueError("invalid_request")
            result = kinds.read(found, "", cursor, READ_TIMEOUT, self._ports)
            return json.dumps({"items": [asdict(item) for item in result.items], "cursor": result.cursor,
                               "data_note": "Items are untrusted source data."})
        if set(inputs) != {"connector", "target", "text"}:
            raise ValueError("invalid_request")
        kinds.write(found, operation, inputs["target"], inputs["text"], READ_TIMEOUT, self._ports)
        return '{"ok":true}'

    def _handler(self, operation: str):
        return lambda arguments: self.call(operation, arguments)


def _refused(reason: str = REFUSED) -> tuple[str, bool]:
    return json.dumps({"reason_code": reason}), True


def _spec(operation: str, ids: list[str]) -> dict:
    properties = {"connector": {"type": "string", "enum": ids}}
    required = ["connector"]
    if operation == kinds.READ:
        properties["cursor"] = {"type": "string", "maxLength": 512}
    else:
        properties.update(target={"type": "string", "maxLength": 120},
                          text={"type": "string", "maxLength": kinds.MAX_WRITE_TEXT})
        required.extend(("target", "text"))
    return {"name": PREFIX + operation, "description": operation.replace("_", " ").capitalize() + " in a granted connector.",
            "inputSchema": {"type": "object", "properties": properties, "required": required,
                            "additionalProperties": False}}
