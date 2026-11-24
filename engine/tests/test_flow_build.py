"""Build phase: parallel builders, code review loop, ordered merge and the full test run."""

import json
import re
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura import slots
from sakura.flow import config as flow_config
from sakura.flow import final_head, flowfiles, plan, workspaces
from sakura.flow.phases import build, build_task, integrate

from conftest import git
from flow_support import engine_flows, make_context

APPROVE = {"verdict": "APPROVE", "findings": []}
REVISE = {"verdict": "REVISE", "findings": ["rename the helper"]}
WAIT_SECONDS = 20


@pytest.fixture(autouse=True)
def no_hook_install(monkeypatch):
    """Builds here commit with --no-verify; skipping `sakura install` keeps worktree creation fast."""
    monkeypatch.setattr(workspaces, "_prepare", lambda ctx, name, path: None)


def _task(task_id, depends_on=()):
    return {"id": task_id, "title": f"title {task_id}", "depends_on": list(depends_on), "rows": ["R1"],
            "files": [f"{task_id.lower()}.txt"], "seam": False}


def _mechanical(task_id, depends_on=()):
    """A task whose class never reaches the Opus gate, so failure rounds stay on the dial's fix-round limit."""
    return {**_task(task_id, depends_on), "task_class": "mechanical"}


def _ctx(repo, tmp_path, tasks, dial="balanced", review=True, full=None, builders=None):
    base = git(repo, "rev-parse", "HEAD")
    overrides = {"dial": dial, "workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []},
                 "tests": {"full": full or []}}
    if builders:
        overrides["builders"] = builders
    state = {"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}, "build": {}, "rounds": {},
             "contract_sha256": "c" * 64}
    ctx = make_context(repo, overrides=overrides, state=state)
    object.__setattr__(ctx, "steps", flow_config.PresetSteps(True, review, True))
    workspaces.create(ctx, "0", "feat/flow-demo", base)
    ctx.store.plan_md.write_text("# Plan\n")
    ctx.store.plan_json.write_text(json.dumps({"tasks": tasks}))
    digest = plan.digest(ctx.store)
    ctx.store.update_state(lambda state: state.update(plan_sha256=digest))
    return ctx


def _zero(ctx) -> Path:
    return Path(ctx.store.read_state()["workspaces"]["0"]["path"])


class BuildRunner:
    """Builder runs call `builder(request)`; reviewer runs pop queued results."""

    def __init__(self, builder, reviews=None):
        self.builder, self.reviews = builder, list(reviews or [])
        self.requests, self.on_review, self.flags = [], None, {}
        self.builder_result = None
        self.final = ""  # what every run's last message says
        self._lock = threading.Lock()

    def run(self, request):
        with self._lock:
            self.requests.append(request)
        result = None
        if request.role == "builder":
            self.builder(request)
            result = self.builder_result
        elif request.role == "code_reviewer":
            with self._lock:
                result = self.reviews.pop(0) if self.reviews else APPROVE
            if self.on_review:
                self.on_review(request)
        flags = {"exit_code": 0, "stalled": False, "stopped": False, **self.flags.get(request.role, {})}
        return SimpleNamespace(final=self.final, result=result, log_path=Path("run.log"), **flags)

    def of(self, role, task=None):
        return [r for r in self.requests if r.role == role
                and (task is None or r.extra_env["SAKURA_FLOW_BUILD_TASK"] == task)]


def _commit(request, name=None, text=None):
    task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
    count = len(list(request.cwd.glob(f"{task.lower()}-*.txt")))
    name = name or f"{task.lower()}-{count}.txt"
    (request.cwd / name).write_text(text or f"{task} {count}\n")
    git(request.cwd, "add", name)
    git(request.cwd, "commit", "--no-verify", "-qm", f"feat({task.lower()}): {name}")


def _use(ctx, builder, reviews=None):
    runner = BuildRunner(builder, reviews)
    object.__setattr__(ctx, "runner", runner)
    return runner


def _succeeds(ctx):
    outcome = build.run(ctx)
    assert outcome.kind == "next", outcome.reason


def _merge_subjects(ctx):
    return git(_zero(ctx), "log", "--first-parent", "--reverse", "--format=%s").splitlines()


def test_ready_tasks_wait_for_dependencies(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2", ["T1"])])
    seen = {}

    def builder(request):
        task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
        seen[task] = sorted(p.name for p in request.cwd.glob("t*.txt"))
        _commit(request)

    _use(ctx, builder)
    _succeeds(ctx)
    assert seen["T1"] == [] and seen["T2"] == ["t1-0.txt"]
    state = ctx.store.read_state()["build"]
    assert {t: state[t]["status"] for t in state} == {"T1": "merged", "T2": "merged"}
    assert state["T2"]["workspace"] == "2" and state["T2"]["branch"] == "feat/flow-demo-t2"


@pytest.mark.parametrize("dial,builders,expected", [
    ("save", {"min": 1, "max": 4}, 1), ("balanced", {"min": 1, "max": 4}, 3), ("power", {"min": 1, "max": 4}, 4),
    ("save", {"min": 2, "max": 4}, 2), ("power", {"min": 1, "max": 2}, 2)])
def test_worker_count_follows_dial_and_bounds(dial, builders, expected):
    assert build.worker_count({"dial": dial, "builders": builders}) == expected


@pytest.mark.parametrize("builders,lanes,expected", [(1, 6, 1), (3, 1, 3), (6, 1, 4), (2, 5, 2)])
def test_Presets_Builders_DriveBuildConcurrency(builders, lanes, expected):
    knobs = {"builders": builders, "lanes": lanes}
    assert build.worker_count({"dial": "balanced", "builders": {"min": 1, "max": 4}, "knobs": knobs}) == expected


def test_independent_builders_overlap_in_own_worktrees_with_own_ports(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task(f"T{n}") for n in range(1, 5)], dial="lean", review=False)
    barrier, active, lock, seen = threading.Barrier(2, timeout=WAIT_SECONDS), [0], threading.Lock(), []

    def builder(request):
        with lock:
            active[0] += 1
            seen.append((active[0], str(request.cwd), request.extra_env["SAKURA_FLOW_PORT"],
                         request.extra_env["SAKURA_FLOW_PORTS"]))
        barrier.wait()
        _commit(request)
        with lock:
            active[0] -= 1

    _use(ctx, builder)
    _succeeds(ctx)
    assert max(count for count, *_ in seen) == 2
    assert len({cwd for _, cwd, *_ in seen}) == 4 and len({port for _, _, port, _ in seen}) == 4
    assert all(re.fullmatch(r"\d+-\d+", ports) for *_, ports in seen)


def test_merge_follows_plan_order_when_later_task_finishes_first(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)
    second_done = threading.Event()

    def builder(request):
        if request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T1":
            assert second_done.wait(WAIT_SECONDS)
        _commit(request)
        second_done.set()

    _use(ctx, builder)
    _succeeds(ctx)
    merges = [s for s in _merge_subjects(ctx) if s.startswith("flow: merge")]
    assert merges == ["flow: merge T1 title T1", "flow: merge T2 title T2"]


def test_builder_without_commit_counts_as_failed_round(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    runner = _use(ctx, lambda request: None if len(runner.of("builder")) == 1 else _commit(request))
    _succeeds(ctx)
    assert "builder made no commit" in runner.of("builder")[1].values["findings"]
    assert ctx.store.read_state()["build"]["T1"]["rounds"] == 2


def test_rounds_beyond_the_dial_block_as_escalation(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_mechanical("T1")], dial="lean", review=False)
    runner = _use(ctx, lambda request: None)
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "builder made no commit" in outcome.reason
    assert len(runner.of("builder")) == flow_config.DIAL["lean"].fix_rounds + 1


def test_failed_focused_tests_count_as_failed_round(repo, tmp_path, monkeypatch):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    codes = iter([1, 0])
    monkeypatch.setattr(build_task.testing, "run", lambda path, suite, **_kwargs: next(codes))
    runner = _use(ctx, _commit)
    _succeeds(ctx)
    assert "focused tests failed" in runner.of("builder")[1].values["findings"]


def test_review_revise_reruns_builder_with_findings(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, _commit, [REVISE, APPROVE])
    _succeeds(ctx)
    assert "rename the helper" in runner.of("builder")[1].values["findings"]
    assert "rename the helper" not in runner.of("builder")[0].values["findings"]
    review = runner.of("code_reviewer")[0]
    # Review 11: the reviewer works in a sealed copy of its own, never in the builder's workspace.
    assert review.phase == "review" and review.cwd != runner.of("builder")[0].cwd and review.repository is not None
    assert "git diff" in review.template and review.values["start"]
    ledger = ctx.store.ledger.read_text()
    assert re.search(r"(?i)review[^\n]*round[ :=]*2", ledger)
    entries = [h for h in ctx.store.read_state()["history"] if h["phase"] == "review"]
    assert [h["status"] for h in entries] == ["revise", "approve"]


def test_review_without_result_counts_as_revise(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, _commit, [None, APPROVE])
    _succeeds(ctx)
    assert "agent returned no result" in runner.of("builder")[1].values["findings"]


def test_reviewer_changing_the_worktree_blocks(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, _commit)
    runner.on_review = lambda request: (request.cwd / "stray.txt").write_text("x\n")
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "should only read the project" in outcome.reason and "while reviewing the code" in outcome.reason  # plain words (round 6, N28)


def test_merge_conflict_blocks_naming_files_and_leaves_zero_clean(repo, tmp_path):
    # Requirement changed (round 6, N22): a conflict between tasks first gets a builder round (`test_flow_build_conflict.py`);
    # this builder gives the round up (it aborts the merge and commits its own file again), so the lane blocks as before.
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)

    def stubborn(request):
        task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
        again = Path(request.cwd, git(request.cwd, "rev-parse", "--git-path", "MERGE_HEAD")).exists()
        if again:
            git(request.cwd, "merge", "--abort")
        _commit(request, "shared.txt", f"{task}{' again' if again else ''}\n")

    _use(ctx, stubborn)
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "shared.txt" in outcome.reason and "T2" in outcome.reason
    assert flowfiles.uncommitted(_zero(ctx)) == []
    merge_head = Path(_zero(ctx), git(_zero(ctx), "rev-parse", "--git-path", "MERGE_HEAD"))
    assert not merge_head.exists()
    state = ctx.store.read_state()["build"]
    assert state["T1"]["status"] == "merged" and state["T2"]["status"] == "approved"


def test_resume_skips_merged_tasks(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _mechanical("T2")], dial="save", review=False)
    _use(ctx, lambda request: _commit(request) if request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T1" else None)
    assert build.run(ctx).kind == "blocked"
    assert ctx.store.read_state()["build"]["T1"]["status"] == "merged"
    resumed = _use(ctx, _commit)
    _succeeds(ctx)
    assert [r.extra_env["SAKURA_FLOW_BUILD_TASK"] for r in resumed.of("builder")] == ["T2"]


def test_resume_of_an_approved_task_merges_without_rebuilding(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    _use(ctx, _commit)
    _succeeds(ctx)
    ctx.store.update_state(lambda state: state["build"]["T1"].update(status="approved"))
    resumed = _use(ctx, _commit)
    _succeeds(ctx)
    assert resumed.of("builder") == []
    assert ctx.store.read_state()["build"]["T1"]["status"] == "merged"


def test_builder_environment_and_prompt_values(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    runner = _use(ctx, _commit)
    build.run(ctx)
    request = runner.of("builder")[0]
    assert request.phase == "build" and request.extra_env["SAKURA_FLOW_BUILD_TASK"] == "T1"
    assert request.extra_env["SAKURA_FLOW_PORT"] and request.values["task_id"] == "T1"
    assert "sakura test focused" in request.template


def test_stopped_builder_stops_the_phase(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, lambda request: None)
    runner.flags["builder"] = {"stopped": True}
    assert build.run(ctx).kind == "stopped"


def test_stalled_builder_blocks(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, lambda request: None)
    runner.flags["builder"] = {"stalled": True}
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and outcome.reason.startswith("stalled: build")


def test_green_full_tests_record_the_tested_head_and_print_skips(repo, tmp_path, capsys):
    command = "echo 'SKIPPED [1] tests/a.py:3: needs db'; echo '1 skipped'"
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=[command])
    _use(ctx, _commit)
    _succeeds(ctx)
    state = ctx.store.read_state()
    assert state["full_green_head"] == git(_zero(ctx), "rev-parse", "HEAD")
    assert state["full_green_inputs_sha256"] == integrate.full_tests_inputs_hash(ctx)
    assert "skipped: tests/a.py: needs db" in capsys.readouterr().out
    assert "1 skipped" in (ctx.store.runs_dir / "operations" / "full-tests.log").read_text()
    assert not list(ctx.store.runs_dir.glob("*.log"))


def test_red_full_tests_block_with_gate_red(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=["echo broken; exit 1"])
    _use(ctx, _commit)
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and outcome.event == "gate_red" and "full tests" in outcome.reason
    assert ctx.store.read_state().get("full_green_head") is None


def test_full_test_slot_timeout_blocks_as_stalled_and_records_queue_details(repo, tmp_path, monkeypatch):
    command = "python3 -m pytest -q tests"
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=[command])
    ctx.config.values["timeouts"]["build"] = 0.02
    monkeypatch.setenv("SAKURA_HEAVY_SLOTS", "1")
    queue = slots.SlotQueue(slots.default_root(), 1)

    with queue.acquire("budget-holder", False, 1):
        outcome = integrate.run_full_tests(ctx)

    full_log = ctx.store.runs_dir / "operations" / "full-tests.log"
    ledger = ctx.store.ledger.read_text()
    assert outcome.kind == "blocked" and outcome.reason == "stalled"
    assert outcome.event == "escalation"
    assert queue.snapshot().waiting == []
    assert command in full_log.read_text()
    assert "tests.full 1" in full_log.read_text()
    assert "wait budget: 0.02s" in full_log.read_text()
    assert str(full_log) in ledger and command in ledger
    assert (ctx.store.runs_dir / "operations" / "full-tests-1.raw").is_file()
    assert not list(ctx.store.runs_dir.glob("*.log"))


@pytest.mark.parametrize(
    ("stalled", "timed_out", "label"),
    [(False, True, "timed out"), (True, False, "stalled")],
)
def test_full_test_execution_timeouts_keep_diagnostics(repo, tmp_path, stalled, timed_out, label):
    command = "python3 -m pytest -q tests"
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=[command])
    log = ctx.store.runs_dir / "operations" / "full-tests.log"
    outcome = integrate._failure_of(
        ctx,
        SimpleNamespace(exit_code=None, stalled=stalled, timed_out=timed_out, stopped=False),
        command,
        log,
    )

    assert outcome.reason == f"{label}: full tests ({command}); see {log}"


def test_inputs_hash_binds_plan_contract_and_base(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    first = build.inputs_hash(ctx)
    ctx.store.update_state(lambda state: state.update(contract_sha256="d" * 64))
    assert build.inputs_hash(ctx) != first


def _survivor_commit(path, name="survivor.txt"):
    """What a detached survivor of a run can do in a folder its cage wrote: add a commit and move the branch."""
    (path / name).write_text("never reviewed\n")
    git(path, "add", name)
    git(path, "commit", "--no-verify", "-qm", f"survivor: {name}")
    return git(path, "rev-parse", "HEAD")


def _engine_tree(ctx):
    """The files of the engine's own head, read from its store."""
    store, head = final_head.proved(ctx)
    return set(git(store, "ls-tree", "-r", "--name-only", head).splitlines())


def _around_merges(monkeypatch, before=None, after=None):
    """Let a survivor act just before or just after each of the engine's merges."""
    real = build.final_head.merge

    def merge(ctx, label, title, approved):
        if before:
            before(ctx)
        head = real(ctx, label, title, approved)
        if after:
            after(ctx)
        return head

    monkeypatch.setattr(build.final_head, "merge", merge)


def test_Build_ACommitAddedAfterApproval_IsIgnoredAndOnlyTheApprovedCommitIsMerged(repo, tmp_path, monkeypatch):
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    _use(ctx, _commit)

    def survivor(context):
        _survivor_commit(Path(context.store.read_state()["workspaces"]["1"]["path"]), "late.txt")
        _survivor_commit(_zero(context))

    _around_merges(monkeypatch, before=survivor)
    _succeeds(ctx)
    state = ctx.store.read_state()
    store, head = final_head.proved(ctx)
    assert git(store, "rev-parse", f"{head}^2") == state["build"]["T1"]["approved_head"]
    assert git(store, "rev-parse", f"{head}^1") == state["base_commit"]
    assert "t1-0.txt" in _engine_tree(ctx) and not {"late.txt", "survivor.txt"} & _engine_tree(ctx)
    assert git(_zero(ctx), "rev-parse", "HEAD") == head and not (_zero(ctx) / "survivor.txt").exists()


def test_Build_ADependentTaskStartsAtTheEnginesHead_NotAMovedWorkspaceZero(repo, tmp_path, monkeypatch):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2", ["T1"])], review=False)
    seen = {}

    def builder(request):
        seen[request.extra_env["SAKURA_FLOW_BUILD_TASK"]] = sorted(p.name for p in request.cwd.glob("*.txt"))
        _commit(request)

    _use(ctx, builder)
    _around_merges(monkeypatch, after=lambda context: _survivor_commit(_zero(context)))
    _succeeds(ctx)
    state = ctx.store.read_state()
    first = state["integration"]["steps"][0]["head"]
    assert state["build"]["T2"]["start"] == first
    assert seen["T2"] == ["t1-0.txt"]
    assert "survivor.txt" not in _engine_tree(ctx)


def test_Build_TheFullTestsRunOnTheEnginesHead_NotOnAMovedTaskBranch(repo, tmp_path, monkeypatch):
    check = "test -e t1-0.txt && test ! -e survivor.txt"
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=[check])
    _use(ctx, _commit)
    _around_merges(monkeypatch, after=lambda context: _survivor_commit(_zero(context)))
    _succeeds(ctx)
    assert ctx.store.read_state()["full_green_head"] == final_head.proved(ctx)[1]


def test_merged_commit_equals_approved_head(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    _use(ctx, _commit)
    _succeeds(ctx)
    approved = ctx.store.read_state()["build"]["T1"]["approved_head"]
    assert approved == git(_zero(ctx), "rev-parse", "HEAD^2")


def test_failed_focused_tests_send_the_builder_the_engines_output_tail(repo, tmp_path, monkeypatch):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    output = "\n".join(f"line {number}" for number in range(100)) + "\n\x1b[31mAssertionError: expected 1\x1b[0m\n"
    results = iter([(1, output), (0, "")])
    monkeypatch.setattr(build_task.process, "run_test_command", lambda *_args, **_kwargs: next(results))
    monkeypatch.setattr(build_task.testing, "run", lambda path, suite, command_runner=None: (
        0 if command_runner("pytest", path)[0] == 0 else 1))
    runner = _use(ctx, _commit)

    _succeeds(ctx)

    findings = runner.of("builder")[1].values["findings"]
    assert "focused tests failed" in findings and "The engine's output, last lines:" in findings
    assert "AssertionError: expected 1" in findings and "\x1b" not in findings
    assert "line 99" in findings and "line 10\n" not in findings


def test_no_full_test_command_is_said_plainly_in_the_ledger_and_the_state(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    _use(ctx, _commit)

    _succeeds(ctx)

    assert ctx.store.read_state()["full_tests"] == "not configured"
    ledger = ctx.store.ledger.read_text()
    assert "tests.full is empty" in ledger and ".sakura/flow.toml" in ledger


def test_green_full_tests_are_recorded_as_passed(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False, full=["true"])
    _use(ctx, _commit)

    _succeeds(ctx)

    assert ctx.store.read_state()["full_tests"] == "passed"


def _rewrite_loose_object(path, claimed, content):
    """What a survivor can do in the task's object database: the loose file of `claimed` holds other bytes."""
    import zlib

    objects = Path(git(path, "rev-parse", "--path-format=absolute", "--git-common-dir")) / "objects"
    target = objects / claimed[:2] / claimed[2:]
    target.chmod(0o644)
    target.write_bytes(zlib.compress(b"blob %d\0" % len(content) + content))


def test_Review_TheReviewerReadsTheSealedCommitsInARepositoryOfItsOwn_NotTheTaskRepository(repo, tmp_path):
    # Review 11: what a reviewer approves is what is integrated. Its copy comes from the engine's store, so a
    # survivor rewriting the task's objects during the review changes nothing it sees, and it reads nothing else.
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    runner = _use(ctx, lambda request: _commit(request, "t1.txt", "the builder's change\n"))
    seen = {}

    def review(request):
        lane = Path(ctx.store.read_state()["workspaces"]["1"]["path"])
        head = request.values["branch"]
        _rewrite_loose_object(lane, git(lane, "rev-parse", f"{head}:t1.txt"), b"something else\n")
        seen.update(cwd=request.cwd, repository=request.repository, lane=lane,
                    shown=git(request.cwd, "show", f"{head}:t1.txt"),
                    diff=git(request.cwd, "diff", "--name-only", f"{request.values['start']}..{head}"))

    runner.on_review = review
    _succeeds(ctx)  # sealed before the review, so the rewrite reaches neither the reviewer nor the engine's head
    store, head = final_head.proved(ctx)
    assert git(store, "show", f"{head}:t1.txt") == "the builder's change"
    assert seen["cwd"] != seen["lane"] and seen["repository"] is not None
    assert seen["repository"] != Path(ctx.store.read_state()["task_repository"]["path"])
    assert seen["shown"] == "the builder's change" and seen["diff"] == "t1.txt"
    assert not seen["cwd"].exists() and not ctx.store.read_state().get("role_repositories")


def test_Build_TheContractFlowsTheEngineCommittedAreNotAProtectedChangeOfTheTasks(repo, tmp_path):
    # The flows commit is the first step of the engine's head and its files are protected for the build tasks; the
    # check over the engine's head starts after it, as each task's check does.
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    engine_flows(ctx, {"tests/test_flow_t1.py": (b"def test_flow():\n    pass\n", False)}, ["tests/**"],
                 "test(flow): contract flows for demo")
    ctx.store.update_state(lambda state: state.update(protected_paths=["tests/test_flow_t1.py"]))
    _use(ctx, _commit)
    _succeeds(ctx)
    assert "tests/test_flow_t1.py" in _engine_tree(ctx) and "t1-0.txt" in _engine_tree(ctx)


def test_Review_TheReviewerIsShownTheContractFlowsAsPartOfWhatItApproves(repo, tmp_path):
    # Review 12, finding 1: the flows ship with the change, so every review covers them.
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    flows = engine_flows(ctx, {"e2e/flow_t1.py": (b"# flow\n", False)}, ["e2e/**"],
                         "test(flow): contract flows for demo")
    runner = _use(ctx, _commit)
    _succeeds(ctx)
    review = runner.of("code_reviewer")[0]
    base = ctx.store.read_state()["base_commit"]
    assert f"git diff {base}..{flows}" in review.values["flows"]


def test_Review_ABuilderCommittedClaudeLinkToThePersonsFolder_IsRefusedAndTheFolderIsUntouched(repo, tmp_path):
    # Review 14, finding 1: preparing the reviewer's copy must not write through a link the builder committed.
    outside = tmp_path / "person-claude"
    outside.mkdir()
    (outside / "settings.local.json").write_text('{"mine": true}\n')
    ctx = _ctx(repo, tmp_path, [_task("T1")])

    def link_the_persons_folder(request):
        (request.cwd / ".claude").symlink_to(outside)
        git(request.cwd, "add", ".claude")
        git(request.cwd, "commit", "--no-verify", "-qm", "feat(t1): settings")

    runner = _use(ctx, link_the_persons_folder)
    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and ".claude" in outcome.reason
    assert [path.name for path in outside.iterdir()] == ["settings.local.json"]
    assert (outside / "settings.local.json").read_text() == '{"mine": true}\n'
    assert runner.of("code_reviewer") == []


def _flows_candidate(ctx):
    return final_head.flows_candidate(ctx, {"e2e/flow_t1.py": (b"# flow\n", False)}, ["e2e/**"],
                                      "test(flow): contract flows for demo")


def test_RecordFlows_WithoutTheCodeReviewersApprovalOfThatCommit_IsRefused(repo, tmp_path):
    # Review 13, finding 1: the flows commit never enters the engine's head unreviewed, whatever the tasks do.
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    candidate = _flows_candidate(ctx)

    with pytest.raises(final_head.MergeBlocked, match="no code review approval"):
        final_head.record_flows(ctx, candidate, ["e2e/**"])
    ctx.store.update_state(lambda state: state.update({final_head.FLOWS_REVIEW_KEY: {
        "base": state["base_commit"], "head": state["base_commit"], "verdict": "APPROVE"}}))
    with pytest.raises(final_head.MergeBlocked, match="no code review approval"):
        final_head.record_flows(ctx, candidate, ["e2e/**"])
    assert final_head.recorded(ctx.store.read_state()) == ctx.store.read_state()["base_commit"]

    final_head.approve_flows(ctx, candidate)
    assert final_head.record_flows(ctx, candidate, ["e2e/**"]) == candidate
    assert final_head.proved(ctx)[1] == candidate


def test_RecordFlows_WithCodeReviewOffInThePreset_RecordsThatNoReviewRan(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    candidate = _flows_candidate(ctx)

    assert final_head.record_flows(ctx, candidate, ["e2e/**"]) == candidate
    step = ctx.store.read_state()[final_head.STATE_KEY]["steps"][0]
    assert step["review"] == final_head.REVIEW_OFF


def test_Build_ATaskWithNoChange_StillNeedsTheFlowsReviewReceipt(repo, tmp_path):
    # A flows step recorded without its review (a forged or older state) proves nothing, so even a build whose only
    # task needs no change and is never code-reviewed delivers nothing from it.
    ctx = _ctx(repo, tmp_path, [_task("T1")])
    engine_flows(ctx, {"e2e/flow_t1.py": (b"# flow\n", False)}, ["e2e/**"])
    ctx.store.update_state(lambda state: state[final_head.STATE_KEY]["steps"][0].pop("review"))
    runner = _use(ctx, lambda _request: None)
    runner.builder_result = {"no_change": "the repository already does what the task asks"}

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and "code review approval" in outcome.reason
    assert runner.of("code_reviewer") == []
    with pytest.raises(final_head.HeadRefused):
        final_head.proved(ctx)


def test_Build_AFlowFileABuildTaskChanged_IsAProtectedChangeOfTheEnginesHead(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    engine_flows(ctx, {"e2e/flow_t1.py": (b"# flow\n", False)}, ["e2e/**"], "test(flow): contract flows for demo")
    ctx.store.update_state(lambda state: state.update(protected_paths=["e2e/flow_t1.py"]))
    _use(ctx, lambda request: _commit(request, "e2e/flow_t1.py", "# weakened\n"))
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "e2e/flow_t1.py" in outcome.reason
