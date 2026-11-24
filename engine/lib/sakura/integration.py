"""Local-only assembly of pull requests on the configured integration branch."""

from __future__ import annotations

import subprocess
from pathlib import Path

from sakura import profile, testing
from sakura.flow import gitutil


BRANCH = "sakura-integration/local"


def run(repo: Path, numbers: list[str], full: bool) -> int:
    """Merge pull request heads locally, run setup, and never configure a real push remote."""
    if not numbers or any(not number.isdigit() for number in numbers):
        raise ValueError("integra requires one or more numeric pull request numbers")
    if _git(repo, "status", "--porcelain"):
        raise RuntimeError("integra requires a clean worktree")
    settings = profile.load(repo).values
    base = settings["integration_branch"]
    _run(repo, "fetch", "origin", base)
    for number in numbers:
        _run(repo, "fetch", "origin", f"+pull/{number}/head:refs/sakura/integration/{number}")
    _run(repo, "switch", "-C", BRANCH, f"origin/{base}")
    _run(repo, "config", f"branch.{BRANCH}.pushRemote", "sakura-no-push")
    for number in numbers:
        if _merge(repo, number):
            return 1
    for command in settings["integration"]["setup"]:
        if subprocess.run(command, shell=True, cwd=repo).returncode:
            return 1
    return testing.run(repo, "full") if full else 0


def _merge(repo: Path, number: str) -> bool:
    done = _call(repo, "merge", "--no-ff", "--no-edit", "-m", f"sakura-integration: #{number}",
                 f"refs/sakura/integration/{number}")
    if not done.returncode:
        return False
    files = _git(repo, "diff", "--name-only", "--diff-filter=U").splitlines()
    print(f"integra: #{number} conflicts: " + ", ".join(files))
    _run(repo, "merge", "--abort")
    return True


def _run(repo: Path, *args: str) -> None:
    done = _call(repo, *args)
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or "git command failed")


def _git(repo: Path, *args: str) -> str:
    done = _call(repo, *args)
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or "git command failed")
    return done.stdout.strip()


def _call(repo: Path, *args: str):
    """One Git call through the engine's single Git function (the person's config applies: `integra` fetches)."""
    try:
        return gitutil.run(repo, *args, person_config=True)
    except gitutil.GitError as error:
        raise RuntimeError(str(error)) from error
