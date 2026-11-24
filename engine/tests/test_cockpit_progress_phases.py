"""The cockpit's progress agrees with `flow status --json` in every phase (rows from the contract as soon as it
exists, plan tasks from plan.json as soon as it exists)."""

import json

import pytest

from sakura.flow import cli, contract, plan, progress

from test_cockpit_snapshot import collect, create_task, item_for

TASK_IDS = ("T1", "T2")
ROW_IDS = ("R1", "R2", "R3")
PLAN_DRIFT_LINE = "changed after the plan was approved"


def create_started_task(repo, phase):
    """A task as `flow start` records it: both digests empty until the engine binds them."""
    return create_task(repo, "demo", phase=phase, contract_sha256="", plan_sha256="")


def rows():
    return [contract.Row(row_id, "journey", "step", "cli", None, "true", False, False, "check") for row_id in ROW_IDS]


def write_contract(task, *, approved):
    """The contract file as the contract agent leaves it, bound to the state only once approved."""
    if approved:
        contract.save(task, rows())
    else:
        task.contract.write_bytes(contract._payload(rows()))


def write_plan(task, *, approved):
    task.plan_json.write_text(json.dumps({"tasks": [
        {"id": task_id, "title": task_id, "depends_on": [], "rows": list(ROW_IDS), "files": [f"{task_id}.py"],
         "seam": False} for task_id in TASK_IDS]}))
    task.plan_md.write_text("# plan\n")
    if approved:
        digest = plan.digest(task)
        task.update_state(lambda state: state.update(plan_sha256=digest))


def merged(task):
    task.update_state(lambda state: state.update(build={task_id: {"status": "merged"} for task_id in TASK_IDS}))


PHASES = {
    "contract_written": ("contract", lambda task: write_contract(task, approved=False), progress.Progress(0, 3, "contract")),
    "contract_approved": ("contract", lambda task: write_contract(task, approved=True), progress.Progress(0, 3, "contract")),
    "plan_written": ("plan", lambda task: (write_contract(task, approved=True), write_plan(task, approved=False)),
                     progress.Progress(0, 5, "plan")),
    "plan_approved": ("plan", lambda task: (write_contract(task, approved=True), write_plan(task, approved=True)),
                      progress.Progress(0, 5, "plan")),
    "building": ("build", lambda task: (write_contract(task, approved=True), write_plan(task, approved=True),
                                        merged(task)), progress.Progress(2, 5, "build")),
    "checks": ("checks", lambda task: (write_contract(task, approved=True), write_plan(task, approved=True),
                                       merged(task)), progress.Progress(2, 5, "checks")),
    "pr": ("pr", lambda task: (write_contract(task, approved=True), write_plan(task, approved=True), merged(task)),
           progress.Progress(2, 5, "pr")),
}


@pytest.mark.parametrize("name", PHASES)
def test_Progress_EveryPhase_AgreesWithFlowStatus(name, repo):
    phase, prepare, expected = PHASES[name]
    task = create_started_task(repo, phase)
    prepare(task)

    item = item_for(collect([repo]), "demo")
    status = cli._task_status(repo, task)

    assert item.progress == expected
    assert item.progress.phase == status["phase"]
    assert status["contract"]["total"] == len(ROW_IDS)
    assert not [line for line in item.pending if line.startswith("problem:")]


def test_Progress_PlanChangedAfterRecordedDigest_ShowsTheProblemLine(repo):
    task = create_started_task(repo, "build")
    write_contract(task, approved=True)
    write_plan(task, approved=True)
    task.plan_md.write_text("# plan, edited\n")

    item = item_for(collect([repo]), "demo")

    assert [line for line in item.pending if PLAN_DRIFT_LINE in line] != []


# -- the document counts in the four steps the app draws, in words a person knows ---------------------------------------------


def progress_document(repo, phase, prepare):
    from sakura.cockpit import api

    task = create_started_task(repo, phase)
    prepare(task)
    return api.snapshot_document(collect([repo]))["items"][0]["progress"]


STEP_WORDS = {
    "contract_written": ("contract", "plan", "Plan", 0, 0, "", "Plan"),
    "plan_approved": ("plan", "plan", "Plan", 0, 0, "", "Plan"),
    "building": ("build", "build", "Build", 2, 2, "tasks", "Build, 2 of 2 tasks"),
    "checks": ("evaluate", "checks", "Checks", 0, 3, "passed", "Checks, 0 of 3 passed"),
    "pr": ("pr", "deliver", "Deliver", 0, 0, "", "Deliver"),
    "report": ("report", "deliver", "Deliver", 0, 0, "", "Deliver"),
}


@pytest.mark.parametrize("name", STEP_WORDS)
def test_ProgressDocument_EveryPhase_SaysTheStepAndCountsWithinIt(name, repo):
    phase, step, label, done, total, unit, text = STEP_WORDS[name]
    prepare = (lambda task: (write_contract(task, approved=True), write_plan(task, approved=True), merged(task)))

    document = progress_document(repo, phase, prepare)

    assert (document["step"], document["phase"], document["done"], document["total"], document["unit"],
            document["text"]) == (step, label, done, total, unit, text)


def test_ProgressDocument_KeepsTheDetailedCountsInTheirOwnFields(repo):
    prepare = (lambda task: (write_contract(task, approved=True), write_plan(task, approved=True), merged(task)))

    document = progress_document(repo, "build", prepare)

    assert (document["engine_phase"], document["tasks_done"], document["tasks_total"], document["checks_done"],
            document["checks_total"], document["overall_done"], document["overall_total"]) == ("build", 2, 2, 0, 3, 2, 5)


@pytest.mark.parametrize("phase", ["contract", "pr", "build"])
def test_ProgressDocument_NoStepTextHasAnEngineWord(repo, phase):
    prepare = (lambda task: (write_contract(task, approved=True), write_plan(task, approved=True), merged(task)))
    text = progress_document(repo, phase, prepare)["text"]
    assert not any(word in text.lower() for word in ("contract", "row", "pr ", "pull request"))
