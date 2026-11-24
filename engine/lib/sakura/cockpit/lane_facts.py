"""What a lane's page shows beside its state: the goal, the steps, who opened it, the preset, the pace and the reviews.

Everything here is read from the lane's own records (its input, ledger and state) and the effective flow config; nothing
is decided or written. The goal is the person's or the master's own text and is data, never instructions.
"""

from __future__ import annotations

import io
from pathlib import Path
from typing import BinaryIO

from sakura import actors, file_cache
from sakura.flow import config, lane_ceiling, lane_steps, pace, pace_check, pipeline, presets, step_accounts, tasks
from sakura.flow.progress import Progress

GOAL_LIMIT = 8000
INPUT_FILE = "input.md"
LEDGER_FILE = "ledger.md"
OPENED_BY_MASTER, OPENED_BY_PERSON, OPENED_BY_OUTSIDE = "master", "person", "outside"
STATE_DONE, STATE_NOW, STATE_NEXT, STATE_SKIPPED = "done", "now", "next", "skipped"
REVIEW_ROLES = ("plan_reviewer", "code_reviewer")
LIVE_RUN_STATES = ("starting", "running")
# The lane's steps, each the engine phases it covers; cleanup is not a step.
STEPS = (("plan", ("intake", "contract", "plan")), ("build", ("build",)), ("checks", ("evaluate",)),
         ("deliver", ("pr", "report")))
STEP_ROLES = {"plan": "planner", "build": "builder", "checks": "evaluator", "deliver": "reporter"}
# The reviews, the config role behind each and the step each one follows. The final review runs on the code reviewer's model.
REVIEWS = (("plan_review", "plan_reviewer", "plan"), ("task_review", "code_reviewer", "build"),
           ("final_review", "code_reviewer", "deliver"))


def lane_facts(task_dir: Path, state: dict, knobs: dict) -> dict:
    """The facts of a flow lane: `goal`, `opened_by`, `preset` and its `preset_label`, `dial`, `pace`, `steps`, `reviews`, `in_review`
    and `pipeline`: every step of the lane as `{id, name, state, on, can_toggle, reason}` (`flow/lane_steps.py`), `state` being
    done, running, next or off and `reason` why `can_toggle` is false.

    `task_dir` is the lane's task folder; `state` its state; `knobs` the effective flow config values
    (`FlowConfig.values`), from which the preset, the dial, the roles and the optional steps are read. A step is
    `{id, name, state, model, account}`; the account is None while the step runs on the project's own account.
    """
    steps = _steps(state, knobs)
    pipe = lane_steps.of_lane(knobs, state)
    return {"goal": _goal(task_dir, state), "opened_by": _opened_by(task_dir), "preset": knobs["preset"],
            "preset_label": presets.label(knobs["preset"]), "dial": knobs["dial"], "pace": pace_of(state, pace.lane_level(knobs["knobs"])), "steps": steps, "reviews": _reviews(steps, state, knobs),
            "in_review": _in_review(state), "pipeline": pipe, "current_step": lane_steps.current(pipe)}


def step_progress(found: Progress) -> dict:
    """A flow lane's progress in the steps the page draws (Plan, Build, Checks, Deliver), counted within the current one in
    words a person knows: `Build, 2 of 5 tasks`, `Checks, 3 of 9 passed`; Plan and Deliver have nothing to count. The
    engine's own phase and the counts of tasks, checks and everything together stay in their own fields."""
    step = next((step_id for step_id, phases in STEPS if found.phase in phases), STEPS[-1][0] if found.phase else "")
    done, total, unit = _counted_in(step, found)
    label = step.capitalize()
    return {"step": step, "phase": label, "done": done, "total": total, "unit": unit,
            "text": f"{label}, {done} of {total} {unit}" if total else label, "engine_phase": found.phase,
            "tasks_done": found.tasks_done, "tasks_total": found.tasks_total, "checks_done": found.rows_done,
            "checks_total": found.rows_total, "overall_done": found.done, "overall_total": found.total}


def _counted_in(step: str, found: Progress) -> tuple[int, int, str]:
    if step == "build":
        return found.tasks_done, found.tasks_total, "tasks"
    if step == "checks":
        return found.rows_done, found.rows_total, "passed"
    return 0, 0, ""


def outside_facts(title: str, about: str) -> dict:
    """The facts of an outside lane: it has no steps or reviews, its goal is its first report (else its title)."""
    return {"goal": (about or title)[:GOAL_LIMIT], "opened_by": OPENED_BY_OUTSIDE, "steps": [], "reviews": [],
            "in_review": False}


# Opener and goal

def _goal(task_dir: Path, state: dict) -> str:
    """The task input; for a lane that was given a title the description after it, or the whole text when there is none."""
    try:
        text = file_cache.load(task_dir / INPUT_FILE, _read_goal)
    except OSError:
        return ""
    return (tasks.split_title(text)[1] or text.strip()) if state.get(tasks.TITLE_KEY) else text


def _read_goal(stream: BinaryIO) -> str:
    text = io.TextIOWrapper(stream, errors="replace")
    try:
        return text.read(GOAL_LIMIT)
    finally:
        text.detach()


def _opened_by(task_dir: Path) -> str:
    ledger = task_dir / LEDGER_FILE
    try:
        contents = file_cache.small_bytes(ledger)
    except OSError:
        return OPENED_BY_PERSON
    opener = lane_ceiling.opener_in(ledger, contents=contents)
    return OPENED_BY_MASTER if opener and opener.source == actors.MASTER_SOURCE else OPENED_BY_PERSON


# Steps

def _steps(state: dict, knobs: dict) -> list[dict]:
    current = _current_step(state)
    skipped = _skipped_steps(state, knobs)
    roles = knobs["roles"]
    return [{"id": step_id, "name": step_id.capitalize(), "state": _step_state(index, current, step_id in skipped),
             "model": _role_model(roles, STEP_ROLES[step_id]), "account": _account(knobs, state, step_id)}
            for index, (step_id, _) in enumerate(STEPS)]


def _current_step(state: dict) -> int:
    """The index of the step the lane is in; the number of steps once it is past delivery (cleanup)."""
    phase = state.get("phase") or STEPS[0][1][0]
    return next((index for index, (_, phases) in enumerate(STEPS) if phase in phases), len(STEPS))


def _step_state(index: int, current: int, is_skipped: bool) -> str:
    if is_skipped:
        return STATE_SKIPPED
    if index < current:
        return STATE_DONE
    return STATE_NOW if index == current else STATE_NEXT


def _skipped_steps(state: dict, knobs: dict) -> set[str]:
    skipped = set()
    # The checks step runs the project's own checks even without an evaluator (`phases/evaluate.py`): it is
    # skipped only when it is switched off, as the pipeline draws it.
    if not pipeline.of_values(knobs).step("checks").on:
        skipped.add("checks")
    if not pipeline.of_values(knobs).step("plan").on or any(entry.get("phase") == "plan" and entry.get("status") == "skipped" for entry in state.get("history") or []):
        skipped.add("plan")
    return skipped


def _role_model(roles: dict, role: str | None) -> str | None:
    model = roles.get(role) if role else None
    return None if model in (None, config.NO_ROLE) else model


def pace_of(state: dict, engine_level: str) -> str:
    """The lane's pace from its state alone: its own (`flow/pace.py`), else `engine_level`; for a lane whose config cannot
    be read."""
    return ((state.get("overrides") or {}).get("knobs") or {}).get("lane_pace") or engine_level


def _account(values: dict, state: dict, item_id: str) -> str | None:
    """The Leeway account a step or review runs on, or None for the project's own (`flow/step_accounts.py`)."""
    role = next((name for name, step in pace_check.STEP_OF_ROLE.items() if step == item_id), None)
    chosen = (step_accounts.account_of(values, state, role) if role
              else values["step_accounts"].get(item_id, pace.PROJECT_ACCOUNT))
    return None if chosen == pace.PROJECT_ACCOUNT else chosen


# Reviews

def _reviews(steps: list[dict], state: dict, knobs: dict) -> list[dict]:
    states = {step["id"]: step["state"] for step in steps}
    enabled = config.steps(knobs)
    is_on = {"plan_review": enabled.plan_review, "task_review": enabled.code_review, "final_review": enabled.final_review}
    is_on["plan_review"] = is_on["plan_review"] or _plan_review_ran(state)
    return [_review(review_id, role, is_on[review_id] and states[step_id] != STATE_SKIPPED, states[step_id], state, knobs)
            for review_id, role, step_id in REVIEWS]


def _plan_review_ran(state: dict) -> bool:
    """Whether the plan was reviewed, even if the preset was switched to one without that review since: a review that
    happened stays shown as done."""
    return any(isinstance(run, dict) and run.get("role") == "plan_reviewer" for run in (state.get("runs") or {}).values())


def _review(review_id: str, role: str | None, is_on: bool, step_state: str, state: dict, knobs: dict) -> dict:
    return {"id": review_id, "on": is_on, "model": _role_model(knobs["roles"], role) if is_on else None,
            "account": _account(knobs, state, review_id) if is_on else None,
            "state": step_state if is_on else STATE_SKIPPED}


def _in_review(state: dict) -> bool:
    return any(isinstance(run, dict) and run.get("role") in REVIEW_ROLES and run.get("state") in LIVE_RUN_STATES
               for run in (state.get("runs") or {}).values())
