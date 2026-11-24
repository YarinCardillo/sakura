"""Validation and safe result values for the cockpit connector protocol."""

from __future__ import annotations

import json
import re

from sakura import text_limits
from sakura.digest import config

VERSION = 1
ACTIONS = frozenset({"connectoradd", "connectoredit", "connectordisconnect", "connectorreplace"})
ID = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}")
REVISION = re.compile(r"[1-9][0-9]{0,8}")
REASONS = frozenset({"invalid_choices", "invalid_request", "connector_unavailable", "stale_revision",
                     "unknown_connector", "operation_conflict", "reconnect_required", "scope_unavailable",
                     "approval_required", "replacement_unproven", "replacement_incompatible", "local_only", "operator_only"})
OPTIONS = {"id": "id", "kind": "kind", "label": "label", "repo": "repo", "branch": "branch",
           "whole_repo": "whole-repo", "team": "team", "database": "database", "address": "address",
           "program": "program", "args": "arg", "reads": "reads", "recipient": "recipient"}
FIELDS = {"github": {"repo", "branch", "whole_repo"}, "linear": {"team", "recipient"},
          "notion": {"database"}, "feed": {"address"}, "program": {"program", "args", "reads"}}
LIMITS = {"label": config.MAX_LABEL, "reads": config.MAX_READS, "program": config.MAX_ARGUMENT_CHARACTERS,
          "address": 500, "repo": 200, "branch": 255, "team": 120, "database": 120, "recipient": 120}
MAX_FORM = 70_000


class Refusal(ValueError):
    """A fixed reason safe to return over the cockpit wire."""


def _unique_fields(pairs: list[tuple[str, object]]) -> dict:
    fields = dict(pairs)
    if len(fields) != len(pairs):
        raise Refusal("invalid_choices")
    return fields


def form(value: str, *, edit: bool = False) -> dict:
    """Decode one operator form and validate its kind-specific fields and bounds."""
    try:
        if text_limits.one_line_problem(value, MAX_FORM):
            raise ValueError
        fields = json.loads(value, object_pairs_hook=_unique_fields)
        return validate(fields, edit=edit)
    except (ValueError, RecursionError):
        raise Refusal("invalid_choices") from None


def validate(fields: object, *, edit: bool = False) -> dict:
    """Validate typed fields shared by the cockpit and connectors CLI."""
    if not isinstance(fields, dict) or "label" not in fields or any(
        not _valid_field(name, value) for name, value in fields.items()
    ):
        raise Refusal("invalid_choices")
    if edit and fields.keys() <= {"label", "reads"}:
        return fields
    kind = fields.get("kind")
    if kind not in FIELDS or fields.keys() - FIELDS[kind] - {"kind", "label"} or not _valid_scope(kind, fields):
        raise Refusal("invalid_choices")
    return fields


def _valid_field(name: str, value: object) -> bool:
    if name == "kind":
        return isinstance(value, str) and value in FIELDS
    if name == "whole_repo":
        return value is True
    if name == "args":
        return isinstance(value, list) and len(value) < config.MAX_ARGUMENTS and all(
            isinstance(word, str) and (not word or not text_limits.one_line_problem(word, config.MAX_ARGUMENT_CHARACTERS))
            for word in value)
    return (name in LIMITS and isinstance(value, str) and bool(value.strip())
            and text_limits.one_line_problem(value, LIMITS[name]) is None)


def _valid_scope(kind: str, fields: dict) -> bool:
    if kind == "github":
        return "repo" in fields and (("branch" in fields) != ("whole_repo" in fields))
    required = {"linear": {"team"}, "notion": {"database"}, "feed": {"address"}, "program": {"program", "reads"}}
    return required[kind] <= fields.keys()


def command(verb: str, inputs: dict) -> tuple[tuple[str, ...], list[str], list[str]]:
    """Convert a validated cockpit request to allowlisted connector CLI options."""
    options = []
    for name, value in inputs.items():
        if name == "choices":
            fields = form(value, edit=verb == "edit")
            options.extend(flag for field in fields for flag in _flags(field, fields[field]))
        elif name in {"name", "candidate", "op", "revision", "candidate_revision"}:
            pattern = REVISION if name in {"revision", "candidate_revision"} else ID
            if not isinstance(value, str) or not pattern.fullmatch(value):
                raise Refusal("invalid_request")
            option = "id" if name == "name" else name.replace("_", "-")
            options.append(f"--{option}={value}")
        else:
            raise Refusal("invalid_request")
    return ("connectors", verb), options, []


def _flags(name: str, value: object) -> list[str]:
    option = OPTIONS[name]
    if name == "whole_repo":
        return [f"--{option}"]
    if name == "args":
        return [f"--{option}={argument}" for argument in value]
    return [f"--{option}={value}"]


def client_reason(output: str) -> str:
    """Return only a recognized reason, even if a subprocess prints arbitrary text."""
    value = output.strip()
    return value if value in REASONS else "connector_unavailable"


def receipt(output: str) -> dict:
    """Return only the ID and revision of a well-formed mutation receipt."""
    try:
        result = json.loads(output)
        if (isinstance(result, dict) and isinstance(result.get("id"), str) and ID.fullmatch(result["id"])
                and type(result.get("revision")) is int and REVISION.fullmatch(str(result["revision"]))):
            return {"id": result["id"], "revision": result["revision"]}
    except (ValueError, RecursionError):
        pass
    return {}


def client_receipt(output: str) -> str:
    """Encode a checked receipt for the common string output field."""
    return json.dumps(receipt(output), separators=(",", ":"))
