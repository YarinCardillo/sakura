"""A step that must only read the project stops the lane when it changes it, but never for the caches its tools leave behind;
the ask says who, in which step, and which files, in the words the person knows."""

import subprocess

import pytest

from sakura.flow.phases import runs

from conftest import git


@pytest.fixture
def workspace(repo):
    (repo / "tally.py").write_text("print('tally')\n")
    git(repo, "add", "tally.py")
    git(repo, "commit", "-qm", "tally")
    return repo


def _head(path):
    return git(path, "rev-parse", "HEAD").strip()


def test_WorkspaceChanges_InterpreterCachesTheReviewerRan_AreNoChange(workspace):
    head = _head(workspace)
    for name in ("__pycache__/tally.cpython-314.pyc", "tests/__pycache__/test_reverse.cpython-314.pyc",
                 ".pytest_cache/v/cache/lastfailed", ".ruff_cache/0.1/x"):
        (workspace / name).parent.mkdir(parents=True, exist_ok=True)
        (workspace / name).write_bytes(b"\0")

    assert runs.workspace_changes(workspace, "plan_reviewer", "arbiter", head) is None


def test_WorkspaceChanges_WhatTheProjectsGitignoreIgnores_IsNoChange(workspace):
    (workspace / ".gitignore").write_text("build/\n*.log\n")
    git(workspace, "add", ".gitignore")
    git(workspace, "commit", "-qm", "ignore")
    head = _head(workspace)
    (workspace / "build").mkdir()
    (workspace / "build" / "out.bin").write_bytes(b"\0")
    (workspace / "run.log").write_text("x\n")

    assert runs.workspace_changes(workspace, "code_reviewer", "review", head) is None


def test_WorkspaceChanges_ARealChange_NamesWhoDidItInWhichStepAndWhichFilesBesidesTheCaches(workspace):
    head = _head(workspace)
    (workspace / "tally.py").write_text("print('changed')\n")
    (workspace / "notes.txt").write_text("a new file\n")
    (workspace / "__pycache__").mkdir()
    (workspace / "__pycache__" / "tally.cpython-314.pyc").write_bytes(b"\0")

    outcome = runs.workspace_changes(workspace, "plan_reviewer", "arbiter", head)

    assert outcome.kind == "blocked"
    assert outcome.reason == ("The plan reviewer should only read the project, but it changed notes.txt and tally.py while "
                              "ruling on a request for a protected change. The lane stopped so that nothing unreviewed goes "
                              "on: resume it to try again.")
    for engine_word in ("arbiter", "__pycache__", "workspace"):
        assert engine_word not in outcome.reason


@pytest.mark.parametrize("role,phase,who,step", [
    ("code_reviewer", "review", "code reviewer", "reviewing the code"),
    ("plan_reviewer", "plan_review", "plan reviewer", "reviewing the plan"),
    ("planner", "contract", "planner", "writing the contract"),
    ("planner", "plan", "planner", "planning"),
    ("planner", "intake", "planner", "writing the spec"),
])
def test_WorkspaceChanges_EveryStep_IsNamedInTheWordsTheAppUses(workspace, role, phase, who, step):
    head = _head(workspace)
    (workspace / "tally.py").write_text("print('changed')\n")

    reason = runs.workspace_changes(workspace, role, phase, head).reason

    assert reason.startswith(f"The {who} should only read the project, but it changed tally.py while {step}.")


def test_WorkspaceChanges_ANewCommit_IsSaidAsACommit(workspace):
    head = _head(workspace)
    (workspace / "more.py").write_text("x = 1\n")
    git(workspace, "add", "more.py")
    git(workspace, "commit", "-qm", "more")

    reason = runs.workspace_changes(workspace, "code_reviewer", "review", head).reason

    assert reason.startswith("The code reviewer should only read the project, but it added a commit while reviewing the code.")


def test_WorkspaceChanges_ManyFiles_AreCutWithTheirCount(workspace):
    head = _head(workspace)
    for number in range(8):
        (workspace / f"f{number}.txt").write_text("x\n")

    reason = runs.workspace_changes(workspace, "code_reviewer", "review", head).reason

    assert "f0.txt, f1.txt, f2.txt, f3.txt, f4.txt and 3 more" in reason
