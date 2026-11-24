"""Turn one optional step of one lane on or off, for the steps the lane has not reached yet.

A switch is the lane's own word: it lives in the lane's overrides (the `flags` layer of its config), so the global preset
and the rules layer stay as they are and another lane is not touched. The step's knob is the only thing it sets, with what
that step needs when a review is turned on while the reviewers are off (`rules.review_companions`).
"""

from __future__ import annotations

from sakura.flow import config, pipeline, rules, run_records

ON, OFF = "on", "off"
VALUES = (ON, OFF)
STATE_DONE, STATE_RUNNING, STATE_NEXT, STATE_OFF, STATE_WAITING = "done", "running", "next", "off", "waiting"
# The lane's statuses in which the step it is in waits for the person instead of running.
WAITING_STATUSES = ("awaiting_approval", "blocked", "failed", "stopped")
WAITING_NOTE = "Waiting here: the lane needs you before it goes on."
# Every step of a lane in the order it runs, with the optional ones that have a knob of their own.
ORDER = ("plan", "plan_review", "build", "task_review", "final_review", "checks", "evaluator", "deliver", "reporter")
SWITCHABLE = ("plan", "plan_review", "task_review", "final_review", "checks", "evaluator", "reporter")
NAMES = {"plan_review": "plan review", "task_review": "task review", "final_review": "final review"}
# The first and last engine phase each step lives in; a step inside a phase that needs a record to say it started is in EVIDENCE.
PHASES = {"plan": ("plan", "plan"), "plan_review": ("plan", "plan"), "build": ("build", "build"),
          "task_review": ("build", "build"), "final_review": ("build", "build"), "checks": ("evaluate", "evaluate"),
          "evaluator": ("evaluate", "evaluate"), "deliver": ("pr", "report"), "reporter": ("report", "report")}
EVIDENCE = ("plan_review", "task_review", "final_review")
ROLE_NEEDED = {"plan_review": "plan_reviewer", "task_review": "code_reviewer", "final_review": "code_reviewer",
               "evaluator": "evaluator"}
FINAL_REVIEW_KEY = "final_review"  # `phases.final_review.STATE_KEY`


class LaneStepRefused(ValueError):
    """Raised, with the reason in plain words, when a step cannot be turned on or off for a lane."""


def name_of(step: str) -> str:
    """The step as a person says it."""
    return NAMES.get(step, step)


def require_known(step: str, value: str) -> None:
    """Raise LaneStepRefused unless `step` is a step of a lane and `value` is `on` or `off`."""
    if step not in ORDER:
        raise LaneStepRefused(f"unknown step {step!r}; use one of {', '.join(ORDER)}")
    if value not in VALUES:
        raise LaneStepRefused(f"a step is turned on or off; got {value!r}, use on or off")


def check(values: dict, state: dict, step: str, value: str) -> None:
    """Raise LaneStepRefused, with the reason, unless `step` may be turned `value` for the lane now."""
    require_known(step, value)
    reason = refusal(values, state, step, value == ON)
    if reason:
        raise LaneStepRefused(reason)


def refusal(values: dict, state: dict, step: str, on: bool) -> str:
    """Why `step` cannot be turned on (`on`) or off for the lane now, or an empty string when it can."""
    if step not in SWITCHABLE:
        return f"{name_of(step)} is always on: the least a lane does is {step}"
    where = _where(state, step)
    if where != "before":
        return _reached_words(step, where, is_on_for_lane(values, state, step))
    return _needs(values, step) if on else ""


def edits(values: dict, step: str, on: bool) -> dict:
    """The config keys that turn `step` on or off for a lane running with `values`."""
    changes = {f"knobs.{step}": on}
    if on and step in pipeline.REVIEW_IDS:
        changes.update(rules.review_companions(step, values["knobs"]))
    return changes


def apply(state: dict, values: dict, step: str, on: bool) -> None:
    """Keep the switch in the lane's overrides, with the relaxation it needs to take effect."""
    overrides = state.setdefault("overrides", {})
    keys = edits(values, step, on)
    for key, value in keys.items():
        overrides.setdefault("knobs", {})[key.removeprefix("knobs.")] = value
    overrides["relax"] = sorted({*overrides.get("relax", []), *keys})


def stored_knobs_moved(values: dict, state: dict) -> bool:
    """Whether the lane's own step switches name a value other than the one `values` was loaded with."""
    kept = (state.get("overrides") or {}).get("knobs") or {}
    return any(kept[step] != values["knobs"][step] for step in SWITCHABLE if step in kept)


def of_lane(values: dict, state: dict) -> list[dict]:
    """Every step of the lane: `{id, name, state, on, can_toggle, reason, note}`; `state` is done, running, waiting, next or
    off (waiting: the lane is in this step and needs the person, `note` says so in words), and `reason` says why a step
    cannot be turned on or off now."""
    return [_entry(values, state, step) for step in ORDER]


def current(pipeline_steps: list[dict]) -> dict | None:
    """The step the lane is in now, the one source for a header and a step bar: the last step that runs or waits, as
    `{id, name, waiting, note}`; None when none does (a lane that has not started, or is past delivery)."""
    found = [step for step in pipeline_steps if step["state"] in (STATE_RUNNING, STATE_WAITING)]
    if not found:
        return None
    step = found[-1]
    return {"id": step["id"], "name": step["name"], "waiting": step["state"] == STATE_WAITING, "note": step["note"]}


def is_on_for_lane(values: dict, state: dict, step: str) -> bool:
    """Whether `step` is on for this lane: the config it runs with says so, or the step ran (its run is on the lane's
    record), whatever the preset says now."""
    return is_on(values, step) or _has_started(state, step)


def is_on(values: dict, step: str) -> bool:
    """Whether the lane runs `step` with `values`."""
    built = pipeline.of_values(values)
    if step in pipeline.REVIEW_IDS:
        return built.review(step).on
    if step == "evaluator":
        return config.steps(values).evaluate and built.step("checks").on
    if step == "reporter":
        return bool(values["knobs"]["reporter"])
    return built.step(step).on


def _reached_words(step: str, where: str, was_on: bool) -> str:
    if not was_on:
        return f"{name_of(step)} was off when the lane reached it: it cannot be turned on now"
    return f"{name_of(step)} is already done on this lane" if where == "past" else f"{name_of(step)} is running on this lane"


def _entry(values: dict, state: dict, step: str) -> dict:
    on = is_on_for_lane(values, state, step)
    reason = refusal(values, state, step, not on)
    now = _state(state, step, on)
    return {"id": step, "name": name_of(step).capitalize(), "state": now, "on": on,
            "can_toggle": not reason, "reason": reason, "note": WAITING_NOTE if now == STATE_WAITING else ""}


def _state(state: dict, step: str, on: bool) -> str:
    where = _where(state, step)
    if not on:
        return STATE_OFF
    if where == "in" and state.get("status") in WAITING_STATUSES:
        return STATE_WAITING
    return {"past": STATE_DONE, "in": STATE_RUNNING}.get(where, STATE_NEXT)


def _needs(values: dict, step: str) -> str:
    role = ROLE_NEEDED.get(step)
    if role and values["roles"][role] == config.NO_ROLE:
        return f"the {name_of(step)} cannot run: the {role} has no model; set one first"
    built = pipeline.of_values(values)
    if step == "plan_review" and not built.step("plan").on:
        return "the plan review needs the plan step on: turn the plan on first"
    if step == "evaluator" and not built.step("checks").on:
        return "the evaluator proves rows in the checks step: turn checks on first"
    return ""


def _where(state: dict, step: str) -> str:
    """`past`, `in` or `before`: where the lane is, in its phases, against `step`."""
    # Imported here: the engine imports the inbox, which imports this module.
    from sakura.flow import engine

    order = engine.PHASE_ORDER
    first, last = PHASES[step]
    index = order.index(state["phase"]) if state.get("phase") in order else 0
    if index > order.index(last):
        return "past"
    if index < order.index(first) or (step in EVIDENCE and not _has_started(state, step)):
        return "before"
    return "in"


def _has_started(state: dict, step: str) -> bool:
    runs = (run for run in (state.get("runs") or {}).values() if isinstance(run, dict))
    ran = any((run.get("step") or run_records.STEP_OF_ROLE.get(run.get("role", ""), "")) == step for run in runs)
    return ran or (step == "final_review" and bool(state.get(FINAL_REVIEW_KEY)))
