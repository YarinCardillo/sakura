"""The role exchange folder: roles read their inputs and write their outputs in their workspace, never in the engine home."""

import contextlib
import json
import os
import signal
import subprocess
import sys
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.flow import config, exchange, roles, runner
from sakura.flow.store import TaskStore

from flow_support import make_context, task_workspace


def _request(ctx, phase, values, cwd):
    return runner.RunRequest("planner", phase, "", values, cwd)


def test_paths_in_the_task_folder_become_paths_in_the_workspace_with_the_inputs_copied(repo):
    ctx = make_context(repo)
    ctx.store.spec.write_text("# Spec\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"spec_path": str(ctx.store.spec), "title": "x"}, repo))
    spec = Path(prepared.values["spec_path"])
    assert spec.read_text() == "# Spec\n" and repo in spec.parents
    assert prepared.values["title"] == "x"
    assert str(engine_home.root()) not in json.dumps(prepared.values)
    exchange.collect(prepared)


def test_the_exchange_folder_is_invisible_to_git(repo):
    ctx = make_context(repo)
    ctx.store.spec.write_text("# Spec\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"spec_path": str(ctx.store.spec)}, repo))
    status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain", "--untracked-files=all"],
                            capture_output=True, text=True, check=True).stdout
    assert status == ""
    exchange.collect(prepared)
    assert not prepared.folder.exists()
    assert [entry.name for entry in (repo / exchange.EXCHANGE_DIR).iterdir()] == [".gitignore"]


def test_a_declared_output_written_in_the_workspace_is_picked_up(repo):
    ctx = make_context(repo)
    values = {"plan_md_path": str(ctx.store.plan_md), "plan_json_path": str(ctx.store.plan_json)}
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", values, repo))
    Path(prepared.values["plan_md_path"]).write_text("# Plan\n")
    Path(prepared.values["plan_json_path"]).write_text('{"tasks": []}\n')
    exchange.collect(prepared)
    assert ctx.store.plan_md.read_text() == "# Plan\n" and ctx.store.plan_json.read_text() == '{"tasks": []}\n'


def test_an_input_changed_by_the_role_is_not_copied_back(repo):
    ctx = make_context(repo)
    ctx.store.spec.write_text("# Spec\n")
    values = {"spec_path": str(ctx.store.spec), "plan_md_path": str(ctx.store.plan_md)}
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", values, repo))
    Path(prepared.values["spec_path"]).write_text("# Forged spec\n")
    exchange.collect(prepared)
    assert ctx.store.spec.read_text() == "# Spec\n"


def test_an_output_that_is_a_symlink_is_refused(repo, tmp_path):
    ctx = make_context(repo)
    secret = tmp_path / "secret.txt"
    secret.write_text("secret\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    Path(prepared.values["plan_md_path"]).unlink()
    Path(prepared.values["plan_md_path"]).symlink_to(secret)
    exchange.collect(prepared)
    assert not ctx.store.plan_md.exists()


def test_evidence_named_by_the_result_lands_in_the_task_evidence(repo):
    ctx = make_context(repo)
    evidence = ctx.store.root / "evidence"
    prepared = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(evidence)}, repo))
    written = Path(prepared.values["evidence_dir"])
    (written / "role").mkdir(parents=True)
    (written / "base" / "role").mkdir(parents=True)
    (written / "role" / "R1.txt").write_text("PASS proof\n")
    (written / "base" / "role" / "R2.txt").write_text("base proof\n")
    (written / "role" / "unnamed.txt").write_text("not in the result\n")
    (written / "role" / "link.txt").symlink_to("/etc/hosts")
    names = exchange.evidence_names({"rows": [
        {"evidence": "evidence/role/R1.txt"}, {"evidence": "evidence/base/role/R2.txt"},
        {"evidence": "evidence/role/link.txt"}, {"evidence": "evidence/../state.json"}, {"evidence": "/etc/hosts"}]})
    assert names == ("evidence/role/R1.txt", "evidence/base/role/R2.txt", "evidence/role/link.txt")
    refused = exchange.collect(prepared, names)
    assert (evidence / "role" / "R1.txt").read_text() == "PASS proof\n"
    assert (evidence / "base" / "role" / "R2.txt").read_text() == "base proof\n"
    assert not (evidence / "role" / "unnamed.txt").exists() and not (evidence / "role" / "link.txt").exists()
    assert [name for name, _ in refused] == ["evidence/role/link.txt"]


@pytest.mark.parametrize("engine_log", ["evidence/base/engine/B1-api.log", "evidence/checks/R1-api.log",
                                        "evidence/evaluator/R1-api.log", "evidence/R1.txt"])
def test_EvidenceAtAnEnginePath_IsNeverCopiedOverTheEnginesFile(repo, engine_log):
    # Review 14, finding 3: a role's evidence lives under evidence/role/ (evidence/base/role/ on calibration); a
    # result naming any other evidence path cites it and never replaces it.
    ctx = make_context(repo)
    evidence = ctx.store.root / "evidence"
    target = ctx.store.root / engine_log
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("exit 1\n$ the engine's own log\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(evidence)}, repo))
    planted = Path(prepared.values["evidence_dir"]).parent / engine_log
    planted.parent.mkdir(parents=True, exist_ok=True)
    planted.write_text("exit 0\n$ forged\n")

    refused = exchange.collect(prepared, exchange.evidence_names({"rows": [{"evidence": engine_log}]}))

    assert target.read_text() == "exit 1\n$ the engine's own log\n"
    assert [name for name, _ in refused] == [engine_log] and "evidence/role/" in refused[0][1]


def test_ACitedEngineLogTheRoleDidNotWrite_IsNotARefusal(repo):
    ctx = make_context(repo)
    evidence = ctx.store.root / "evidence"
    prepared = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(evidence)}, repo))
    assert exchange.collect(prepared, ("evidence/evaluator/R1-api.log",)) == []


def test_two_runs_in_one_workspace_keep_separate_folders(repo):
    ctx = make_context(repo)
    first = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(ctx.store.root / "evidence")}, repo))
    second = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(ctx.store.root / "evidence")}, repo))
    assert first.folder != second.folder
    exchange.collect(first)
    assert second.folder.is_dir()
    exchange.collect(second)


def test_the_runner_hands_the_role_workspace_paths_and_collects_its_output(repo, tmp_path):
    ctx = make_context(repo)
    ctx.store.spec.write_text("# Spec\n")
    script = tmp_path / "role.py"
    script.write_text("import pathlib, re, sys\nprompt = sys.stdin.read()\n"
                      "spec, plan = re.findall(r'(\\S+\\.md)', prompt)\n"
                      "pathlib.Path(plan).write_text(pathlib.Path(spec).read_text() + 'planned\\n')\n")
    agent = runner.AgentRunner(ctx.store, config.load(repo, overrides={"roles": {"planner": f"exec:{sys.executable} {script}"}}))
    workspace = task_workspace(ctx)
    request = runner.RunRequest("planner", "plan", "read $spec_path write $plan_md_path", {
        "spec_path": str(ctx.store.spec), "plan_md_path": str(ctx.store.plan_md)}, workspace)
    result = agent.run(request)
    assert result.exit_code == 0
    assert ctx.store.plan_md.read_text() == "# Spec\nplanned\n"
    assert [entry.name for entry in (workspace / exchange.EXCHANGE_DIR).iterdir()] == [".gitignore"]
    assert str(engine_home.root()) not in result.log_path.read_text()
    assert TaskStore(repo, ctx.store.task_id).spec.read_text() == "# Spec\n"


# -- an agent controls its workspace: no symlink, swap or oversized file reaches the task folder -------------

def test_a_symlinked_exchange_folder_fails_the_role(repo, tmp_path):
    ctx = make_context(repo)
    (tmp_path / "elsewhere").mkdir()
    (repo / exchange.EXCHANGE_DIR).symlink_to(tmp_path / "elsewhere")
    with pytest.raises(roles.RoleRefused, match="not a real folder"):
        exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    assert list((tmp_path / "elsewhere").iterdir()) == []


def test_a_symlinked_gitignore_is_not_followed(repo, tmp_path):
    ctx = make_context(repo)
    target = tmp_path / "victim.txt"
    target.write_text("keep\n")
    (repo / exchange.EXCHANGE_DIR).mkdir()
    (repo / exchange.EXCHANGE_DIR / ".gitignore").symlink_to(target)
    with pytest.raises(roles.RoleRefused):
        exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    assert target.read_text() == "keep\n"


def test_a_symlinked_output_file_is_refused(repo, tmp_path):
    ctx = make_context(repo)
    secret = tmp_path / "secret.txt"
    secret.write_text("secret\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    Path(prepared.values["plan_md_path"]).unlink()
    Path(prepared.values["plan_md_path"]).symlink_to(secret)
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "regular" in refused[0][1]


def test_a_directory_in_place_of_an_output_file_is_refused(repo):
    ctx = make_context(repo)
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    Path(prepared.values["plan_md_path"]).unlink()
    Path(prepared.values["plan_md_path"]).mkdir()
    (Path(prepared.values["plan_md_path"]) / "inner.md").write_text("x\n")
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "regular" in refused[0][1]


def test_an_oversized_output_file_is_refused(repo):
    ctx = make_context(repo)
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    with open(prepared.values["plan_md_path"], "wb") as handle:
        handle.truncate(exchange.MAX_OUTPUT_BYTES + 1)
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "larger" in refused[0][1]


def test_an_evidence_folder_swapped_for_a_symlink_is_refused(repo, tmp_path):
    ctx = make_context(repo)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "role").mkdir()
    (outside / "role" / "R1.txt").write_text("planted\n")
    evidence = ctx.store.root / "evidence"
    prepared = exchange.prepare(ctx.store, _request(ctx, "evaluate", {"evidence_dir": str(evidence)}, repo))
    Path(prepared.values["evidence_dir"]).symlink_to(outside)
    refused = exchange.collect(prepared, ("evidence/role/R1.txt",))
    assert not (evidence / "role" / "R1.txt").exists() and refused


def test_an_output_replaces_an_older_one_in_the_task_folder(repo):
    ctx = make_context(repo)
    ctx.store.plan_md.write_text("old\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    Path(prepared.values["plan_md_path"]).write_text("new\n")
    assert exchange.collect(prepared) == []
    assert ctx.store.plan_md.read_text() == "new\n"


def test_the_runner_ledgers_an_output_it_refused(repo, tmp_path):
    ctx = make_context(repo)
    script = tmp_path / "role.py"
    script.write_text("import os, re, sys\nplan = re.findall(r'(\\S+\\.md)', sys.stdin.read())[0]\nos.unlink(plan); os.symlink('/etc/hosts', plan)\n")
    agent = runner.AgentRunner(ctx.store, config.load(repo, overrides={"roles": {"planner": f"exec:{sys.executable} {script}"}}))
    agent.run(runner.RunRequest("planner", "plan", "write $plan_md_path", {"plan_md_path": str(ctx.store.plan_md)},
                                task_workspace(ctx)))
    assert not ctx.store.plan_md.exists()
    assert "role output plan.md ignored: not a regular file" in ctx.store.ledger.read_text()


class Blocked(BaseException):
    """Raised by the deadline; not an OSError, so no handler in the exchange can mistake it for a refusal."""


@contextlib.contextmanager
def _deadline(seconds):
    """Fail the test instead of hanging when an open blocks."""
    def expired(_signum, _frame):
        raise Blocked("the exchange blocked")
    previous = signal.signal(signal.SIGALRM, expired)
    signal.alarm(seconds)
    try:
        yield
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)


def test_a_fifo_planted_under_an_output_name_is_refused_promptly(repo):
    ctx = make_context(repo)
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    Path(prepared.values["plan_md_path"]).unlink()
    os.mkfifo(prepared.values["plan_md_path"])
    with _deadline(5):
        refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "not a regular file" in refused[0][1]


def test_a_fifo_planted_as_the_gitignore_fails_the_role_promptly(repo):
    ctx = make_context(repo)
    (repo / exchange.EXCHANGE_DIR).mkdir()
    os.mkfifo(repo / exchange.EXCHANGE_DIR / ".gitignore")
    with _deadline(5), pytest.raises(roles.RoleRefused):
        exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))


# -- hardlinks: a second name for an inode the agent does not own ------------------------------------------------

def _replace_with_link(output: str, existing: Path) -> None:
    """What an agent does: drop the file the engine pre-created and hardlink another inode under that name."""
    Path(output).unlink(missing_ok=True)
    os.link(existing, output)


def test_an_output_hardlinked_to_a_file_outside_the_exchange_is_refused(repo, tmp_path):
    ctx = make_context(repo)
    outside = tmp_path / "private.txt"
    outside.write_text("private\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    _replace_with_link(prepared.values["plan_md_path"], outside)
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "hardlink" in refused[0][1]
    assert outside.read_text() == "private\n"


def test_an_output_hardlinked_to_an_engine_file_never_reaches_the_task_folder(repo):
    ctx = make_context(repo)
    ctx.store.spec.write_text("# Spec\n")
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    _replace_with_link(prepared.values["plan_md_path"], ctx.store.spec)
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "hardlink" in refused[0][1]
    assert ctx.store.spec.read_text() == "# Spec\n"


def test_a_link_added_while_the_output_is_read_is_refused(repo, tmp_path, monkeypatch):
    ctx = make_context(repo)
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md)}, repo))
    output = Path(prepared.values["plan_md_path"])
    output.write_text("# Plan\n")
    real_read = os.read

    def read_then_link(fd, size):
        data = real_read(fd, size)
        if data and not (tmp_path / "late-link").exists():
            os.link(output, tmp_path / "late-link")
        return data

    monkeypatch.setattr(exchange.os, "read", read_then_link)
    refused = exchange.collect(prepared)
    assert not ctx.store.plan_md.exists() and refused and "hardlink" in refused[0][1]


def test_the_engine_creates_the_declared_outputs_before_the_role_starts(repo):
    ctx = make_context(repo)
    prepared = exchange.prepare(ctx.store, _request(ctx, "plan", {"plan_md_path": str(ctx.store.plan_md),
                                                                  "plan_json_path": str(ctx.store.plan_json)}, repo))
    for key in ("plan_md_path", "plan_json_path"):
        path = Path(prepared.values[key])
        assert path.is_file() and path.stat().st_nlink == 1 and path.stat().st_size == 0
    assert exchange.collect(prepared) == []
    assert not ctx.store.plan_md.exists() and not ctx.store.plan_json.exists()
