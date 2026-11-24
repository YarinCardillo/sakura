"""A lane as a pipeline of steps: plan, build, checks and deliver, with the reviews that loop on them.

The pipeline is a reading of the flow knobs, never a second store: `of_knobs` derives it, so a preset, a config
file and a rule all say the same thing in one place. A review runs only while the `reviewer` knob is on; with `plan`
off the lane builds from its goal and the plan review goes with it; `build` and `deliver` are always on.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Mapping

from sakura.flow import presets

STEP_IDS = ("plan", "build", "checks", "deliver")
REVIEW_IDS = ("plan_review", "task_review", "final_review")
PROJECT_ACCOUNT = presets.PROJECT_ACCOUNT
# The role whose model each step or review runs on; deliver runs no agent of its own.
STEP_ROLES = {"plan": "planner", "build": "builder", "checks": "evaluator", "plan_review": "plan_reviewer",
              "task_review": "code_reviewer", "final_review": "code_reviewer"}
ALWAYS_ON = ("build", "deliver")


@dataclass(frozen=True)
class Step:
    """One step of a lane: whether it runs, and the model and account that run it (empty model: no agent of its own)."""

    id: str
    on: bool
    model: str = ""
    account: str = PROJECT_ACCOUNT


@dataclass(frozen=True)
class Review:
    """A review that loops on a step: it asks for changes at most `rounds` times."""

    id: str
    on: bool
    model: str = ""
    account: str = PROJECT_ACCOUNT
    rounds: int = 0


@dataclass(frozen=True)
class Pipeline:
    """The steps and the reviews of a lane, in the order they run."""

    steps: tuple[Step, ...]
    reviews: tuple[Review, ...]

    def step(self, step_id: str) -> Step:
        """The step `step_id`."""
        return next(step for step in self.steps if step.id == step_id)

    def review(self, review_id: str) -> Review:
        """The review `review_id`."""
        return next(review for review in self.reviews if review.id == review_id)


def of_knobs(knobs: Mapping, roles: Mapping | None = None, step_accounts: Mapping | None = None) -> Pipeline:
    """The pipeline the knobs describe; `roles` give each step its model and `step_accounts` its account."""
    roles, step_accounts = roles or {}, step_accounts or {}
    reviewer = bool(knobs["reviewer"])
    plan_on = bool(knobs["plan"])
    switched = {"plan": plan_on, "build": True, "checks": bool(knobs["checks"]), "deliver": True,
                "plan_review": reviewer and plan_on and bool(knobs["plan_review"]),
                "task_review": reviewer and bool(knobs["task_review"]),
                "final_review": reviewer and bool(knobs["final_review"])}
    rounds = {"plan_review": knobs["fix_rounds"], "task_review": knobs["fix_rounds"],
              "final_review": knobs["final_review_rounds"]}

    def model(item: str) -> str:
        return str(roles.get(STEP_ROLES.get(item, ""), ""))

    def account(item: str) -> str:
        return str(step_accounts.get(item, PROJECT_ACCOUNT))

    return Pipeline(tuple(Step(item, switched[item], model(item), account(item)) for item in STEP_IDS),
                    tuple(Review(item, switched[item], model(item), account(item), rounds[item]) for item in REVIEW_IDS))


def of_values(values: Mapping) -> Pipeline:
    """The pipeline of a loaded flow config (`FlowConfig.values`)."""
    return of_knobs(values["knobs"], values["roles"], values.get("step_accounts"))


def to_toml(pipeline: Pipeline) -> str:
    """The preset-file lines that give a lane this pipeline: the switches of its steps and reviews, rounds and accounts.

    Reviews not on are written off explicitly, so the file reads the same whatever preset it is based on.
    """
    lines = [f"reviewer = {_bool(any(review.on for review in pipeline.reviews))}",
             f"plan = {_bool(pipeline.step('plan').on)}", f"checks = {_bool(pipeline.step('checks').on)}"]
    lines += [f"{review.id} = {_bool(review.on)}" for review in pipeline.reviews]
    lines += [f"fix_rounds = {pipeline.review('task_review').rounds}",
              f"final_review_rounds = {pipeline.review('final_review').rounds}"]
    chosen = {item.id: item.account for item in (*pipeline.steps, *pipeline.reviews) if item.account != PROJECT_ACCOUNT}
    if chosen:
        lines += ["", "[accounts]", *(f'{item} = "{account}"' for item, account in chosen.items())]
    return "\n".join(lines) + "\n"


def _bool(value: bool) -> str:
    return "true" if value else "false"
