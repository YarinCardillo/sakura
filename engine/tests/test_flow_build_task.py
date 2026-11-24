"""Build task: the builder is routed by task class, and an Opus escalation waits at the approve gate."""

import json

import pytest

from sakura.flow import plan
from sakura.flow.phases import build, build_task

from test_flow_build import _commit, _ctx, _use


def _task(task_id="T1", task_class=None):
    task = {"id": task_id, "title": f"title {task_id}", "depends_on": [], "rows": ["R1"],
            "files": [f"{task_id.lower()}.txt"], "seam": False}
    return {**task, "task_class": task_class} if task_class else task


def _never_commits(ctx, task_class="integration", dial="balanced"):
    ctx = _ctx(*ctx, [_task(task_class=task_class)], dial=dial, review=False)
    return ctx, _use(ctx, lambda request: None)


@pytest.fixture
def args(repo, tmp_path):
    return repo, tmp_path


def test_builder_request_carries_the_task_class_and_no_failures(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], review=False)
    runner = _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    request = runner.of("builder")[0]
    assert (request.task_class, request.failures, request.opus_approved) == ("mechanical", 0, False)


def test_failed_rounds_are_counted_and_sent_to_the_next_builder_run(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None if len(runner.of("builder")) < 3 else _commit(request))
    assert build.run(ctx).kind == "next"
    assert [r.failures for r in runner.of("builder")] == [0, 1, 2]
    assert ctx.store.read_state()["build"]["T1"]["failures"] == 2


def test_two_failed_rounds_of_an_integration_task_wait_at_the_opus_gate(args):
    ctx, runner = _never_commits(args)
    outcome = build.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "opus:T1")
    assert "T1" in outcome.reason and "opus" in outcome.reason and "2 failed review rounds" in outcome.reason
    assert len(runner.of("builder")) == 2


def test_gate_reason_ends_with_the_estimate_text(args, monkeypatch):
    ctx, _ = _never_commits(args)
    monkeypatch.setattr(build_task, "_estimate_text", lambda ctx, task, route: " Extra: about 4% of the week.")
    assert build.run(ctx).reason.endswith(" Extra: about 4% of the week.")


def test_an_approved_opus_gate_lets_that_task_run_the_escalated_builder(args):
    ctx, runner = _never_commits(args)
    build.run(ctx)
    ctx.store.update_state(lambda state: state["approvals"].update(
        {"opus:T1": {"at": "now", "plan_sha256": state["plan_sha256"]}}))
    build.run(ctx)
    request = runner.of("builder")[2]
    assert (request.failures, request.opus_approved) == (2, True)


def test_approving_one_task_does_not_unlock_another(args):
    ctx = _ctx(*args, [_task("T1"), _task("T2")], dial="balanced", review=False, builders={"min": 1, "max": 1})
    runner = _use(ctx, lambda request: None)
    ctx.store.update_state(lambda state: state["approvals"].update({"opus:T2": {"at": "now"}}))
    outcome = build.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "opus:T1")
    assert not any(r.opus_approved for r in runner.of("builder"))


def test_a_mechanical_task_escalates_to_sonnet_without_a_gate(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="balanced", review=False)
    runner = _use(ctx, lambda request: None)
    outcome = build.run(ctx)
    assert outcome.kind == "blocked"
    assert [r.failures for r in runner.of("builder")] == [0, 1, 2]


def test_an_opus_approval_of_another_plan_is_ignored(args):
    ctx, runner = _never_commits(args)
    ctx.store.update_state(lambda state: state["approvals"].update({"opus:T1": {"at": "now", "plan_sha256": "old"}}))
    outcome = build.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "opus:T1")
    assert not any(r.opus_approved for r in runner.of("builder"))


def test_an_opus_approval_without_a_plan_digest_is_ignored(args):
    ctx, runner = _never_commits(args)
    ctx.store.update_state(lambda state: state["approvals"].update({"opus:T1": {"at": "now"}}))
    assert build.run(ctx).gate == "opus:T1"
    assert not any(r.opus_approved for r in runner.of("builder"))


def test_a_replanned_task_with_a_reused_id_starts_with_no_failures_and_no_approval(args):
    ctx, runner = _never_commits(args)
    build.run(ctx)
    ctx.store.update_state(lambda state: state["approvals"].update(
        {"opus:T1": {"at": "now", "plan_sha256": state["plan_sha256"]}}))
    ctx.store.plan_json.write_text(json.dumps({"tasks": [{**_task(task_class="integration"), "title": "another T1"}]}))
    digest = plan.digest(ctx.store)
    ctx.store.update_state(lambda state: state.update(plan_sha256=digest))

    outcome = build.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "opus:T1")
    assert [r.failures for r in runner.of("builder")][-2:] == [0, 1], "the old failures do not carry over"
    assert not any(r.opus_approved for r in runner.of("builder"))


def test_a_merged_task_keeps_its_record_when_the_plan_changes(args):
    ctx = _ctx(*args, [_task()], review=False)
    _use(ctx, _commit)
    assert build.run(ctx).kind == "next"
    ctx.store.update_state(lambda state: state["build"]["T1"].update(failures=1))
    ctx.store.plan_json.write_text(json.dumps({"tasks": [{**_task(), "title": "renamed"}]}))
    digest = plan.digest(ctx.store)
    ctx.store.update_state(lambda state: state.update(plan_sha256=digest))
    build._reset(ctx, plan.load(ctx.store, digest))
    assert ctx.store.read_state()["build"]["T1"]["failures"] == 1


def _declares_no_change(runner, reason="tests/test_app.py::test_greeting already covers it"):
    runner.builder_result = {"no_change": reason}


def test_a_builder_that_declares_no_change_ends_its_task_without_a_block(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], review=True)
    runner = _use(ctx, lambda request: None)
    _declares_no_change(runner)

    outcome = build.run(ctx)

    entry = ctx.store.read_state()["build"]["T1"]
    assert outcome.kind == "next", outcome.reason
    assert entry["status"] == build_task.MERGED
    assert entry["no_change"] == "tests/test_app.py::test_greeting already covers it"
    assert len(runner.of("builder")) == 1 and runner.of("code_reviewer") == []
    assert "T1 needs no change" in ctx.store.ledger.read_text()


def test_no_change_without_a_declaration_is_still_a_missing_commit(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None)

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and build_task.NO_COMMIT_FINDING in outcome.reason
    assert len(runner.of("builder")) > 1


@pytest.mark.parametrize("declared", [{"no_change": ""}, {"no_change": "   "}, {"no_change": 7}, {"no_change": None}])
def test_an_empty_or_malformed_no_change_declaration_is_a_missing_commit(args, declared):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None)
    runner.builder_result = declared

    assert build.run(ctx).kind == "blocked"


def test_no_change_with_an_uncommitted_file_is_refused(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: (request.cwd / "scratch.txt").write_text("left behind\n"))
    _declares_no_change(runner)

    assert build.run(ctx).kind == "blocked"


def test_no_change_after_an_earlier_commit_is_not_accepted(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=True)
    runner = _use(ctx, lambda request: _commit(request) if not runner.of("code_reviewer") else None,
                  reviews=[{"verdict": "REVISE", "findings": ["rename the helper"]}])
    runner.builder_result = None

    def second_round(request):
        if len(runner.of("builder")) == 1:
            _commit(request)
        else:
            runner.builder_result = {"no_change": "nothing more to do"}

    runner.builder = second_round

    outcome = build.run(ctx)

    assert outcome.kind == "blocked"
    assert ctx.store.read_state()["build"]["T1"].get("no_change", "") == ""


def _leaves(*names):
    def action(request):
        for name in names:
            (request.cwd / name).parent.mkdir(parents=True, exist_ok=True)
            (request.cwd / name).write_text("left behind\n")
    return action


@pytest.mark.parametrize("cache", ["__pycache__/mod.cpython-312.pyc", ".pytest_cache/v/cache/lastfailed",
                                   "tests/__pycache__/test_app.cpython-312.pyc", "node_modules/.cache/babel/x.json"])
def test_no_change_with_only_a_tool_cache_left_behind_still_ends_the_task(args, cache):
    ctx = _ctx(*args, [_task(task_class="mechanical")], review=True)
    runner = _use(ctx, _leaves(cache))
    _declares_no_change(runner)

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert ctx.store.read_state()["build"]["T1"]["status"] == build_task.MERGED


def test_no_change_with_a_stray_file_names_it_and_the_choices_and_not_the_caches(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, _leaves("__pycache__/mod.pyc", "notes/scratch.txt", "out.log"))
    _declares_no_change(runner)

    outcome = build.run(ctx)

    assert outcome.kind == "blocked"
    assert build_task.NO_COMMIT_FINDING in outcome.reason
    assert "notes/scratch.txt" in outcome.reason and "out.log" in outcome.reason
    assert "__pycache__" not in outcome.reason
    assert all(choice in outcome.reason for choice in ("commit them", "remove them", "stop"))


def test_no_change_with_a_changed_tracked_file_is_refused_and_the_file_is_named(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: (request.cwd / "README").write_text("edited\n"))
    _declares_no_change(runner)

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and "README" in outcome.reason


# -- a builder that made no commit because its commits were refused says why

def _refusing(ctx, reason="sakura pre-commit: commits on protected branch 'main' are refused"):
    from sakura.flow import run_records

    def behavior(request):
        name = f"build-{len(ctx.runner.of('builder'))}"
        run_records.begin(ctx.store, name, "builder", "build", "claude", "haiku", "low", "local", request.cwd, {})
        log = run_records.log_path(ctx.store, name)
        log.parent.mkdir(parents=True, exist_ok=True)
        text = (json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t", "name": "Bash",
                                                                          "input": {"command": "git commit -m x"}}]}}) + "\n"
                + json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t",
                                                                        "content": f"Exit code 1\nhint: x\n{reason}"}]}}) + "\n")
        log.write_text(text)

    return behavior


def test_a_builder_whose_commits_were_refused_gets_that_in_its_finding_and_the_block_reason(args):
    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None)
    runner.builder = _refusing(ctx)

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and build_task.NO_COMMIT_FINDING in outcome.reason
    assert "the commit check refused its commits: commits on protected branch 'main' are refused" in outcome.reason


def test_a_builder_that_made_no_commit_for_no_visible_reason_has_the_plain_finding(args):
    ctx, runner = _never_commits(args, task_class="mechanical", dial="lean")

    outcome = build.run(ctx)

    assert outcome.reason.endswith(f": {build_task.NO_COMMIT_FINDING}")


def test_the_refusal_named_is_the_one_of_this_tasks_builder_not_another_tasks(args):
    from sakura.flow import run_records

    ctx = _ctx(*args, [_task(task_class="mechanical")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None)
    refusing = _refusing(ctx, "sakura pre-commit: this task's own refusal")

    def behavior(request):
        refusing(request)
        other = request.cwd.parent / "other-lane"
        other.mkdir(exist_ok=True)
        ctx.store.update_state(lambda state: state["runs"].update(other={
            **state["runs"][f"build-{len(runner.of('builder'))}"], "name": "other", "started_at": "2099-01-01T00:00:00+00:00",
            "cwd": str(other)}))
        run_records.log_path(ctx.store, "other").write_text(
            run_records.log_path(ctx.store, f"build-{len(runner.of('builder'))}").read_text().replace("this task's own", "ANOTHER"))

    runner.builder = behavior

    outcome = build.run(ctx)

    assert "this task's own refusal" in outcome.reason and "ANOTHER" not in outcome.reason
