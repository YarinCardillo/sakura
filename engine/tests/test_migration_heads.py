from __future__ import annotations

from sakura import migration_heads

from conftest import git


def test_heads_reads_commit_without_checkout(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "a.py").write_text("revision = 'a'\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "migration")
    commit = git(repo, "rev-parse", "HEAD")
    (versions / "a.py").write_text("not valid python")
    assert migration_heads.heads(commit, "migrations/versions", repo) == ["a"]


def test_heads_returns_multiple_heads(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "a.py").write_text("revision = 'a'\ndown_revision = None\n")
    (versions / "b.py").write_text("revision = 'b'\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "migrations")
    assert migration_heads.heads("HEAD", "migrations/versions", repo) == ["a", "b"]


def test_heads_reads_chained_revision_assignment(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "b.py").write_text("revision = alias = 'b'\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "chained migration")
    assert migration_heads.heads("HEAD", "migrations/versions", repo) == ["b"]


def test_heads_refuses_ambiguous_revision_assignment(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "b.py").write_text("revision, alias = ('b', 'alias')\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "ambiguous migration")
    try:
        migration_heads.heads("HEAD", "migrations/versions", repo)
    except migration_heads.MigrationError as error:
        assert "cannot resolve" in str(error)
    else:
        raise AssertionError("ambiguous migration was accepted")


def test_heads_refuses_conditionally_assigned_revision(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "b.py").write_text("if enabled:\n    revision = 'b'\ndown_revision = None\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "conditional migration")
    try:
        migration_heads.heads("HEAD", "migrations/versions", repo)
    except migration_heads.MigrationError as error:
        assert "cannot resolve" in str(error)
    else:
        raise AssertionError("conditional migration was accepted")


def test_heads_rejects_unknown_parent_in_migration_graph(repo):
    versions = repo / "migrations/versions"
    versions.mkdir(parents=True)
    (versions / "a.py").write_text("revision = 'a'\ndown_revision = 'missing'\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "invalid migration")
    try:
        migration_heads.heads("HEAD", "migrations/versions", repo)
    except migration_heads.MigrationError as error:
        assert "unknown" in str(error)
    else:
        raise AssertionError("invalid graph was accepted")
