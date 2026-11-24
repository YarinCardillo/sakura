"""Plan model validation and the plan phase with cross-model plan review."""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import config as flow_config
from sakura.flow import contract, gitutil, plan
from sakura.flow.phases import plan as plan_phase

from conftest import git
from flow_support import make_context


def _task(task_id="T1", **changes):
    base = {"id": task_id, "title": "t", "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False}
    return plan.BuildTask(**{**base, **changes})


def _row(row_id="R1", baseline=False):
    return contract.Row(row_id, "J1", "s", "api", None, "true", baseline, False)


def test_plan_validate_requires_row_coverage():
    rows = [_row("R1"), _row("R2"), _row("B1", baseline=True)]
    errors = plan.validate([_task(rows=["R1"])], rows)
    assert any("R2" in error and "not covered" in error for error in errors)
    assert not any("B1" in error for error in errors)
    assert plan.validate([_task(rows=["R1", "R2"])], rows) == []


def test_plan_validate_rejects_missing_rows_files_duplicates_and_unknowns():
    rows = [_row("R1")]
    tasks = [_task("T1", rows=[], files=[]), _task("T1"), _task("T2", depends_on=["T9"], rows=["R7"])]
    text = "\n".join(plan.validate(tasks, rows))
    assert "duplicate task id T1" in text
    assert "T1 has no rows" in text and "T1 has no files" in text
    assert "T2 depends on unknown task T9" in text
    assert "T2 references unknown row R7" in text


def test_plan_validate_rejects_cycles():
    tasks = [_task("T1", depends_on=["T2"]), _task("T2", depends_on=["T1"], files=["b.py"])]
    assert any("cycle" in error for error in plan.validate(tasks, [_row()]))


def test_plan_validate_requires_seam_for_task_spanning_two_others():
    tasks = [_task("T1", files=["a.py"]), _task("T2", files=["b.py"], rows=["R2"]),
             _task("T3", files=["a.py", "b.py"], rows=["R3"], depends_on=["T1", "T2"])]
    rows = [_row("R1"), _row("R2"), _row("R3")]
    assert any("T3" in error and "seam" in error for error in plan.validate(tasks, rows))
    tasks[2] = _task("T3", files=["a.py", "b.py"], rows=["R3"], depends_on=["T1", "T2"], seam=True)
    assert plan.validate(tasks, rows) == []


def test_plan_review_uses_title_as_owner_for_schema_valid_seam(repo):
    tasks = [
        {"id": "T1", "title": "API builder owns route", "depends_on": [], "rows": ["R1"],
         "files": ["api.py"], "seam": False},
        {"id": "T2", "title": "Page builder owns page", "depends_on": [], "rows": ["R1"],
         "files": ["pages.py"], "seam": False},
        {"id": "T3", "title": "Integration builder owns seam", "depends_on": ["T1", "T2"],
         "rows": ["R1"], "files": ["api.py", "pages.py"], "seam": True},
    ]
    ctx, scripted = _ctx(repo, [APPROVE], plans=[tasks])

    outcome = plan_phase.run(ctx)

    assert outcome.kind == "next"
    stored = json.loads(ctx.store.plan_json.read_text())["tasks"][2]
    assert "owner" not in stored and stored["title"] == "Integration builder owns seam"
    assert plan.validate([plan.BuildTask(**task) for task in json.loads(ctx.store.plan_json.read_text())["tasks"]],
                         contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])) == []
    reviewer_prompt = next(request.template for request in scripted.requests if request.role == "plan_reviewer")
    assert "seam owner to be named in the seam task title" in reviewer_prompt


def test_plan_order_respects_dependencies():
    tasks = [_task("T3", depends_on=["T2"]), _task("T1"), _task("T2", depends_on=["T1"])]
    assert plan.order(tasks) == ["T1", "T2", "T3"]
    assert plan.order([_task("A"), _task("B")]) == ["A", "B"]


def test_plan_load_rejects_malformed_json(repo):
    ctx = make_context(repo, state={"rounds": {}, "workspaces": {}, "contract_sha256": ""})
    ctx.store.plan_json.write_text('{"tasks": [{"id": "T1"}]}')
    with pytest.raises(plan.PlanError):
        plan.load(ctx.store, None)


# -- phase ---------------------------------------------------------------

GOOD_TASKS = [{"id": "T1", "title": "t", "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False}]


class ScriptedRunner:
    """Planner writes the plan; reviewer replies with queued results."""

    def __init__(self, ctx, verdicts, plans=None):
        self.ctx, self.verdicts, self.plans = ctx, list(verdicts), list(plans or [GOOD_TASKS])
        self.requests = []
        self.flags = {}
        self.on_run = None
        self.final = ""

    def run(self, request):
        self.requests.append(request)
        result = None
        if request.role == "planner":
            tasks = self.plans.pop(0) if len(self.plans) > 1 else self.plans[0]
            self.seen_plan = self.ctx.store.plan_json.exists()
            self.ctx.store.plan_md.write_text("# Plan\n")
            self.ctx.store.plan_json.write_text(json.dumps({"tasks": tasks}))
        else:
            result = self.verdicts.pop(0)
        if self.on_run:
            self.on_run(request)
        flags = {"exit_code": 0, "stalled": False, "stopped": False, **self.flags.get(request.role, {})}
        return SimpleNamespace(final=self.final, result=result, log_path=Path("run.log"), **flags)


def _ctx(repo, verdicts, plans=None, review=True):
    ctx = make_context(repo, state={"rounds": {}, "workspaces": {}, "contract_sha256": ""})
    object.__setattr__(ctx, "steps", flow_config.PresetSteps(review, True, True))
    work = repo / "wt"
    git(repo, "init", "-q", "-b", "main", str(work))
    git(work, "config", "user.name", "T")
    git(work, "config", "user.email", "t@example.invalid")
    (work / "keep").write_text("x\n")
    git(work, "add", ".")
    git(work, "commit", "-qm", "init")
    gitutil.trust_repository(work)  # a separate repository stands in for the worktree the engine makes
    spec_hash = "a" * 64
    ctx.store.spec.write_text("# Spec\n")
    contract_hash = contract.save(ctx.store, [_row("R1"), _row("B1", baseline=True)])
    ctx.store.update_state(lambda s: s.update(
        contract_sha256=contract_hash, base_commit="abc", branch="b",
        workspaces={"0": {"path": str(repo / "wt"), "branch": "b", "ports": [1, 2]}}))
    runner = ScriptedRunner(ctx, verdicts, plans)
    object.__setattr__(ctx, "runner", runner)
    return ctx, runner


APPROVE = {"verdict": "APPROVE", "findings": []}
REVISE = {"verdict": "REVISE", "findings": ["split T1"]}


def test_plan_phase_approves_and_runs_in_workspace_zero(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    assert [(r.role, r.phase) for r in runner.requests] == [("planner", "plan"), ("plan_reviewer", "plan_review")]
    assert all(r.cwd == repo / "wt" for r in runner.requests)


def test_plan_review_without_result_counts_as_revise(repo):
    ctx, runner = _ctx(repo, [None, APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    assert "agent returned no result" in runner.requests[2].values["review_findings"]
    assert ctx.store.read_state()["rounds"]["plan_review"] == 1
    assert "agent returned no result" in ctx.store.ledger.read_text()


def test_plan_review_escalates_after_fix_rounds(repo):
    ctx, runner = _ctx(repo, [REVISE] * 5)
    outcome = plan_phase.run(ctx)
    assert outcome.kind == "blocked"
    assert outcome.reason == "plan review still asked for changes after 2 fix rounds (3 reviews): split T1"
    assert [r.role for r in runner.requests].count("plan_reviewer") == 3
    assert outcome.event == "escalation" and "notify escalation" not in ctx.store.ledger.read_text()
    assert ctx.store.read_state()["rounds"]["plan_review"] == 3


def test_plan_invalid_plan_reruns_once_then_blocks(repo):
    bad = [{**GOOD_TASKS[0], "rows": []}]
    ctx, runner = _ctx(repo, [], plans=[bad])
    outcome = plan_phase.run(ctx)
    assert outcome.kind == "blocked" and "plan invalid" in outcome.reason
    assert [r.role for r in runner.requests] == ["planner", "planner"]
    assert "has no rows" in runner.requests[1].values["previous_error"]


def test_plan_review_skipped_when_step_off(repo):
    ctx, runner = _ctx(repo, [], review=False)
    assert plan_phase.run(ctx).kind == "next"
    assert [r.role for r in runner.requests] == ["planner"]
    assert {"phase": "plan_review", "status": "skip"}.items() <= ctx.store.read_state()["history"][-1].items()


def test_plan_inputs_hash_changes_with_contract(repo):
    ctx, _ = _ctx(repo, [])
    first = plan_phase.inputs_hash(ctx)
    ctx.store.update_state(lambda s: s.update(contract_sha256="f" * 64))
    assert plan_phase.inputs_hash(ctx) != first


def test_plan_validate_reports_unknown_dependency_without_calling_it_a_cycle():
    errors = plan.validate([_task("T1", depends_on=["T9"])], [_row()])
    assert errors == ["task T1 depends on unknown task T9"]


def test_plan_validate_rejects_self_loop_and_three_cycle():
    assert any("cycle" in e for e in plan.validate([_task("T1", depends_on=["T1"])], [_row()]))
    ring = [_task("T1", depends_on=["T3"]), _task("T2", depends_on=["T1"], files=["b.py"]),
            _task("T3", depends_on=["T2"], files=["c.py"])]
    errors = plan.validate(ring, [_row()])
    assert any("cycle" in e and "T1, T2, T3" in e for e in errors)


def test_plan_binding_load_detects_tampering(repo):
    ctx, _ = _ctx(repo, [APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    digest = ctx.store.read_state()["plan_sha256"]
    assert [t.id for t in plan.load(ctx.store, digest)] == ["T1"]
    ctx.store.plan_md.write_text("# changed\n")
    with pytest.raises(plan.PlanTampered):
        plan.load(ctx.store, digest)


def test_plan_binding_is_stored_on_skip_path(repo):
    ctx, _ = _ctx(repo, [], review=False)
    assert plan_phase.run(ctx).kind == "next"
    assert ctx.store.read_state()["plan_sha256"] == plan.digest(ctx.store)


def test_plan_changed_during_review_blocks(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    runner.on_run = lambda r: ctx.store.plan_md.write_text("# sneaky\n") if r.role == "plan_reviewer" else None
    outcome = plan_phase.run(ctx)
    assert outcome.kind == "blocked" and "changed during plan review" in outcome.reason


def test_revise_keeps_previous_plan_and_findings_verbatim(repo):
    ctx, runner = _ctx(repo, [REVISE, APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    second = [r for r in runner.requests if r.role == "planner"][1]
    assert "- split T1" in second.values["review_findings"]
    assert runner.seen_plan is True


def test_resume_mid_revise_carries_persisted_findings(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    ctx.store.update_state(lambda s: s.update(plan_review_findings=["split T1"]))
    ctx.store.plan_md.write_text("# old\n")
    ctx.store.plan_json.write_text(json.dumps({"tasks": GOOD_TASKS}))
    assert plan_phase.run(ctx).kind == "next"
    assert "- split T1" in runner.requests[0].values["review_findings"]
    assert runner.seen_plan is True
    assert ctx.store.read_state()["plan_review_findings"] == []


def test_revise_persists_findings_in_state(repo):
    ctx, runner = _ctx(repo, [REVISE, REVISE, REVISE])
    plan_phase.run(ctx)
    assert ctx.store.read_state()["plan_review_findings"] == ["split T1"]


def test_malformed_reviewer_result_is_revise_with_finding(repo):
    ctx, runner = _ctx(repo, [{"verdict": "MAYBE"}, {"verdict": "REVISE", "findings": "x"}, APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    assert ctx.store.read_state()["rounds"]["plan_review"] == 2
    assert "agent returned no result" in runner.requests[-2].values["review_findings"]


@pytest.mark.parametrize("role,phase", [("planner", "plan"), ("plan_reviewer", "plan_review")])
@pytest.mark.parametrize("flag,kind,text", [({"stopped": True}, "stopped", "stopped during"),
                                            ({"exit_code": 7}, "blocked", "exited 7 during")])
def test_plan_phase_maps_run_failures(repo, role, phase, flag, kind, text):
    ctx, runner = _ctx(repo, [APPROVE])
    runner.flags[role] = flag
    outcome = plan_phase.run(ctx)
    assert outcome.kind == kind and f"{text} {phase}" in outcome.reason
    assert role.replace("_", " ") in outcome.reason or kind == "stopped"


@pytest.mark.parametrize("role", ["planner", "plan_reviewer"])
def test_agent_touching_workspace_blocks(repo, role):
    ctx, runner = _ctx(repo, [APPROVE])
    runner.on_run = lambda r: (r.cwd / "app.py").write_text("x\n") if r.role == role else None
    outcome = plan_phase.run(ctx)
    assert outcome.kind == "blocked" and "app.py" in outcome.reason and role.replace("_", " ") in outcome.reason


def test_agent_committing_in_workspace_blocks(repo):
    ctx, runner = _ctx(repo, [APPROVE])

    def commit(request):
        (request.cwd / "keep").write_text("y\n")
        git(request.cwd, "commit", "-qam", "sneak")

    runner.on_run = commit
    assert plan_phase.run(ctx).kind == "blocked"


def test_install_artifacts_do_not_count_as_changes(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    def hooks(request):
        (request.cwd / ".claude").mkdir(exist_ok=True)
        (request.cwd / ".claude/settings.local.json").write_text("{}")
    runner.on_run = hooks
    assert plan_phase.run(ctx).kind == "next"


def test_skip_history_entry_not_repeated_on_rerun(repo):
    ctx, _ = _ctx(repo, [], review=False)
    plan_phase.run(ctx)
    plan_phase.run(ctx)
    assert [h["status"] for h in ctx.store.read_state()["history"]].count("skip") == 1


def test_review_prompt_names_the_repo_path(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    plan_phase.run(ctx)
    review = runner.requests[1]
    assert review.values["repo_path"] == str(repo / "wt")
    assert "$repo_path" in runner.requests[1].template


def test_failure_outcome_maps_agent_run_failures():
    from sakura.flow.phases import runs

    def result(**changes):
        return SimpleNamespace(**{"exit_code": 0, "stalled": False, "stopped": False, "log_path": Path("r.log"), **changes})

    assert runs.failure_outcome(result(), "planner", "plan") is None
    assert runs.failure_outcome(result(stopped=True), "planner", "plan").kind == "stopped"
    stalled = runs.failure_outcome(result(stalled=True), "plan_reviewer", "plan_review")
    assert stalled.kind == "blocked" and stalled.reason == "stalled: plan_review; see r.log"
    failed = runs.failure_outcome(result(exit_code=3), "planner", "intake")
    assert failed.reason == "planner exited 3 during intake; see r.log"


def test_failure_outcome_blocks_a_role_that_says_it_could_not_read_its_inputs():
    from sakura.flow.phases import runs

    said = {"error": "cannot read input.md"}
    result = SimpleNamespace(exit_code=0, stalled=False, stopped=False, log_path=Path("r.log"), result=said)
    outcome = runs.failure_outcome(result, "planner", "intake")
    assert outcome.kind == "blocked"
    assert outcome.reason == "planner could not do its job in intake: cannot read input.md; see r.log"
    for ignored in ({"error": ""}, {"error": 3}, {"verdict": "APPROVE"}, None):
        assert runs.failure_outcome(SimpleNamespace(**{**vars(result), "result": ignored}), "planner", "intake") is None


@pytest.mark.parametrize("error_json", ['"none"', '" None "', '""', '" "', "null"])
def test_failure_outcome_accepts_no_error_in_result_line(error_json):
    from sakura.flow import roles
    from sakura.flow.phases import runs

    said = roles.parse_result(f'SAKURA_RESULT {{"error": {error_json}}}')
    assert "error" in said
    result = SimpleNamespace(exit_code=0, stalled=False, stopped=False, log_path=Path("r.log"), result=said)
    assert runs.failure_outcome(result, "planner", "intake") is None


def test_plan_phase_stall_maps_to_stalled_plan_review(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    runner.flags["plan_reviewer"] = {"stalled": True}
    outcome = plan_phase.run(ctx)
    assert outcome.kind == "blocked" and outcome.reason.startswith("stalled: plan_review") and outcome.event == "escalation"


def test_plan_and_plan_review_prompts_agree_on_the_seam_owner_format():
    # Real Haiku runs looped: the planner named a role as owner, the reviewer refused a role as "not a named owner".
    prompts = Path(__file__).parents[1] / "lib" / "sakura" / "flow" / "prompts"
    owner = "(owner: builder of T3)"
    assert owner in (prompts / "plan.md").read_text()
    review = (prompts / "plan_review.md").read_text()
    assert owner in review and "a role is the owner" in review


# -- task classes --------------------------------------------------------

def test_old_plan_without_class_is_integration(repo):
    ctx = make_context(repo, state={"rounds": {}, "workspaces": {}, "contract_sha256": ""})
    ctx.store.plan_json.write_text(json.dumps({"tasks": GOOD_TASKS}))
    loaded = plan.load(ctx.store, None)[0]
    assert (loaded.task_class, loaded.class_reason) == ("integration", "")


def test_plan_reads_task_class_and_reason(repo):
    ctx = make_context(repo, state={"rounds": {}, "workspaces": {}, "contract_sha256": ""})
    entry = {**GOOD_TASKS[0], "task_class": "design", "class_reason": "new layout"}
    ctx.store.plan_json.write_text(json.dumps({"tasks": [entry]}))
    loaded = plan.load(ctx.store, None)[0]
    assert (loaded.task_class, loaded.class_reason) == ("design", "new layout")


def test_plan_validate_rejects_an_unknown_task_class():
    errors = plan.validate([_task(task_class="trivial")], [_row()])
    assert any("T1" in error and "trivial" in error for error in errors)


def test_planner_and_reviewer_prompts_agree_on_task_classes():
    from sakura.flow import runner
    planner, reviewer = runner.load_template("plan"), runner.load_template("plan_review")
    for name in ("mechanical", "integration", "design"):
        assert name in planner
    assert '"task_class"' in planner and '"class_reason"' in planner
    assert "task_class" in reviewer


# -- the plan step off: the lane builds from its goal ---------------------------------------------------

def test_plan_phase_with_the_plan_step_off_makes_one_task_from_the_goal_and_runs_no_agent(repo):
    ctx, runner = _ctx(repo, [])
    ctx.config.values["knobs"]["plan"] = False
    ctx.store.spec.write_text("# Add the export button\n\nDetails follow.\n")

    outcome = plan_phase.run(ctx)

    assert outcome.kind == "next" and runner.requests == []
    state = ctx.store.read_state()
    tasks = plan.load(ctx.store, state["plan_sha256"])
    assert [(task.id, task.title, task.rows) for task in tasks] == [("T1", "Add the export button", ["R1"])]
    assert plan.validate(tasks, contract.load(ctx.store, state["contract_sha256"])) == []
    assert {"phase": "plan_review", "status": "skip"}.items() <= state["history"][-1].items()
    assert "[plan] skip: the plan step is off" in ctx.store.ledger.read_text()


def test_plan_phase_with_the_plan_step_off_names_the_task_after_the_goal_when_the_spec_has_no_words(repo):
    ctx, _ = _ctx(repo, [])
    ctx.config.values["knobs"]["plan"] = False
    ctx.store.spec.write_text("\n\n")
    assert plan_phase.run(ctx).kind == "next"
    assert plan.load(ctx.store, None)[0].title == plan.GOAL_TITLE


def test_plan_TheWriterEndsWithoutWritingThePlan_SaysSoInPlainWordsWithItsOwnLastMessage(repo):
    ctx, runner = _ctx(repo, [APPROVE])
    runner.final = "The spec is too thin to split into tasks."

    def writes_nothing(request):
        ctx.store.plan_md.unlink(missing_ok=True)
        ctx.store.plan_json.unlink(missing_ok=True)

    runner.on_run = writes_nothing

    outcome = plan_phase.run(ctx)

    assert outcome.kind == "blocked"
    assert outcome.reason == ("The planner ended without writing the plan. It said: \"The spec is too thin to split into "
                              "tasks.\" Resume the lane to ask it again, or stop it.")
    assert "plan invalid after 2 attempts: plan.md was not written" in ctx.store.ledger.read_text()
