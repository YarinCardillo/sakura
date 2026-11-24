"""Contract rows: the acceptance checklist of a task, written once and then verdict-only."""

from __future__ import annotations

import hashlib
import json
import os
import re
from collections import Counter
from collections.abc import Callable
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import Literal

from sakura.flow import controls, readonly_checkout
from sakura.flow.config import FlowConfig, PresetSteps
from sakura.flow.store import TaskStore

NO_NEW_ROW_ERROR = "contract has no non-baseline row"
NO_BASELINE_ERROR = "contract has no baseline row (needed to prove existing behavior still works)"
ROW_FIELDS = ("id", "journey", "step", "kind", "viewport", "flow", "baseline", "passes")
ROW_KINDS = ("web", "api", "cli")
# Who proves a row. `check`: the engine runs `flow` and its exit code is the verdict. `evaluator`: the evaluator
# role's evidence. `owner`: the person's `sakura flow mark`; `flow` then says what to try.
CHECK, EVALUATOR, OWNER = "check", "evaluator", "owner"
PROVERS = (CHECK, EVALUATOR, OWNER)
ENGINE_RUN_PROVERS = (CHECK, EVALUATOR)
ROW_ID = re.compile(r"[A-Za-z0-9_-]{1,40}")
VIEWPORT = re.compile(r"\d{2,5}x\d{2,5}")


class ContractError(ValueError):
    """Raised when contract.json is missing, malformed or cannot take the requested change."""


class ContractTampered(ContractError):
    """Raised when contract.json no longer matches the hash the engine saved."""


@dataclass(frozen=True)
class Row:
    """One acceptance row; `passes` is written only by the engine.

    `prover` is empty in contracts written before provers existed: see `prover_of`.
    """

    id: str
    journey: str
    step: str
    kind: Literal["web", "api", "cli"]
    viewport: str | None
    flow: str
    baseline: bool
    passes: bool
    prover: str = ""


def prover_of(row: Row, steps: PresetSteps) -> str:
    """The row's prover; a row without one is the evaluator's when the preset runs it, else a check."""
    return row.prover or (EVALUATOR if steps.evaluate else CHECK)


def rows_digest(rows: list[Row]) -> str:
    """The digest of the rows without their verdicts: what a receipt binds to, unchanged when a row flips."""
    entries = [{**_entry(row), "passes": False} for row in rows]
    return hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest()


def proven_by(rows: list[Row], steps: PresetSteps, *provers: str) -> list[Row]:
    """The rows whose prover is one of `provers`."""
    return [row for row in rows if prover_of(row, steps) in provers]


def load(store: TaskStore, expected_sha256: str | None) -> list[Row]:
    """Read rows, recovering only a pending engine-authenticated verdict transition."""
    transition = _recover_transition(store)
    if transition and expected_sha256 == transition[0]:
        expected_sha256 = transition[1]
    try:
        raw = store.contract.read_bytes()
    except OSError as error:
        raise ContractError(f"contract.json cannot be read: {error}") from error
    if expected_sha256 is not None and hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ContractTampered("contract.json changed outside the engine")
    return _parse(raw)


def validate(rows: list[Row], config: FlowConfig, steps: PresetSteps) -> list[str]:
    """Return every reason the rows are not an acceptable contract; empty means valid."""
    viewports = config.values["tests"]["viewports"]
    errors = _duplicate_ids(rows) + _mixed_provers(rows, steps)
    row_ids = {row.id for row in rows}
    missing = [row_id for row_id in config.values["contract"]["required_ids"] if row_id not in row_ids]
    if missing:
        errors.append(f"contract is missing required row ids: {', '.join(missing)}")
    for row in rows:
        errors.extend(_row_problems(row, viewports))
        errors.extend(_prover_problems(row, steps))
        errors.extend(_checkout_write_problems(row, steps))
    if not any(not row.baseline for row in rows):
        errors.append(NO_NEW_ROW_ERROR)
    if steps.evaluate and not any(row.baseline for row in rows):
        errors.append(NO_BASELINE_ERROR)
    return errors


def save(store: TaskStore, rows: list[Row]) -> str:
    """Atomically save engine rows, authenticating either the initial or prior digest."""
    _recover_transition(store)
    initial = not store.read_state().get("contract_sha256")
    return _replace_and_bind(store, rows, initial=initial)


def recover_initial(store: TaskStore) -> list[Row] | None:
    """Recover a completed or pending initial save before the contract phase replans."""
    state = store.read_state()
    transition = state.get("contract_transition")
    if isinstance(transition, dict) and transition.get("initial") is True:
        old_digest = transition.get("old_sha256")
        try:
            raw = store.contract.read_bytes() if store.contract.exists() else None
        except OSError as error:
            raise ContractError(f"contract.json cannot be read: {error}") from error
        actual = hashlib.sha256(raw).hexdigest() if raw is not None else ""
        _recover_transition(store)
        if actual == transition.get("new_sha256"):
            return _parse(store.contract.read_bytes())
        if raw is not None and actual == old_digest:
            rows = _parse(raw)
            save(store, rows)
            return rows
        return None
    if transition:
        _recover_transition(store)
        state = store.read_state()
    digest = state.get("contract_sha256")
    if digest:
        return load(store, digest)
    return None


def authorize_replan(store: TaskStore, rewind: Callable[[dict], None] | None = None) -> None:
    """Clear a saved contract only after its current bytes match the authenticated digest.

    `rewind` runs inside the same state update, so a caller's state change and the cleared digest are
    one transition. The contract file is removed only after that write.
    """
    state = store.read_state()
    if state.get("contract_transition"):
        raise ContractError("cannot replan while a contract transition is pending")
    digest = state.get("contract_sha256", "")
    if digest:
        try:
            actual = hashlib.sha256(store.contract.read_bytes()).hexdigest()
        except OSError as error:
            raise ContractTampered("contract.json changed outside the engine") from error
        if actual != digest:
            raise ContractTampered("contract.json changed outside the engine")

    def clear(current: dict) -> None:
        current["contract_sha256"] = ""
        current.pop("contract_transition", None)
        if rewind:
            rewind(current)

    store.update_state(clear)
    store.contract.unlink(missing_ok=True)


def _payload(rows: list[Row]) -> bytes:
    return (json.dumps({"rows": [_entry(row) for row in rows]}, indent=2) + "\n").encode()


def _entry(row: Row) -> dict:
    """A row as saved; an empty prover is left out, so a contract from before provers keeps its bytes."""
    entry = asdict(row)
    if not row.prover:
        entry.pop("prover")
    return entry


def _replace_contract(store: TaskStore, payload: bytes) -> None:
    temp = store.contract.with_name(f"{store.contract.name}.{os.getpid()}.tmp")
    with open(temp, "wb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temp, store.contract)
    descriptor = os.open(store.contract.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _replace_and_bind(store: TaskStore, rows: list[Row], initial: bool = False) -> str:
    state = store.read_state()
    old_digest = state.get("contract_sha256", "")
    payload = _payload(rows)
    new_digest = hashlib.sha256(payload).hexdigest()
    if initial:
        if old_digest:
            if old_digest == new_digest and store.contract.exists() and hashlib.sha256(
                    store.contract.read_bytes()).hexdigest() == new_digest:
                return new_digest
            raise ContractTampered("initial contract already exists")
        try:
            old_payload = store.contract.read_bytes() if store.contract.exists() else None
        except OSError as error:
            raise ContractError(f"contract.json cannot be read: {error}") from error
        if old_payload is not None:
            if _parse(old_payload) != rows:
                raise ContractTampered("initial contract changed before save")
            old_digest = hashlib.sha256(old_payload).hexdigest()
    else:
        try:
            actual = hashlib.sha256(store.contract.read_bytes()).hexdigest()
        except OSError as error:
            raise ContractTampered("contract.json changed outside the engine") from error
        if actual != old_digest:
            raise ContractTampered("contract.json changed outside the engine")
    transition = {"old_sha256": old_digest, "new_sha256": new_digest, "initial": initial}

    def prepare(current: dict) -> None:
        if not initial:
            current["contract_sha256"] = old_digest
        current["contract_transition"] = transition

    store.update_state(prepare)
    _replace_contract(store, payload)

    def finish(current: dict) -> None:
        current["contract_sha256"] = new_digest
        current.pop("contract_transition", None)

    store.update_state(finish)
    return new_digest


def _recover_transition(store: TaskStore) -> tuple[str, str] | None:
    state = store.read_state()
    transition = state.get("contract_transition")
    if transition is None:
        return None
    if not isinstance(transition, dict) or set(transition) != {"old_sha256", "new_sha256", "initial"}:
        raise ContractTampered("contract transition record is invalid")
    old_digest, new_digest = transition["old_sha256"], transition["new_sha256"]
    initial = transition["initial"]
    if (not isinstance(old_digest, str) or (old_digest and not re.fullmatch(r"[0-9a-f]{64}", old_digest))
            or not isinstance(new_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", new_digest)
            or not isinstance(initial, bool)):
        raise ContractTampered("contract transition record is invalid")
    state_old_digest = "" if initial else old_digest
    if state.get("contract_sha256") != state_old_digest:
        raise ContractTampered("contract transition does not match saved state")
    try:
        actual = hashlib.sha256(store.contract.read_bytes()).hexdigest()
    except OSError as error:
        if old_digest == "" and not store.contract.exists():
            actual = ""
        else:
            raise ContractError(f"contract.json cannot be read: {error}") from error
    if actual not in (old_digest, new_digest):
        raise ContractTampered("contract.json does not match its pending transition")

    def finish(current: dict) -> None:
        if actual == new_digest:
            current["contract_sha256"] = new_digest
        current.pop("contract_transition", None)

    store.update_state(finish)
    return (state_old_digest, new_digest) if actual == new_digest else None


def mark_passed(store: TaskStore, state: dict, row_id: str, viewport: str | None, evidence: Path) -> None:
    """Set `passes` on one row, backed by a non-empty evidence file, and update the saved hash."""
    set_verdict(store, state, row_id, viewport, evidence, True)


def set_verdict(store: TaskStore, state: dict, row_id: str, viewport: str | None, evidence: Path,
                passes: bool) -> None:
    """Set `passes` of one row to `passes`, backed by a non-empty evidence file, and update the saved hash."""
    if not evidence.is_file() or evidence.stat().st_size == 0:
        raise ContractError(f"evidence for {row_id} is missing or empty: {evidence}")
    rows = load(store, state["contract_sha256"])
    matching = [row for row in rows if row.id == row_id and row.viewport == viewport]
    if not matching:
        raise ContractError(f"no contract row {row_id} for viewport {viewport}")
    updated = [replace(row, passes=passes) if row in matching else row for row in rows]
    if updated != rows:
        _replace_and_bind(store, updated)


def reset_passes(store: TaskStore, state: dict) -> None:
    """Clear engine verdicts so rows must be evaluated again for a new code revision."""
    rows = load(store, state["contract_sha256"])
    if not any(row.passes for row in rows):
        return
    _replace_and_bind(store, [replace(row, passes=False) for row in rows])


def _parse(raw: bytes) -> list[Row]:
    try:
        entries = json.loads(raw)["rows"]
        rows = [Row(**{field: entry[field] for field in ROW_FIELDS}, prover=entry.get("prover", "")) for entry in entries]
    except (ValueError, KeyError, TypeError) as error:
        raise ContractError(f"contract.json is malformed: {error!r}") from error
    for row in rows:
        if isinstance(row.flow, str) and any(controls.is_control(character) for character in row.flow):
            # A command the person approves is shown and remembered exactly: none of these reaches the gate.
            raise ContractError(f"row {row.id}: flow holds a control character: {controls.escaped(row.flow)}")
    return rows


def _duplicate_ids(rows: list[Row]) -> list[str]:
    counts = Counter(str(row.id) for row in rows)
    return [f"duplicate row id {row_id}" for row_id, count in sorted(counts.items()) if count > 1]


def _row_problems(row: Row, viewports: list[str]) -> list[str]:
    problems = []
    if not (isinstance(row.id, str) and ROW_ID.fullmatch(row.id)):
        problems.append(f"row id {row.id!r} must match {ROW_ID.pattern}")
    if row.passes is not False:
        problems.append(f"row {row.id}: passes must be false at creation")
    if not isinstance(row.baseline, bool):
        problems.append(f"row {row.id}: baseline must be true or false (JSON boolean)")
    if row.kind not in ROW_KINDS:
        problems.append(f"row {row.id}: kind must be one of {', '.join(ROW_KINDS)}")
    problems.extend(_viewport_problems(row, viewports))
    if not isinstance(row.flow, str) or not row.flow.strip():
        problems.append(f"row {row.id}: flow command is missing")
    return problems


def _checkout_write_problems(row: Row, steps: PresetSteps) -> list[str]:
    """A command the engine runs may not write inside its read-only checkout (see `readonly_checkout`)."""
    if not isinstance(row.flow, str) or prover_of(row, steps) not in ENGINE_RUN_PROVERS:
        return []
    writes = readonly_checkout.plain_writes(row.flow)
    return [readonly_checkout.refusal(row.id, writes)] if writes else []


def _mixed_provers(rows: list[Row], steps: PresetSteps) -> list[str]:
    """Rows sharing an id (one per viewport) are marked or checked together, so they must share one prover."""
    provers: dict[str, set[str]] = {}
    for row in rows:
        provers.setdefault(str(row.id), set()).add(prover_of(row, steps))
    return [f"row {row_id}: rows sharing an id must share one prover; got {', '.join(sorted(names))}"
            for row_id, names in sorted(provers.items()) if len(names) > 1]


def _prover_problems(row: Row, steps: PresetSteps) -> list[str]:
    if row.prover and row.prover not in PROVERS:
        return [f"row {row.id}: prover must be one of {', '.join(PROVERS)}; got {row.prover!r}"]
    if row.prover == EVALUATOR and not steps.evaluate:
        return [f"row {row.id}: prover evaluator needs the evaluator, which this preset turns off; use check or owner"]
    return []


def _viewport_problems(row: Row, viewports: list[str]) -> list[str]:
    if row.kind == "web":
        valid = isinstance(row.viewport, str) and VIEWPORT.fullmatch(row.viewport) and row.viewport in viewports
        return [] if valid else [f"row {row.id}: web rows need a viewport from {', '.join(viewports)}; got {row.viewport!r}"]
    if row.viewport is not None:
        return [f"row {row.id}: viewport must be null for {row.kind} rows"]
    return []
