"""Reopening a run: stored paths are re-checked and the provider config root comes from the operator."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from sakura import repository
from sakura.flow import conversations, store
from sakura.flow.conversations import ReopenRefused

SESSION_ID = "9b5c773d-e4a1-4311-bca3-43830f5ff541"
CODEX_ID = "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"


@pytest.fixture
def home(tmp_path):
    path = tmp_path / "operator-home"
    path.mkdir()
    return path


@pytest.fixture
def task(repo, tmp_path):
    workspace = tmp_path / "workspace-0"
    workspace.mkdir()
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo", "status": "running", "workspaces": {"0": {"path": str(workspace)}}})
    return task_store


def record_for(task, **changes):
    record = {"name": "build-1", "role": "builder", "provider": "codex", "model": "gpt", "effort": "low",
              "state": "finished", "session_id": CODEX_ID, "cwd": str(task.repo.resolve()),
              "env": {"SAKURA_FLOW_TASK": "demo"}, "transcript_root": ""}
    record.update(changes)
    return record


def reopen(task, record, home, environ=None):
    return conversations.plan_reopen(record, task, task.read_state(), repository.common_dir(task.repo),
                                     environ=environ or {}, home=home)


def put_transcript(root: Path, cwd: str) -> None:
    path = root / "projects" / re.sub(r"[^A-Za-z0-9]", "-", cwd) / f"{SESSION_ID}.jsonl"
    path.parent.mkdir(parents=True)
    path.write_text("{}\n")


def test_PlanReopen_RepositoryCwd_Allowed(task, home):
    record = record_for(task, transcript_root=str((home / ".codex").resolve()))
    assert reopen(task, record, home).cwd == task.repo.resolve()


def test_PlanReopen_RecordedWorkspaceCwd_Allowed(task, home, tmp_path):
    workspace = (tmp_path / "workspace-0").resolve()
    record = record_for(task, cwd=str(workspace), transcript_root=str((home / ".codex").resolve()))
    assert reopen(task, record, home).cwd == workspace


def test_PlanReopen_CwdOutsideRepositoryAndWorkspaces_Refused(task, home, tmp_path):
    elsewhere = tmp_path / "hooked-project"
    elsewhere.mkdir()
    record = record_for(task, cwd=str(elsewhere), transcript_root=str((home / ".codex").resolve()))
    with pytest.raises(ReopenRefused, match="not the repository or a recorded workspace"):
        reopen(task, record, home)


def test_PlanReopen_StoredRootDiffersFromOperatorRoot_Refused(task, home, tmp_path):
    attacker = tmp_path / "attacker-config"
    attacker.mkdir()
    record = record_for(task, transcript_root=str(attacker.resolve()))
    with pytest.raises(ReopenRefused, match="another config root"):
        reopen(task, record, home)


def test_PlanReopen_DefaultRoot_SetsNoConfigVariable(task, home):
    record = record_for(task, transcript_root=str((home / ".codex").resolve()))
    assert "CODEX_HOME" not in reopen(task, record, home).env


def test_PlanReopen_OperatorRootMatchesRecord_PassesVariable(task, home, tmp_path):
    configured = (tmp_path / "codex-home").resolve()
    record = record_for(task, transcript_root=str(configured))
    plan = reopen(task, record, home, {"CODEX_HOME": str(configured)})
    assert plan.env["CODEX_HOME"] == str(configured)
    assert plan.env["SAKURA_FLOW_TASK"] == "demo"


def test_PlanReopen_OperatorRootDiffersFromRecord_Refused(task, home, tmp_path):
    record = record_for(task, transcript_root=str((home / ".codex").resolve()))
    with pytest.raises(ReopenRefused, match="another config root"):
        reopen(task, record, home, {"CODEX_HOME": str(tmp_path / "other")})


def test_PlanReopen_ClaudeTranscriptLooksUnderOperatorRoot(task, home):
    root = (home / ".claude").resolve()
    record = record_for(task, provider="claude", model="sonnet", session_id=SESSION_ID, transcript_root=str(root))
    with pytest.raises(ReopenRefused, match="no transcript"):
        reopen(task, record, home)
    put_transcript(root, record["cwd"])
    assert reopen(task, record, home).env.get("CLAUDE_CONFIG_DIR") is None


def test_PlanReopen_RelativeStoredCwd_Refused(task, home, monkeypatch):
    monkeypatch.chdir(task.repo)
    record = record_for(task, cwd=".", transcript_root=str((home / ".codex").resolve()))
    with pytest.raises(ReopenRefused, match="not absolute"):
        reopen(task, record, home)


def test_PlanReopen_CodexArgvCarriesTheCheckedDirectory(task, home, tmp_path):
    link = tmp_path / "repo-link"
    link.symlink_to(task.repo)
    record = record_for(task, cwd=str(link), transcript_root=str((home / ".codex").resolve()))
    plan = reopen(task, record, home)
    assert plan.argv[plan.argv.index("-C") + 1] == str(task.repo.resolve())
