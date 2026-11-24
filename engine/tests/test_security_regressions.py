from __future__ import annotations

from pathlib import Path

from sakura import git_checks, stamps, testing

from conftest import git


def test_readme_states_local_guardrails_and_server_required_ci_authority():
    readme = (Path(__file__).parents[1] / "README.md").read_text().lower()
    assert "local guardrails" in readme
    assert "required ci" in readme


def test_fast_test_never_stamps_head_from_dirty_worktree(repo):
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[tests]\nfast = ["test -f uncommitted-proof"]\n')
    (repo / "uncommitted-proof").write_text("present only in worktree\n")
    assert testing.run(repo, "fast") == 1
    assert not stamps.has(repo, git_checks._tree(repo, "HEAD"), "fast", ["test -f uncommitted-proof"])


def test_fast_stamp_binds_the_configured_command_set(repo):
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[tests]\nfast = ["true"]\n')
    assert testing.run(repo, "fast") == 0
    config.write_text('[tests]\nfast = ["true", "true"]\n')
    ok, reason = git_checks.pre_push(
        repo,
        f"refs/heads/feat/example {git(repo, 'rev-parse', 'HEAD')} refs/heads/feat/example {'0' * 40}",
    )
    assert not ok and "stamp" in reason


def test_fast_test_refuses_stamp_when_private_checkout_tree_changes(repo):
    (repo / "tracked.txt").write_text("original\n")
    git(repo, "add", "tracked.txt")
    git(repo, "commit", "-qm", "tracked file")
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[tests]\nfast = ["printf tampered > tracked.txt && git add tracked.txt"]\n')
    commands = ["printf tampered > tracked.txt && git add tracked.txt"]
    assert testing.run(repo, "fast") == 1
    assert not stamps.has(repo, git_checks._tree(repo, "HEAD"), "fast", commands)
