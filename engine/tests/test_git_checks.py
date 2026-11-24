from __future__ import annotations

from sakura import git_checks, guard, migration_heads, repository, testing

from conftest import git


def push_line(repo, branch="feat/example"):
    sha = git(repo, "rev-parse", "HEAD")
    return f"refs/heads/{branch} {sha} refs/heads/{branch} {'0' * 40}"


def test_pre_commit_refuses_protected_branch(repo):
    ok, reason = git_checks.pre_commit(repo)
    assert not ok
    assert "protected" in reason


def test_pre_push_refuses_protected_branch_and_invalid_branch(repo):
    sha = git(repo, "rev-parse", "HEAD")
    ok, reason = git_checks.pre_push(repo, f"refs/heads/main {sha} refs/heads/main {'0' * 40}")
    assert not ok and "protected" in reason
    ok, reason = git_checks.pre_push(repo, f"refs/heads/bad {sha} refs/heads/bad {'0' * 40}")
    assert not ok and "branch_pattern" in reason


def test_pre_push_refuses_deleting_a_protected_branch(repo):
    ok, reason = git_checks.pre_push(repo, f"refs/heads/main {'0' * 40} refs/heads/main {'0' * 40}")
    assert not ok and "protected" in reason


def test_pre_push_refuses_integration_branch_and_merge_subject(repo):
    ok, reason = git_checks.pre_push(repo, push_line(repo, "sakura-integration/local"))
    assert not ok and "local-only" in reason
    git(repo, "commit", "--allow-empty", "-qm", "sakura-integration: #1")
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "merge commits" in reason


def test_pre_push_requires_fast_test_stamp_for_pushed_tree(repo):
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[tests]\nfast = ["true"]\n')
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "stamp" in reason
    assert testing.run(repo, "fast") == 0
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert ok and not reason


def test_pre_push_skips_stamp_when_fast_tests_are_empty(repo, capsys):
    assert testing.run(repo, "fast") == 0
    assert "no commands" in capsys.readouterr().out
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert ok and not reason
    assert "stamp check skipped" in capsys.readouterr().err


def test_pre_push_requires_signed_commits_when_configured(repo):
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text("require_signed_commits = true\n")
    git(repo, "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "unsigned")
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "signed" in reason


def test_pre_push_verifies_only_outgoing_commit_signatures(repo, monkeypatch):
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text("require_signed_commits = true\n")
    sha = "a" * 40
    old = "b" * 40

    def fake_git(_, *args):
        if args[:2] == ("log", "--format=%s"):
            return ""
        if args == ("rev-list", sha, "--not", "--remotes"):
            return "new-commit"
        if args == ("verify-commit", "new-commit"):
            raise repository.RepositoryError("bad signature")
        if args == ("rev-list", sha):
            return "new-commit"
        if args[:2] == ("cat-file", "-p"):
            return "gpgsig forged"
        raise AssertionError(args)

    monkeypatch.setattr(git_checks, "_git", fake_git)
    ok, reason = git_checks.pre_push(repo, f"refs/heads/feat/example {sha} refs/heads/feat/example {old}")
    assert not ok and "signed" in reason


def test_unsigned_check_excludes_commits_reachable_from_remote_tracking_refs(repo, monkeypatch):
    sha = "a" * 40
    calls = []

    def fake_git(_, *args):
        calls.append(args)
        if args == ("rev-list", sha, "--not", "--remotes"):
            return "new-commit"
        if args == ("verify-commit", "new-commit"):
            return ""
        raise AssertionError(args)

    monkeypatch.setattr(git_checks, "_git", fake_git)
    assert not git_checks._unsigned(repo, sha, "0" * 40)
    assert calls == [("rev-list", sha, "--not", "--remotes"), ("verify-commit", "new-commit")]


def test_pre_push_refuses_sakura_integration_destination(repo):
    ok, reason = git_checks.pre_push(
        repo,
        f"refs/heads/feat/example {git(repo, 'rev-parse', 'HEAD')} refs/heads/sakura-integration/x {'0' * 40}",
    )
    assert not ok and "local-only" in reason


def test_pre_push_refuses_guarded_change_latched_by_agent_session(repo):
    guard.save_snapshot(repo, "agent")
    target = repo / ".sakura/guarded"
    target.parent.mkdir()
    target.write_text("changed")
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "guarded-file fingerprint" in reason


def test_pre_push_refuses_multiple_migration_heads(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "a.py").write_text("revision = 'a'\ndown_revision = None\n")
    (versions / "b.py").write_text("revision = 'b'\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "migrations")
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[migrations]\nalembic_versions = "migrations/versions"\n')
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "multiple Alembic heads" in reason


def test_pre_push_refuses_annotated_migration_heads(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "a.py").write_text("revision: str = 'a'\ndown_revision: str | None = None\n")
    (versions / "b.py").write_text("revision: str = 'b'\ndown_revision: str | None = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "annotated migrations")
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir()
    config.write_text('[migrations]\nalembic_versions = "migrations/versions"\n')
    ok, reason = git_checks.pre_push(repo, push_line(repo))
    assert not ok and "multiple Alembic heads" in reason
