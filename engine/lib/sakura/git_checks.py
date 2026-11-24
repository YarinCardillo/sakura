"""Deterministic Git hook checks."""

from __future__ import annotations

import re
import sys
from pathlib import Path, PurePosixPath

from sakura import engine_home, guard, migration_heads, profile, repository, skips, stamps
from sakura.flow import config, gitutil

ZERO = "0" * 40


def pre_commit(repo: Path) -> tuple[bool, str]:
    """Refuse commits on protected branches, without a focused stamp, or over skipped tests."""
    settings = profile.load(repo).values
    current = repository.branch(repo)
    if current in settings["protected_branches"]:
        return False, f"commits on protected branch {current!r} are refused"
    if engine_home.sandboxed():
        # Stamps live in the trust root, which no cage may write; the engine runs the focused tests itself
        # on every commit an agent hands back, before it reviews or merges it.
        print("sakura pre-commit: inside an agent cage, the engine runs the focused tests itself")
        return True, ""
    focused = config.load(repo, trusted=True).values["tests"]["focused"]
    if not focused:
        print("sakura pre-commit: tests.focused is empty, stamp check skipped")
        return True, ""
    return _check_focused_stamp(repo, focused)


def skips_touch(staged: list[str], entries: list[skips.SkipEntry]) -> list[skips.SkipEntry]:
    """Return skip entries on a staged path, in a staged directory, or matching a staged stem."""
    return [entry for entry in entries if _touches(staged, entry)]


def _check_focused_stamp(repo: Path, focused: list[str]) -> tuple[bool, str]:
    stamp = stamps.read(repo, _git(repo, "write-tree"), "focused", focused)
    if stamp is None:
        return False, "no passing focused-test stamp for this tree; run sakura test focused"
    entries = [skips.SkipEntry(item["path"], item["reason"]) for item in stamp.get("entries", [])]
    staged = _git(repo, "diff", "--cached", "--name-only", "-z").split("\0")
    touched = skips_touch([path for path in staged if path], entries)
    if touched:
        return False, "skipped tests in touched area: " + "; ".join(f"{e.path}: {e.reason}" for e in touched)
    return True, ""


def _touches(staged: list[str], entry: skips.SkipEntry) -> bool:
    if not entry.path:
        return True
    entry_path = PurePosixPath(entry.path)
    return any(
        path == entry.path
        or PurePosixPath(path).parent == entry_path.parent
        or _stem(PurePosixPath(path).name) == _stem(entry_path.name) != ""
        for path in staged
    )


def _stem(name: str) -> str:
    """Strip test markers: test_x.py, x_test.py, x.test.ts and x.spec.ts all map to x."""
    return name.split(".")[0].removeprefix("test_").removesuffix("_test")


def pre_push(repo: Path, refs: str) -> tuple[bool, str]:
    """Validate every pushed ref read from Git's pre-push stdin."""
    if guard.any_snapshot_changed(repo):
        return False, "sakura guarded-file fingerprint changed or is unavailable"
    settings = profile.load(repo).values
    if not settings["tests"]["fast"]:
        print("sakura pre-push: tests.fast is empty, stamp check skipped", file=sys.stderr)
    for line in refs.splitlines():
        fields = line.split()
        if len(fields) != 4:
            return False, "invalid pre-push ref input"
        result = _check_ref(repo, settings, *fields)
        if result:
            return False, result
    return True, ""


def _check_ref(repo: Path, settings: dict, local_ref: str, sha: str, remote_ref: str, old: str) -> str:
    remote_branch = _branch_name(remote_ref)
    if not remote_branch:
        return "only branch refs may be pushed"
    if remote_branch in settings["protected_branches"]:
        return f"changing protected branch {remote_branch!r} is refused"
    if remote_branch.startswith("sakura-integration/"):
        return "sakura integration branches are local-only"
    if not re.fullmatch(settings["branch_pattern"], remote_branch):
        return f"destination branch {remote_branch!r} does not match branch_pattern"
    if sha == ZERO:
        return ""
    local_branch = _branch_name(local_ref)
    if not local_branch:
        return "only branch refs may be pushed"
    if local_branch.startswith("sakura-integration/"):
        return "sakura integration branches are local-only"
    if not re.fullmatch(settings["branch_pattern"], local_branch):
        return f"branch {local_branch!r} does not match branch_pattern"
    if _has_integration_merge(repo, sha):
        return "sakura integration merge commits are local-only"
    if settings["require_signed_commits"] and _unsigned(repo, sha, old):
        return "all pushed commits must be signed"
    migration = settings["migrations"]["alembic_versions"]
    if migration:
        try:
            heads = migration_heads.heads(sha, migration, repo)
        except migration_heads.MigrationError as error:
            return f"cannot verify migration heads: {error}"
        if len(heads) > 1:
            return "multiple Alembic heads: " + ", ".join(heads)
    fast = settings["tests"]["fast"]
    if fast and not stamps.has(repo, _tree(repo, sha), "fast", fast):
        return "fast tests have no passing sakura test stamp for this pushed tree"
    return ""


def _branch_name(ref: str) -> str:
    return ref.removeprefix("refs/heads/") if ref.startswith("refs/heads/") else ""


def _has_integration_merge(repo: Path, sha: str) -> bool:
    subjects = _git(repo, "log", "--format=%s", sha).splitlines()
    return any(subject.startswith("sakura-integration:") for subject in subjects)


def _unsigned(repo: Path, sha: str, old: str) -> bool:
    args = ("rev-list", sha, "--not", "--remotes")
    try:
        commits = _git(repo, *args).splitlines()
        for commit in commits:
            _git(repo, "verify-commit", commit)
    except repository.RepositoryError:
        return True
    return False


def _tree(repo: Path, sha: str) -> str:
    return _git(repo, "rev-parse", f"{sha}^{{tree}}")


def _git(repo: Path, *args: str) -> str:
    try:
        done = gitutil.run(repo, *args)
    except gitutil.GitError as error:
        raise repository.RepositoryError(str(error)) from error
    if done.returncode:
        raise repository.RepositoryError(done.stderr.strip() or "git command failed")
    return done.stdout.strip()


def run_hook(name: str, remote_name: str | None = None, remote_url: str | None = None) -> int:
    """Run a named hook for the current repository."""
    try:
        if name == "pre-push" and (remote_name is None or remote_url is None):
            raise repository.RepositoryError("pre-push requires Git remote name and URL")
        repo = repository.root()
        ok, reason = pre_commit(repo) if name == "pre-commit" else pre_push(repo, sys.stdin.read())
    except (profile.ProfileError, config.FlowConfigError, repository.RepositoryError, OSError) as error:
        ok, reason = False, f"sakura check failed closed: {error}"
    if not ok:
        print(f"sakura {name}: {reason}", file=sys.stderr)
        return 1
    return 0
