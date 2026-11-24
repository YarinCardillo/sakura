"""Shape-based prediction: what a task or a whole flow will cost, in weighted tokens per model class.

The estimate is the median cost per changed file of the nearest past samples times the task's files, with the
20th to 80th percentile as the band. Converting to a share of the week is the caller's job (it needs the
calibration ratios of the account).
"""

from __future__ import annotations

import statistics
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import NamedTuple

from sakura.budget.history import TASK_KINDS, Sample, class_of
from sakura.budget.routing import Route
from sakura.flow.plan import BuildTask

MIN_SAMPLES = 5
LOW_PERCENTILE, HIGH_PERCENTILE = 20, 80
FEW_SAMPLES_LOW, FEW_SAMPLES_HIGH = 0.5, 2.0
NO_HISTORY = "no history"
FEW_SAMPLES = "few samples"
OTHER_CLASS = "other"
# Role medians are per run; a flow runs these this many times per build task (None) or once.
ROLE_RUNS = {"planner": 1, "plan_reviewer": 1, "code_reviewer": None, "evaluator": 1, "reporter": 1}
# The roles that run after the build phase.
POST_BUILD_ROLES = ("evaluator", "reporter")


@dataclass(frozen=True)
class TaskShape:
    """What a task is known to be before it runs: its class and how many files it touches."""

    task_class: str | None
    files: int
    rows: int = 0


@dataclass(frozen=True)
class Prediction:
    """Weighted tokens with a low to high band, the basis that says where it comes from, and its sample count."""

    weighted: float
    low: float
    high: float
    basis: str
    n: int


class Band(NamedTuple):
    weighted: float
    low: float
    high: float


@dataclass(frozen=True)
class FlowPrediction:
    """A flow's weighted tokens per model class (bands add up, so they are conservative), by part in `parts`.

    `missing` names the tasks and roles with no history; they are counted as zero.
    """

    by_class: dict[str, Band]
    parts: dict[str, Prediction]
    missing: tuple[str, ...]
    basis: str


def task(shape: TaskShape, model_class: str, samples: Sequence[Sample]) -> Prediction:
    """Estimate one build task; with no sample at all the answer is zero with the basis `no history`."""
    levels = (
        (f"{shape.task_class} {model_class}", [s for s in samples if s.task_class == shape.task_class
                                        and s.model_class == model_class]),
        (f"any class {model_class}", [s for s in samples if s.model_class == model_class]),
        ("all classes and models", list(samples)),
    )
    candidates = [(name, [s for s in found if s.kind in TASK_KINDS and s.files > 0]) for name, found in levels]
    name, chosen = _nearest(candidates)
    return _estimate([s.weighted / s.files for s in chosen], max(shape.files, 1), f"{_kinds(chosen)} samples, {name}")


def flow(tasks: Sequence[BuildTask], routes: Mapping[str, Route], roles: Mapping[str, str],
         samples: Sequence[Sample]) -> FlowPrediction:
    """Estimate a whole flow: each build task by its route's model class, each role from its own median.

    `routes` maps task id to the Task 3 route (an empty model falls back to the class of the `builder` role);
    `roles` maps a flow role to its model class.
    """
    parts: dict[str, Prediction] = {}
    classes: dict[str, str] = {}
    for item in tasks:
        route = routes.get(item.id)
        classes[item.id] = (class_of(route.model) if route and route.model else "") or roles.get("builder", OTHER_CLASS)
        parts[item.id] = task(TaskShape(item.task_class, len(item.files), len(item.rows)), classes[item.id], samples)
    for role, runs in ROLE_RUNS.items():
        classes[role] = roles.get(role, OTHER_CLASS)
        parts[role] = _role(role, classes[role], runs if runs else len(tasks), samples)
    missing = tuple(name for name, part in parts.items() if part.n == 0)
    totals = _totals({name: part for name, part in parts.items() if part.n}, classes)
    return FlowPrediction(totals, parts, missing, _flow_basis(parts, missing))


def _role(role: str, model_class: str, runs: int, samples: Sequence[Sample]) -> Prediction:
    found = [s for s in samples if s.kind == role]
    same = [s for s in found if s.model_class == model_class]
    chosen = same or found
    label = f"{role} runs, {model_class}" if same else f"{role} runs, any model"
    return _estimate([s.weighted for s in chosen], runs, label)


def _nearest(levels: Sequence[tuple[str, list[Sample]]]) -> tuple[str, list[Sample]]:
    """The nearest level with enough samples; else the nearest one with any."""
    for name, found in levels:
        if len(found) >= MIN_SAMPLES:
            return name, found
    return next(((name, found) for name, found in levels if found), (NO_HISTORY, []))


def _kinds(chosen: Sequence[Sample]) -> str:
    return "+".join(sorted({s.kind for s in chosen}))


def _estimate(values: Sequence[float], scale: float, label: str) -> Prediction:
    if not values:
        return Prediction(0.0, 0.0, 0.0, NO_HISTORY, 0)
    cuts = statistics.quantiles(values, n=100, method="inclusive") if len(values) > 1 else [values[0]] * 99
    weighted = statistics.median(values) * scale
    low, high = cuts[LOW_PERCENTILE - 1] * scale, cuts[HIGH_PERCENTILE - 1] * scale
    if len(values) >= MIN_SAMPLES:
        return Prediction(weighted, low, high, f"median of {len(values)} {label}", len(values))
    low, high = min(low, weighted * FEW_SAMPLES_LOW), max(high, weighted * FEW_SAMPLES_HIGH)
    basis = f"{FEW_SAMPLES}: {len(values)} of {MIN_SAMPLES} needed, {label}"
    return Prediction(weighted, low, high, basis, len(values))


def _totals(parts: Mapping[str, Prediction], classes: Mapping[str, str]) -> dict[str, Band]:
    totals: dict[str, Band] = {}
    for name, part in parts.items():
        before = totals.get(classes[name], Band(0.0, 0.0, 0.0))
        totals[classes[name]] = Band(before.weighted + part.weighted, before.low + part.low, before.high + part.high)
    return totals


def _flow_basis(parts: Mapping[str, Prediction], missing: Sequence[str]) -> str:
    known = len(parts) - len(missing)
    if known == 0:
        return NO_HISTORY
    text = f"{known} of {len(parts)} parts from history"
    if any(FEW_SAMPLES in part.basis for part in parts.values()):
        text += f", {FEW_SAMPLES} in some"
    return text + (f"; {NO_HISTORY} for {', '.join(missing)}" if missing else "")
