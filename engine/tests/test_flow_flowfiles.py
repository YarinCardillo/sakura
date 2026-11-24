"""The workspace snapshot the evaluator check compares: the engine's own exchange folder is not a change."""

import os

from sakura.flow import exchange, flowfiles, runner
from sakura.flow.evidence import EXCHANGE_DIR, EXCHANGE_GITIGNORE


def _engine_exchange(repo):
    folder = repo / EXCHANGE_DIR
    folder.mkdir()
    (folder / ".gitignore").write_bytes(EXCHANGE_GITIGNORE)
    return folder


def test_the_engine_exchange_folder_is_not_a_workspace_change(repo):
    before = flowfiles.workspace_snapshot(repo)
    _engine_exchange(repo)
    assert flowfiles.workspace_snapshot(repo) == before


def test_the_folder_the_exchange_really_creates_is_not_a_workspace_change(repo):
    from flow_support import make_context
    ctx = make_context(repo)
    before = flowfiles.workspace_snapshot(repo)
    prepared = exchange.prepare(ctx.store, runner.RunRequest("planner", "plan", "", {}, repo))
    exchange.collect(prepared)
    assert (repo / EXCHANGE_DIR / ".gitignore").is_file()
    assert flowfiles.workspace_snapshot(repo) == before


def test_a_folder_that_is_not_the_engines_exchange_folder_still_counts(repo):
    folder = repo / EXCHANGE_DIR
    folder.mkdir()
    (folder / ".gitignore").write_text("*\nsomething else\n")
    assert f"{EXCHANGE_DIR}/.gitignore" in flowfiles.workspace_snapshot(repo)


def test_anything_else_left_in_the_exchange_folder_still_counts(repo):
    folder = _engine_exchange(repo)
    (folder / "planted.txt").write_text("x\n")
    (folder / "run-1").mkdir()
    (folder / "run-1" / "leftover.txt").write_text("x\n")
    snapshot = flowfiles.workspace_snapshot(repo)
    assert f"{EXCHANGE_DIR}/planted.txt" in snapshot and f"{EXCHANGE_DIR}/run-1/leftover.txt" in snapshot
    assert f"{EXCHANGE_DIR}/.gitignore" not in snapshot


def test_a_symlinked_exchange_name_still_counts(repo, tmp_path):
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / ".gitignore").write_bytes(EXCHANGE_GITIGNORE)
    os.symlink(elsewhere, repo / EXCHANGE_DIR)
    assert EXCHANGE_DIR in flowfiles.workspace_snapshot(repo)


def test_the_same_folder_name_below_the_top_level_still_counts(repo):
    nested = repo / "sub" / EXCHANGE_DIR
    nested.mkdir(parents=True)
    (nested / ".gitignore").write_bytes(EXCHANGE_GITIGNORE)
    assert f"sub/{EXCHANGE_DIR}/.gitignore" in flowfiles.workspace_snapshot(repo)


def test_other_names_next_to_the_exchange_folder_still_count(repo):
    _engine_exchange(repo)
    (repo / ".sakura-role-copy").mkdir()
    (repo / ".sakura-role-copy" / "x.txt").write_text("x\n")
    (repo / "notes.txt").write_text("x\n")
    snapshot = flowfiles.workspace_snapshot(repo)
    assert ".sakura-role-copy/x.txt" in snapshot and "notes.txt" in snapshot


def test_AddedFlowFiles_AStagedFileOutsideTheTestPatterns_IsNeverAFlowFile(repo):
    # Security review of review 11: what the engine records as vetted flow files only ever matches the patterns.
    from conftest import git

    base = git(repo, "rev-parse", "HEAD")
    (repo / "e2e").mkdir()
    (repo / "e2e" / "flow.py").write_text("# flow\n")
    (repo / "app.py").write_text("print('not a test')\n")
    git(repo, "add", "app.py")
    assert flowfiles.added_flow_files(repo, base, ["e2e/**"]) == ["e2e/flow.py"]
