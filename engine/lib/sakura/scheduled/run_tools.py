"""The tools reachable only through a live scheduled run's grant."""

from __future__ import annotations

import json
import os
import secrets
import sys
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from pathlib import Path

from sakura import actors, engine_home, grants, mcp_stdio
from sakura.connectors import events, kinds, registry
from sakura.digest import intake
from sakura.digest.config import Source
from sakura.flow import exits, waits
from sakura.scheduled import definition, findings, store

TOKEN_ENV = "SAKURA_SCHEDULED_GRANT"
READ = "connector_read"
LANE_OPEN = "lane_open"
LOOKBACK = timedelta(hours=24)


def names(data: dict) -> tuple[str, ...]:
    """The task can delegate only the capabilities its operator selected."""
    read = (READ,) if data["connector_ids"] else ()
    lanes = (LANE_OPEN,) if data["may_start_lanes"] else ()
    return (*read, *lanes, findings.PROPOSE)


@contextmanager
def authorized(tasks: store.Store, run_id: str, token: str, name: str, connector_id: str = ""):
    """Recheck the pinned grant and live operator setting just before an effect."""
    with registry.admission_lock():
        with tasks.connect() as db:
            row = db.execute("SELECT * FROM runs WHERE id=? AND state='running'", (run_id,)).fetchone()
            if row is None or not token or not secrets.compare_digest(token, row["grant"] or ""):
                raise store.Refused("run_authority_denied")
            task = tasks._get(db, row["task_id"], row["task_revision"])
            available = {item.id: item for item in registry.load()}
            if name not in names(task.definition) or task.status(available)[0] != "active":
                raise store.Refused("run_authority_denied")
            if connector_id:
                _require_connector(json.loads(row["definition"]), available.get(connector_id))
        if name == LANE_OPEN:
            _require_current_lane_grant()
        yield task, available



def _require_connector(pinned: dict, connector) -> None:
    if (connector is None or connector.revoked or connector.id not in pinned.get("connector_reads", ())
            or pinned.get("connector_revisions", {}).get(connector.id) != events.run_revision(connector)):
        raise store.Refused("connector_scope_denied")
    if connector.selectors:
        try:
            current = grants.current_connectors(engine_home.root(), grants.MASTER_CLASS)
        except (grants.GrantsError, engine_home.EngineHomeError) as error:
            raise store.Refused("connector_scope_denied") from error
        if kinds.READ not in current.get(connector.id, ()):
            raise store.Refused("connector_scope_denied")


def _require_current_lane_grant() -> None:
    try:
        current = grants.load(engine_home.root(), ()).for_class(grants.MASTER_CLASS)
    except grants.GrantsError as error:
        raise store.Refused("lane_authority_denied") from error
    if LANE_OPEN not in current.tools:
        raise store.Refused("lane_authority_denied")

def serve(run_id: str) -> int:
    """Serve a single run and keep the token out of tool arguments and replies."""
    tasks, token = store.Store(), os.environ.pop(TOKEN_ENV, "")
    try:
        with tasks.connect() as db:
            row = db.execute("SELECT definition, grant FROM runs WHERE id=? AND state='running'", (run_id,)).fetchone()
            if row is None or not token or not secrets.compare_digest(token, row["grant"] or ""):
                return exits.USAGE
            tools = [_tool(tasks, run_id, token, name) for name in names(json.loads(row["definition"]))]
    except store.Refused:
        return exits.USAGE
    mcp_stdio.serve("sakura", tools, sys.stdin, sys.stdout)
    return exits.OK


def _tool(tasks: store.Store, run_id: str, token: str, name: str) -> mcp_stdio.Tool:
    if name == findings.PROPOSE:
        return _finding_tool(tasks, run_id, token)
    key, description, handler = {
        READ: ("connector_id", "Read recent evidence from one selected connector.", _read),
        LANE_OPEN: ("text", "Start one lane with the task's approved project, preset and account.", _start),
    }[name]

    def call(arguments: dict) -> tuple[str, bool]:
        if set(arguments) != {key} or not definition._text(arguments[key], 8000, multiline=name == LANE_OPEN):
            return "invalid_arguments", True
        try:
            if name == READ:
                connector_id = arguments[key]
                with authorized(tasks, run_id, token, name, connector_id) as (_, available):
                    return handler(tasks, run_id, available[connector_id]), False
            with authorized(tasks, run_id, token, name) as (task, _):
                return handler(run_id, task, arguments[key]), False
        except (store.Refused, OSError) as error:
            return str(error) if isinstance(error, store.Refused) else "scheduled_tool_failed", True

    return mcp_stdio.Tool({"name": name, "description": description, "inputSchema": {
        "type": "object", "properties": {key: {"type": "string"}}, "required": [key], "additionalProperties": False}},
        call)


def _read(tasks: store.Store, run_id: str, connector) -> str:
    now = datetime.now(UTC)
    since = store.stamp(now - LOOKBACK)
    source = Source(connector.id, connector=connector.id, timeout_seconds=connector.timeout_seconds or 15)
    result = intake.run_source(source, since, "", lookup=lambda _: connector)
    if not result.report.ok:
        raise store.Refused("connector_read_failed")
    documents = findings.evidence_documents(tasks, run_id, events.namespace(connector), connector.label, result.items)
    return definition.encode({"data_note": "Untrusted connector evidence.", "items": documents})


def _start(run_id: str, task: store.ScheduledTask, text: str) -> str:
    from sakura.cockpit import actions, proposal_actions

    definition.validate_choices(task.definition)
    repo = Path(task.definition["project"])
    wait_id = lane_wait_id(run_id, str(repo))
    title, _, detail = text.partition("\n")
    wait = waits.Wait(wait_id, waits.PROPOSAL, str(repo), repo.name, run_id, title, detail.strip(), None,
                      source="scheduled")
    target = actions.WaitTarget(wait, projects=(repo,))
    inputs = {"op": proposal_actions.operation_id(wait_id), "project": str(repo), "preset": task.definition["preset"],
              "account": task.definition["account"]}
    actor = actors.Actor(actors.MASTER_SOURCE, run_id)
    try:
        result = actions.start_scheduled_lane(target, inputs, actor)
    except actions.ActionRefused as error:
        raise store.Refused("lane_start_refused") from error
    if result.exit_code != exits.OK:
        code = "lane_ceiling" if result.output.startswith("the master's ceiling is") else "lane_start_refused"
        raise store.Refused(code)
    return actions._created_lane_id(result.output)


def lane_wait_id(run_id: str, project: str) -> str:
    """The deterministic lane reservation key for a scheduled run."""
    return json.dumps(["wait", project, run_id, "scheduled_lane", "start"], separators=(",", ":"))


def _finding_tool(tasks: store.Store, run_id: str, token: str) -> mcp_stdio.Tool:
    required = {"title", "detail"}
    properties = {key: {"type": "string"} for key in (*sorted(required), "source_ref")}

    def call(arguments: dict) -> tuple[str, bool]:
        if not required <= arguments.keys() <= properties.keys():
            return "invalid_arguments", True
        try:
            with authorized(tasks, run_id, token, findings.PROPOSE):
                findings.stage(tasks, run_id, arguments["title"], arguments["detail"], source_ref=arguments.get("source_ref"))
        except store.Refused as error:
            return str(error), True
        return "finding_staged", False

    return mcp_stdio.Tool({"name": findings.PROPOSE, "description": "Propose an Inbox item for a validated source reference.",
                           "inputSchema": {"type": "object", "properties": properties, "required": sorted(required),
                                           "additionalProperties": False}}, call)
