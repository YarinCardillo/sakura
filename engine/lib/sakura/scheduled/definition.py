"""Validate scheduled definitions at the engine boundary."""

from __future__ import annotations

import json
import re
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from sakura.accounts import registry as accounts
from sakura.connectors import events
from sakura.connectors import registry as connectors
from sakura.flow import presets, registry
from sakura.flow.render import strip_controls
from sakura.usage import leeway

MAX_DEFINITION = 32_768
MAX_PROMPT = 16_000
MAX_CONNECTORS = 32
MAX_TITLE = 120
TASK_KIND = "scheduled"
CREATE, UPDATE, PAUSE, RESUME, DELETE, RUN = (
    "scheduledcreate", "scheduledupdate", "scheduledpause", "scheduledresume", "scheduleddelete", "scheduledrun",
)
ACTIONS = frozenset({CREATE, UPDATE, PAUSE, RESUME, DELETE, RUN})
OPTIONAL = {"may_start_lanes": False, "enabled": True, "project": None, "preset": None, "account": None}
REQUIRED = frozenset({"title", "prompt", "connector_ids", "when"})
TIME = re.compile(r"(?:[01][0-9]|2[0-3]):[0-5][0-9]")
OP = re.compile(r"[a-zA-Z0-9_-]{1,128}")


class Refused(ValueError):
    """A fixed client-visible refusal code."""


def encode(value: object) -> str:
    """Serialize durable engine data with deterministic field order."""
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"), allow_nan=False)


def decode(raw: str) -> dict:
    """Decode a bounded definition, rejecting duplicate fields."""
    try:
        if not isinstance(raw, str) or len(raw.encode()) > MAX_DEFINITION:
            raise ValueError
        return json.loads(raw, object_pairs_hook=_unique)
    except (ValueError, RecursionError):
        raise Refused("invalid_definition") from None


def _unique(pairs: list[tuple]) -> dict:
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError
    return result


def _text(value: object, maximum: int, *, multiline: bool = False) -> bool:
    return (isinstance(value, str) and 0 < len(value) <= maximum and bool(value.strip())
            and strip_controls(value) == value and "\x00" not in value
            and (multiline or not any(mark in value for mark in "\r\n\t")))


def validate(value: object, *, proposed: bool = False, choices: bool = True) -> dict:
    """Normalize the complete user definition and validate its configured references."""
    if not isinstance(value, dict) or not REQUIRED <= value.keys() <= (REQUIRED | OPTIONAL.keys()):
        raise Refused("invalid_definition")
    data = {**OPTIONAL, **value}
    if not _text(data["title"], MAX_TITLE) or not _text(data["prompt"], MAX_PROMPT, multiline=True):
        raise Refused("invalid_definition")
    if any(type(data[key]) is not bool for key in ("enabled", "may_start_lanes")):
        raise Refused("invalid_definition")
    if proposed and data["may_start_lanes"]:
        raise Refused("lane_authority_denied")
    ids = data["connector_ids"]
    if (not isinstance(ids, list) or len(ids) > MAX_CONNECTORS
            or not all(_text(item, 128) for item in ids) or len(set(ids)) != len(ids)):
        raise Refused("invalid_definition")
    _when(data["when"])
    try:
        size = len(encode(data))
    except (ValueError, TypeError, RecursionError):
        raise Refused("invalid_definition") from None
    if size > MAX_DEFINITION:
        raise Refused("invalid_definition")
    if choices:
        validate_choices(data)
    return data


def _when(when: object) -> None:
    if not isinstance(when, dict):
        raise Refused("invalid_when")
    kind = when.get("kind")
    if (kind == "interval" and set(when) == {"kind", "minutes"}
            and type(when["minutes"]) is int and 5 <= when["minutes"] <= 1440):
        return
    if (kind == "connector_event" and set(when) == {"kind", "connector_id", "event_kind"}
            and all(_text(when[key], 128) for key in ("connector_id", "event_kind"))):
        return
    if (kind == "daily" and set(when) == {"kind", "time", "timezone"} and _text(when["time"], 5)
            and TIME.fullmatch(when["time"]) and _text(when["timezone"], 128)):
        try:
            ZoneInfo(when["timezone"])
            return
        except (ValueError, ZoneInfoNotFoundError):
            pass
    raise Refused("invalid_when")


def required_connectors(data: dict) -> tuple[str, ...]:
    """Include the event dependency even if it is not a selected tool."""
    return tuple(dict.fromkeys([*data["connector_ids"], *([data["when"]["connector_id"]]
                                                        if data["when"]["kind"] == "connector_event" else [])]))


def event_kinds(connector: connectors.Connector) -> tuple[str, ...]:
    """Event capabilities advertised by the connector adapter."""
    return events.capabilities(connector)


def validate_choices(data: dict) -> None:
    """Recheck stored references before accepting an approval or resuming work."""
    try:
        available = {item.id: item for item in connectors.load()} if required_connectors(data) else {}
    except connectors.ConnectorError as error:
        raise Refused("connector_registry_unavailable") from error
    if any(key not in available for key in required_connectors(data)):
        raise Refused("unknown_connector")
    when = data["when"]
    if when["kind"] == "connector_event" and when["event_kind"] not in event_kinds(available[when["connector_id"]]):
        raise Refused("unsupported_event")
    _validate_lane_choices(data)


def _validate_lane_choices(data: dict) -> None:
    project, preset, account = (data[key] for key in ("project", "preset", "account"))
    if project is not None and (not _text(project, 1024) or not _known_project(project)):
        raise Refused("project_unavailable")
    if preset is not None and (not _text(preset, 128) or preset not in presets.names()):
        raise Refused("preset_unavailable")
    if account is not None and (not _text(account, 128) or not _known_account(account)):
        raise Refused("account_unavailable")
    if data["may_start_lanes"] and None in (project, preset, account):
        raise Refused("lane_defaults_required")




def _known_project(project: str) -> bool:
    try:
        return Path(project) in registry.repos(registry.default_path())
    except registry.RegistryError:
        return False

def _known_account(account: str) -> bool:
    try:
        configured = accounts.load_stored(accounts.default_path())
        if configured and any(entry.id == account and entry.enabled for entry in configured.accounts):
            return True
        return any(row.get("id") == account and row.get("enabled", True) and row.get("config_dir")
                   for row in leeway.accounts())
    except (accounts.AccountsConfigError, leeway.LeewayError, OSError):
        return False

def effective(data: dict, available: dict[str, connectors.Connector]) -> tuple[str, str | None, str | None]:
    """Return the engine state, fixed reason and unavailable connector label."""
    for key in required_connectors(data):
        connector = available.get(key)
        if connector is None or connector.revoked or not connector.connected:
            return "connector_unavailable", "connector_unavailable", connector.label if connector else None
    trigger = data["when"]
    if trigger["kind"] == "connector_event":
        connector = available[trigger["connector_id"]]
        if trigger["event_kind"] not in event_kinds(connector):
            return "connector_unavailable", "unsupported_event", connector.label
    return ("active", None, None) if data["enabled"] else ("paused", "operator_paused", None)
