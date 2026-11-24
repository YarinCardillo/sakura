"""Calibration records and baseline-judge rules for evaluators.

What ran on the base build and how it ended is the engine's: it runs every calibration row itself on its read-only
checkout of the base build and keeps a receipt of each result, a failing one too (review 13, finding 2). The
evaluator's verdicts are judged against those receipts and never stand in for a run: a calibration is recorded only
with a receipt of every row at the base build's revision and the contract's digest.
"""

from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path

from sakura import engine_home
from sakura.flow import checks
from sakura.flow.contract import Row

CALIBRATION_FILE = "calibration.json"


def is_current(repo: Path, evaluator_role: str) -> bool:
    """Whether this evaluator role has a successful calibration record."""
    try:
        data = json.loads(_path(repo).read_text())
    except (OSError, ValueError):
        return False
    return isinstance(data, dict) and isinstance(data.get(evaluator_role), dict)


class CalibrationRefused(RuntimeError):
    """A calibration without the engine's receipt of every row on the base build; nothing is recorded."""


def record(repo: Path, evaluator_role: str, task: str, rows: list[Row], receipts: dict, revision: str,
           digest: str) -> None:
    """Record that `evaluator_role` passed calibration on `task`, on the engine's `receipts` of every row of `rows` at
    the base build `revision` and the contract `digest`. Raises CalibrationRefused, recording nothing, otherwise."""
    missing = _unreceipted(rows, receipts, revision, digest)
    if missing:
        raise CalibrationRefused(f"no engine receipt of {', '.join(missing)} on the base build {revision[:12]}; "
                                 "calibration is not recorded")
    path = _path(repo)
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        data = {}
    data[evaluator_role] = {"task": task, "revision": revision, "contract": digest,
                            "at": datetime.now(timezone.utc).isoformat(timespec="seconds")}
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    with temporary.open("w") as stream:
        stream.write(json.dumps(data, indent=2) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def judge(rows: list[Row], verdicts: list[dict], receipts: dict, revision: str, digest: str) -> list[str]:
    """Return calibration errors: a row with no engine receipt at the base build `revision` and contract `digest`, a
    row whose engine result contradicts its kind there (a baseline row must exit 0, a new row must not: review 14,
    finding 4), a baseline row the evaluator does not PASS with evidence, and a new row it PASSes. The engine's
    results decide; the verdicts are judged against them."""
    by_key = {(entry.get("id"), entry.get("viewport")): entry for entry in verdicts if isinstance(entry, dict)}
    errors = [f"{key} has no engine receipt on the base build" for key in _unreceipted(rows, receipts, revision, digest)]
    for row in rows:
        receipt = receipts.get(checks.row_key(row)) or {}
        result = by_key.get((row.id, row.viewport))
        verdict = result.get("verdict") if result else None
        evidence = result.get("evidence") if result else None
        if row.baseline and receipt.get("exit", 0) != 0:
            errors.append(f"{row.id} baseline fails on the base build (exit {receipt['exit']})")
        elif not row.baseline and receipt.get("exit") == 0:
            errors.append(f"{row.id} non-baseline passes on the base build (exit 0)")
        elif row.baseline and (verdict != "PASS" or not evidence):
            errors.append(f"{row.id} baseline must be PASS with evidence")
        elif not row.baseline and verdict == "PASS":
            errors.append(f"{row.id} non-baseline must not be PASS")
    return errors


def _unreceipted(rows: list[Row], receipts: dict, revision: str, digest: str) -> list[str]:
    """The rows of `rows` without an engine receipt at `revision` for the contract `digest`."""
    missing = []
    for row in rows:
        receipt = receipts.get(checks.row_key(row))
        if not isinstance(receipt, dict) or receipt.get("revision") != revision or receipt.get("contract") != digest:
            missing.append(checks.row_key(row))
    return missing


def _path(repo: Path) -> Path:
    return engine_home.flow_dir(repo) / CALIBRATION_FILE
