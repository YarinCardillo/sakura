"""A lane whose preset switches the checks off delivers without contract rows: the walk from the build to the pull request."""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura.flow import contract, engine, plan, workspaces
from sakura.flow.phases import build, evaluate, pull_request

from conftest import git
from flow_support import make_context

PULL_REQUEST_URL = "https://example.test/pull/5"


@pytest.fixture(autouse=True)
def real_phases(monkeypatch):
    """The real build, evaluate and pull request handlers; the walk stops at the report, which has its own tests."""
    monkeypatch.setattr(workspaces, "_prepare", lambda ctx, name, path: None)
    monkeypatch.setattr(engine, "_HANDLERS", {
        "build": build.run, "evaluate": evaluate.run, "pr": pull_request.run,
        "report": lambda ctx: engine.PhaseOutcome("await", gate="report")})
    monkeypatch.setattr(engine, "_INPUTS", {"build": build.inputs_hash, "evaluate": evaluate.inputs_hash})


class _Agents:
    """The agents' double: the builder commits one file, a reviewer approves."""

    def run(self, request):
        result = None
        if request.role == "builder":
            (request.cwd / "greeting.txt").write_text("hello\n")
            git(request.cwd, "add", "greeting.txt")
            git(request.cwd, "commit", "--no-verify", "-qm", "feat: greeting")
        else:
            result = {"verdict": "APPROVE", "findings": []}
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, final="", result=result, log_path=Path("run.log"))


def _fake_gh(monkeypatch):
    """`gh` as a GitHub host would answer: no draft yet, then the URL of the one it creates; the push is real."""
    created, real = [], pull_request._command

    def command(ctx, cwd, name, argv, env=None):
        if argv[0] != "gh":
            return real(ctx, cwd, name, argv, env)
        result = SimpleNamespace(stopped=False, stalled=False, timed_out=False, exit_code=0)
        if argv[:3] == ["gh", "pr", "create"]:
            created.append(argv)
            return result, PULL_REQUEST_URL + "\n", Path("pull-request.log")
        return result, "[]", Path("pull-request.log")

    monkeypatch.setattr(pull_request, "_command", command)
    return created


def _lane(repo, tmp_path, preset):
    """A lane at the start of its build, with a contract of two rows no one ran, and a bare remote to deliver to."""
    remote = tmp_path / "remote.git"
    git(tmp_path, "init", "-q", "--bare", str(remote))
    base = git(repo, "rev-parse", "HEAD")
    overrides = {"preset": preset, "relax": ["preset"], "workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []},
                 "tests": {"full": []}}
    state = {"base_commit": base, "branch": "feat/greeting", "workspaces": {}, "build": {}, "rounds": {},
             "phase": "build", "push_url": str(remote), "pr": {}}
    ctx = make_context(repo, overrides=overrides, state=state)
    workspaces.create(ctx, "0", "feat/greeting", base)
    rows = [contract.Row(f"R{n}", "J1", f"step {n}", "cli", None, "true", False, False) for n in (1, 2)]
    contract.save(ctx.store, rows)
    ctx.store.update_state(lambda current: current.update(contract_sha256=_digest(ctx)))
    ctx.store.spec.write_text("# Add a greeting\n\n## Journeys\n")
    ctx.store.plan_md.write_text("# Plan\n")
    ctx.store.plan_json.write_text(json.dumps({"tasks": [
        {"id": "T1", "title": "greeting", "depends_on": [], "rows": ["R1"], "files": ["greeting.txt"], "seam": False}]}))
    digest = plan.digest(ctx.store)
    ctx.store.update_state(lambda current: current.update(plan_sha256=digest))
    object.__setattr__(ctx, "runner", _Agents())
    return ctx, remote


def _digest(ctx) -> str:
    import hashlib

    return hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest()


def test_Lean_LaneWithOpenContractRows_WalksToADeliveredPullRequest(repo, tmp_path, monkeypatch):
    created = _fake_gh(monkeypatch)
    ctx, remote = _lane(repo, tmp_path, "lean")

    engine.advance(ctx)

    state = ctx.store.read_state()
    assert state["phase"] == "report" and state["pr"]["url"] == PULL_REQUEST_URL, state["reason"]
    assert git(remote, "rev-parse", "refs/heads/feat/greeting") == state["pr"]["head"]
    body = Path(created[0][created[0].index("--body-file") + 1]).read_text()
    assert "checks were switched off by the preset" in body


def test_Standard_LaneWithOpenContractRows_StillWaitsForThem(repo, tmp_path, monkeypatch):
    _fake_gh(monkeypatch)
    ctx, _remote = _lane(repo, tmp_path, "standard")
    ctx.store.update_state(lambda current: current.update(phase="pr"))
    build.run(ctx)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "blocked" and "contract rows are still open" in outcome.reason


def test_LaneSwitchedToLeanAfterItsBuild_DeliversFromThatPointWithoutContractRows(repo, tmp_path, monkeypatch):
    created = _fake_gh(monkeypatch)
    ctx, _remote = _lane(repo, tmp_path, "standard")
    build.run(ctx)
    ctx.store.update_state(lambda current: current.update(phase="pr", overrides={**current["overrides"], "preset": "lean"}))

    engine.advance(ctx)

    state = ctx.store.read_state()
    assert state["phase"] == "report" and state["pr"]["url"] == PULL_REQUEST_URL, state["reason"]
    assert "checks were switched off by the preset" in Path(created[0][created[0].index("--body-file") + 1]).read_text()
