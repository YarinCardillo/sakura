"""Arbiter rulings on builder requests, and the merge-time gate on protected paths."""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import workspaces
from sakura.flow.phases import build, build_task

from conftest import git
from test_flow_build import _commit, _ctx, _task, _zero

REQUEST = {"path": "tests/test_app.py", "kind": "test_change", "change": "add a test", "reason": "new feature"}
GRANT = {"path": "tests/test_app.py", "kind": "test_change", "decision": "grant", "why": "adds coverage only",
         "cost_if_wrong": "a weak test hides a regression", "spec_change": False}


@pytest.fixture(autouse=True)
def no_hook_install(monkeypatch):
    monkeypatch.setattr(workspaces, "_prepare", lambda ctx, name, path: None)


class ArbiterRunner:
    """Builder calls `builder(request, count)` and returns its result; arbiter runs return `arbiter_result`."""

    def __init__(self, builder, arbiter_result=None):
        self.builder, self.arbiter_result, self.requests = builder, arbiter_result, []

    def run(self, request):
        self.requests.append(request)
        result = None
        if request.role == "builder":
            result = self.builder(request, len(self.of("builder")))
        elif request.phase == "arbiter":
            result = self.arbiter_result
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, final="", result=result, log_path=Path("run.log"))

    def of(self, role):
        return [r for r in self.requests if r.role == role]


def _setup(repo, tmp_path, arbiter_result, roles=None, builder=None):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    if roles:
        object.__setattr__(ctx, "config", _with_roles(ctx.config, roles))
    asking = lambda request, count: {"ruling_requests": [REQUEST]} if count == 1 else (_commit(request) or None)
    runner = ArbiterRunner(builder or asking, arbiter_result)
    object.__setattr__(ctx, "runner", runner)
    return ctx, runner


def _with_roles(cfg, roles):
    values = {**cfg.values, "roles": {**cfg.values["roles"], **roles}}
    return SimpleNamespace(values=values, origins=cfg.origins, relaxations=cfg.relaxations)


def test_arbiter_rulings_stored_with_cost(repo, tmp_path):
    ctx, runner = _setup(repo, tmp_path, {"rulings": [GRANT]})
    assert build.run(ctx).kind == "next"
    stored = json.loads(ctx.store.rulings.read_text())["rulings"]
    assert stored[0]["cost_if_wrong"] == "a weak test hides a regression"
    assert ctx.store.granted("tests/test_app.py", "test_change")
    arbiter = runner.of("plan_reviewer")[0]
    assert arbiter.phase == "arbiter" and "tests/test_app.py" in arbiter.values["requests"]
    rerun = runner.of("builder")[1]
    assert "grant test_change tests/test_app.py" in rerun.values["rulings"]
    assert any(line.startswith("- Ruling: grant test_change") for line in ctx.store.ledger.read_text().splitlines())


def test_ruling_request_does_not_count_a_fix_round(repo, tmp_path):
    ctx, _ = _setup(repo, tmp_path, {"rulings": [GRANT]})
    build.run(ctx)
    assert ctx.store.read_state()["build"]["T1"]["rounds"] == 1


def test_arbiter_falls_back_to_code_reviewer(repo, tmp_path):
    ctx, runner = _setup(repo, tmp_path, {"rulings": [GRANT]}, roles={"plan_reviewer": "none"})
    assert build.run(ctx).kind == "next"
    assert [r.phase for r in runner.of("code_reviewer")] == ["arbiter"]


def test_no_arbiter_role_denies_and_ledgers(repo, tmp_path):
    ctx, runner = _setup(repo, tmp_path, None, roles={"plan_reviewer": "none", "code_reviewer": "none"})
    assert build.run(ctx).kind == "next"
    assert not ctx.store.granted("tests/test_app.py", "test_change")
    assert "deny test_change tests/test_app.py" in ctx.store.ledger.read_text()
    assert not runner.of("code_reviewer") and not runner.of("plan_reviewer")


def test_malformed_arbiter_result_denies(repo, tmp_path):
    ctx, _ = _setup(repo, tmp_path, {"rulings": [{"path": "tests/test_app.py"}]})
    build.run(ctx)
    assert not ctx.store.granted("tests/test_app.py", "test_change")
    assert "deny test_change" in ctx.store.ledger.read_text()


def test_spec_change_ruling_is_recorded_in_state(repo, tmp_path):
    ctx, _ = _setup(repo, tmp_path, {"rulings": [{**GRANT, "spec_change": True}]})
    build.run(ctx)
    assert ctx.store.read_state()["pending_spec_rulings"] == [{"path": "tests/test_app.py", "kind": "test_change"}]


def test_endless_ruling_requests_block(repo, tmp_path):
    ctx, _ = _setup(repo, tmp_path, {"rulings": [GRANT]}, builder=lambda request, count: {"ruling_requests": [REQUEST]})
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "ruling" in outcome.reason


def _protect(ctx, *paths):
    ctx.store.update_state(lambda state: state.update(protected_paths=list(paths)))


def _edit_existing(request, name="tests/test_app.py"):
    (request.cwd / name).write_text("changed\n")
    git(request.cwd, "add", name)
    git(request.cwd, "commit", "--no-verify", "-qm", "edit")


def _repo_with_test(repo):
    (repo / "tests").mkdir()
    (repo / "tests/test_app.py").write_text("original\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "add test")


def test_merge_gate_blocks_changed_existing_test(repo, tmp_path):
    _repo_with_test(repo)
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    object.__setattr__(ctx, "runner", ArbiterRunner(lambda request, count: _edit_existing(request)))
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "tests/test_app.py" in outcome.reason


def test_merge_gate_rejects_timeout_ruling_for_test_change(repo, tmp_path):
    _repo_with_test(repo)
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    from sakura.flow.store import Ruling
    ctx.store.add_ruling(Ruling("tests/test_app.py", "timeout", "grant", "raise timeout", "slow test", False))
    object.__setattr__(ctx, "runner", ArbiterRunner(lambda request, count: _edit_existing(request)))
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and "tests/test_app.py" in outcome.reason


def test_merge_gate_allows_ruled_timeout_increase(repo, tmp_path):
    (repo / "tests").mkdir()
    (repo / "tests/test_app.py").write_text("timeout = 5\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "add test")
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    from sakura.flow.store import Ruling
    ctx.store.add_ruling(Ruling("tests/test_app.py", "timeout", "grant", "raise timeout", "slow test", False))

    def builder(request, count):
        _commit(request, "tests/test_app.py", "timeout = 30\n")

    object.__setattr__(ctx, "runner", ArbiterRunner(builder))
    assert build.run(ctx).kind == "next"


def test_merge_gate_rejects_timeout_ruling_for_mixed_assertion_change(repo, tmp_path):
    (repo / "tests").mkdir()
    path = repo / "tests/test_app.py"
    path.write_text("def test_a():\n    run(timeout=5)\n    assert result == expected\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "add protected test")
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    from sakura.flow.store import Ruling
    ctx.store.add_ruling(Ruling("tests/test_app.py", "timeout", "grant", "raise timeout", "slow test", False))

    def builder(request, count):
        path = request.cwd / "tests/test_app.py"
        path.write_text("def test_a():\n    run(timeout=30)\n    assert True\n")
        git(request.cwd, "add", "tests/test_app.py")
        git(request.cwd, "commit", "--no-verify", "-qm", "raise timeout and weaken assertion")

    object.__setattr__(ctx, "runner", ArbiterRunner(builder))

    outcome = build.run(ctx)

    assert outcome.kind == "blocked" and "tests/test_app.py" in outcome.reason


def test_merge_gate_allows_ruled_change_and_new_tests(repo, tmp_path):
    _repo_with_test(repo)
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    ctx.store.update_state(lambda state: state.update(base_commit=git(repo, "rev-parse", "HEAD")))
    ctx.store.add_ruling(build_task_ruling())

    def builder(request, count):
        _commit(request, "tests/test_new.py", "def test_x(): pass\n")
        _edit_existing(request)

    object.__setattr__(ctx, "runner", ArbiterRunner(builder))
    assert build.run(ctx).kind == "next"


def test_merge_gate_blocks_new_sakura_files(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, [_task("T1")], review=False)
    object.__setattr__(ctx, "runner", ArbiterRunner(lambda request, count: _commit_in_dir(request, ".sakura/notes.txt")))
    outcome = build.run(ctx)
    assert outcome.kind == "blocked" and ".sakura/notes.txt" in outcome.reason


def _commit_in_dir(request, name):
    (request.cwd / name).parent.mkdir(parents=True, exist_ok=True)
    _commit(request, name, "x\n")


def build_task_ruling():
    from sakura.flow.store import Ruling
    return Ruling("tests/test_app.py", "test_change", "grant", "why", "cost", False)
