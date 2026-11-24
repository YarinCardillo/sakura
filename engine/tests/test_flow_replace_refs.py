"""A `refs/replace` entry an agent planted must not change what the engine tests, diffs or records."""

from conftest import git
from flow_support import make_context
from sakura.flow import cage, checks, completion, flowfiles, gitutil, sealed


def _delivered_and_substitute(repo):
    """The commit the task delivers (HEAD) and a different commit a replace ref will stand in for it."""
    (repo / "app.txt").write_text("delivered\n")
    git(repo, "add", "app.txt")
    git(repo, "commit", "-qm", "delivered")
    delivered = git(repo, "rev-parse", "HEAD")
    git(repo, "checkout", "-q", "--detach", "HEAD~1")
    (repo / "app.txt").write_text("substitute\n")
    git(repo, "add", "app.txt")
    git(repo, "commit", "-qm", "substitute")
    substitute = git(repo, "rev-parse", "HEAD")
    git(repo, "checkout", "-q", "main")
    git(repo, "replace", delivered, substitute)
    return delivered, substitute


def test_ReplaceRef_EngineGitStillSeesTheRealCommitAndItsFiles(repo):
    delivered, substitute = _delivered_and_substitute(repo)
    assert git(repo, "show", f"{delivered}:app.txt") == "substitute", "the fixture's replace ref must be in effect"

    assert gitutil.git(repo, "show", f"{delivered}:app.txt").strip() == "delivered"
    assert gitutil.git(repo, "log", "-1", "--format=%s", delivered).strip() == "delivered"


def test_ReplaceRef_TheCompletionCheckoutHoldsTheDeliveredFiles(repo, tmp_path):
    delivered, _ = _delivered_and_substitute(repo)
    gitutil.trust_repository(repo)  # a seal verifies its source against the task's repository

    with completion.at(sealed.seal(repo, delivered), delivered, repo.parent, "replace") as checkout:
        assert (checkout.source / "app.txt").read_text() == "delivered\n"
        assert completion.changes(checkout) == ""


def test_ReplaceRef_StatusAndDiffOfTheWorkspaceStayTruthful(repo):
    delivered, _ = _delivered_and_substitute(repo)

    assert flowfiles.uncommitted(repo) == []
    assert gitutil.git(repo, "diff", "--name-only", "HEAD~1", "HEAD").split() == ["app.txt"]
    assert gitutil.git(repo, "diff", "HEAD~1", "HEAD").count("+delivered") == 1


def test_ReplaceRef_CagedCommandsDoNotHonourThemEither(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "replace", cage.load_policy("default"), ())

    assert run_cage.env["GIT_NO_REPLACE_OBJECTS"] == "1"


def test_ReplaceRef_AgentsCannotWriteThemInTheCommonFolder(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    site = cage.RunSite(ctx.store.root, "build-1", "builder", repo, repo / ".git", (tmp_path / "engine",), repo)

    assert str(repo / ".git" / "refs" / "replace") in cage._protected_writes(site, tmp_path)


def test_ReplaceRef_WrittenIntoPackedRefs_IsStillIgnoredByTheEngine(repo):
    delivered, _ = _delivered_and_substitute(repo)
    git(repo, "pack-refs", "--all")
    assert not (repo / ".git" / "refs" / "replace" / delivered).exists()
    assert f"refs/replace/{delivered}" in (repo / ".git" / "packed-refs").read_text()
    assert git(repo, "show", f"{delivered}:app.txt") == "substitute", "the packed replace ref must be in effect"

    assert gitutil.git(repo, "show", f"{delivered}:app.txt").strip() == "delivered"
    gitutil.trust_repository(repo)
    with completion.at(sealed.seal(repo, delivered), delivered, repo.parent, "packed") as checkout:
        assert (checkout.source / "app.txt").read_text() == "delivered\n"


def test_ReplaceRef_TheKitsOwnGitCallsIgnoreThemToo(repo):
    from sakura import git_checks, migration_heads

    delivered, substitute = _delivered_and_substitute(repo)
    real_tree = git(repo, "-c", "core.useReplaceRefs=false", "rev-parse", f"{delivered}^{{tree}}")

    assert git_checks._tree(repo, delivered) == real_tree
    assert migration_heads._run(repo, "show", f"{delivered}:app.txt").stdout == "delivered\n"


def test_Grafts_EngineGitRefusesToRunWhileInfoGraftsExists(repo):
    import pytest

    gitutil.trust_repository(repo)
    (repo / ".git" / "info").mkdir(exist_ok=True)
    (repo / ".git" / "info" / "grafts").write_text(f"{git(repo, 'rev-parse', 'HEAD')}\n")

    with pytest.raises(gitutil.GitError, match="info/grafts"):
        gitutil.git(repo, "rev-parse", "HEAD")


def test_Cage_AgentsCannotWriteGraftsOrTheShallowFile(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    site = cage.RunSite(ctx.store.root, "build-1", "builder", repo, repo / ".git", (tmp_path / "engine",), repo)

    protected = cage._protected_writes(site, tmp_path)

    assert str(repo / ".git" / "info" / "grafts") in protected and str(repo / ".git" / "shallow") in protected
    assert str(repo / ".git" / "packed-refs") not in protected, "an agent's own commits and gc write packed-refs"
