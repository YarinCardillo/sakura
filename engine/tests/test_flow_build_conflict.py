"""Two tasks of one lane that change the same file: the plan's shared files keep them apart, and a conflict that still happens
is work for a builder (one round on its own), then an ask the person can answer."""

from pathlib import Path

from sakura.flow import final_head, waits
from sakura.flow.phases import build

from conftest import git
from test_flow_build import _ctx, _engine_tree, _task, _use, no_hook_install  # noqa: F401  (the fixture is autouse)

SHARED = "shared.txt"


def _in_a_merge(workspace: Path) -> bool:
    return (workspace / git(workspace, "rev-parse", "--git-path", "MERGE_HEAD")).exists()


def _write_and_commit(workspace: Path, text: str, message: str) -> None:
    (workspace / SHARED).write_text(text)
    git(workspace, "add", SHARED)
    git(workspace, "commit", "--no-verify", "-qm", message)


class Writers:
    """Builders that each create `shared.txt` with their own words; a builder started in the middle of a merge resolves it
    (keeping both) when `resolve` says so, else commits its own words over the merge's other side again."""

    def __init__(self, resolve=lambda round_number: True):
        self.resolve, self.rounds, self.findings = resolve, {}, {}

    def __call__(self, request):
        task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
        self.rounds[task] = self.rounds.get(task, 0) + 1
        self.findings.setdefault(task, []).append(request.values["findings"])
        if _in_a_merge(request.cwd):
            if not self.resolve(self.rounds[task]):
                git(request.cwd, "merge", "--abort")
                _write_and_commit(request.cwd, f"{task} again, on its own\n", f"{task}: again")
                return
            theirs = git(request.cwd, "show", "refs/sakura/integration:" + SHARED)
            _write_and_commit(request.cwd, f"{theirs}and {task}\n", f"{task}: both kept")
            return
        _write_and_commit(request.cwd, f"{task} wrote this\n", f"{task}: {SHARED}")


def test_Build_TwoTasksThatListTheSameFile_RunOneAfterTheOther(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [{**_task("T1"), "files": [SHARED]}, {**_task("T2"), "files": [SHARED]}], review=False)
    seen = {}

    def builder(request):
        task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
        seen[task] = (SHARED in {p.name for p in request.cwd.iterdir()})
        (request.cwd / f"{task.lower()}.txt").write_text("x\n")
        git(request.cwd, "add", ".")
        git(request.cwd, "commit", "--no-verify", "-qm", task)

    _use(ctx, builder)

    assert build.run(ctx).kind != "blocked"
    state = ctx.store.read_state()
    assert state["build"]["T2"]["start"] == state["integration"]["steps"][0]["head"], "T2 began on top of what T1 merged"


def test_Build_TasksWithOtherFiles_StillRunTogether(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)
    state = {}

    def builder(request):
        task = request.extra_env["SAKURA_FLOW_BUILD_TASK"]
        state.setdefault("starts", []).append(task)
        (request.cwd / f"{task.lower()}.txt").write_text("x\n")
        git(request.cwd, "add", ".")
        git(request.cwd, "commit", "--no-verify", "-qm", task)

    _use(ctx, builder)
    build.run(ctx)

    assert ctx.store.read_state()["build"]["T2"]["start"] == ctx.store.read_state()["build"]["T1"]["start"]


def test_Build_ATaskThatConflictsWithWhatIsMerged_GetsOneRoundToResolveItOnTopOfTheMergedWork(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)
    writers = Writers()
    runner = _use(ctx, writers)

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert "T1 wrote this" in (_engine_file(ctx) or "") and "and T2" in (_engine_file(ctx) or "")
    [first, second] = writers.findings["T2"]
    assert first == "" and "title T1" in second and SHARED in second and "title T2" in second
    assert len(runner.of("builder", "T2")) == 2


def _engine_file(ctx):
    store, head = final_head.proved(ctx)
    return git(store, "show", f"{head}:{SHARED}") if SHARED in _engine_tree(ctx) else None


def test_Build_AConflictThatSurvivesItsRound_BlocksWithTheTasksByTitleAndTheFile(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)
    _use(ctx, Writers(resolve=lambda round_number: False))

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and outcome.reason == f"merging T2 conflicts in {SHARED}"
    state = ctx.store.read_state()
    ctx.store.update_state(lambda current: current.update(status="blocked", reason=outcome.reason, phase="build", contract_sha256=""))
    (ask,) = [wait for wait in waits.collect([repo]) if wait.kind == "blocked"]
    assert ask.detail.startswith('The lane\'s tasks "title T1" and "title T2" both changed shared.txt')
    assert "Resume the lane to try another round, or stop it." in ask.detail and "T2" not in ask.detail.replace("title T2", "")
    assert ask.technical == outcome.reason and state["merge_conflict"]["files"] == [SHARED]


def test_Build_ResumingAfterAConflictThatSurvived_GivesTheTaskAnotherRound(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1"), _task("T2")], review=False)
    writers = Writers(resolve=lambda round_number: round_number >= 3)
    _use(ctx, writers)
    assert build.run(ctx).kind == "blocked"

    outcome = build.run(ctx)

    assert outcome.kind == "next", outcome.reason
    assert "merge_conflict" not in ctx.store.read_state()
