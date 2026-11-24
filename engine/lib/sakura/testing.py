"""Explicit local test execution and tree-stamp recording."""

from __future__ import annotations

import contextlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import asdict
from pathlib import Path
from typing import Callable

from sakura import engine_home, profile, repository, skips, stamps
from sakura.flow import config, gitutil
from sakura.git_checks import _tree

STAMPED_SUITES = ("fast", "focused")
CAGED_TESTS_ENV = "SAKURA_FLOW_EXCHANGE"
CAGED_TESTS_FILE = "tests.json"


def run(repo: Path, suite: str, command_runner: Callable[[str, Path], tuple[int, str]] | None = None) -> int:
    """Run configured commands, report skips, and stamp the tree after success.

    The `fast` suite stamps HEAD's tree; `focused` stamps the index tree.
    """
    commands = _commands(repo, suite)
    if not commands:
        print(f"sakura test {suite}: no commands configured")
        return 0
    commit = _index_commit(repo) if suite == "focused" else _commit(repo)
    tree = _tree(repo, commit)
    with _checkout(repo, commit) as checkout:
        report = _run_commands(commands, checkout, command_runner)
        if report is None:
            return 1
        if _checkout_tree(checkout) != tree:
            print("sakura test: checkout tree changed, stamp refused", file=sys.stderr)
            return 1
    _print_skips(suite, report)
    if suite in STAMPED_SUITES and engine_home.sandboxed():
        print(f"sakura test {suite}: passed (inside an agent cage: no stamp, the engine runs the tests itself)")
        return 0
    if suite in STAMPED_SUITES:
        details = {"skipped": report.count, "entries": [asdict(entry) for entry in report.entries]}
        stamps.record(repo, tree, suite, commands, details)
    print(f"sakura test {suite}: passed")
    return 0


def _commands(repo: Path, suite: str) -> list[str]:
    if engine_home.sandboxed():
        return _caged_commands(suite)
    if suite == "focused":
        return config.load(repo, trusted=True).values["tests"]["focused"]
    return profile.load(repo).values["tests"][suite]


def _caged_commands(suite: str) -> list[str]:
    """Inside an agent cage the engine config is out of reach: the engine put the commands in the run's folder.

    The agent could rewrite that file, but only to change the tests it runs for itself: the engine runs the
    focused tests again with its own config before it accepts the work.
    """
    folder = os.environ.get(CAGED_TESTS_ENV, "")
    try:
        commands = json.loads((Path(folder) / CAGED_TESTS_FILE).read_text()).get(suite, []) if folder else []
    except (OSError, ValueError):
        commands = []
    return [command for command in commands if isinstance(command, str)]


def _run_commands(commands: list[str], checkout: Path,
                  command_runner: Callable[[str, Path], tuple[int, str]] | None = None) -> skips.SkipReport | None:
    """Run each command, echoing output live; return None on the first failure."""
    reports = []
    for command in commands:
        code, output = (command_runner or _run_echoing)(command, checkout)
        reports.append(skips.parse(output))
        if code:
            _print_skips_of(reports)
            return None
    return skips.merge(reports)


def _run_echoing(command: str, cwd: Path) -> tuple[int, str]:
    process = subprocess.Popen(command, shell=True, cwd=cwd, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, errors="replace")
    lines = []
    for line in process.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    return process.wait(), "".join(lines)


def _print_skips_of(reports: list[skips.SkipReport]) -> None:
    for entry in skips.merge(reports).entries:
        print(f"skipped: {entry.path}: {entry.reason}")


def _print_skips(suite: str, report: skips.SkipReport) -> None:
    print(f"sakura test {suite}: {report.count} skipped")
    for entry in report.entries:
        print(f"skipped: {entry.path}: {entry.reason}")


def _index_commit(repo: Path) -> str:
    """Return a throwaway commit object whose tree is the current index."""
    tree = _git_output(repo, "write-tree")
    parents = ["-p", _commit(repo)] if _has_head(repo) else []
    return _git_output(repo, "-c", "user.name=sakura", "-c", "user.email=sakura@localhost",
                       "commit-tree", tree, *parents, "-m", "sakura focused test snapshot")


def _has_head(repo: Path) -> bool:
    return gitutil.run(repo, "rev-parse", "--verify", "-q", "HEAD").returncode == 0


def _git_output(repo: Path, *args: str) -> str:
    done = gitutil.run(repo, *args)
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or f"git {args[0]} failed")
    return done.stdout.strip()


@contextlib.contextmanager
def _checkout(repo: Path, tree: str):
    """Yield a temporary detached checkout of an immutable Git tree."""
    path = Path(tempfile.mkdtemp(prefix="sakura-test-"))
    path.rmdir()
    _git(repo, "worktree", "add", "--detach", "--quiet", str(path), tree)
    try:
        yield path
    finally:
        _git(repo, "worktree", "remove", "--force", str(path), allow_failure=True)
        shutil.rmtree(path, ignore_errors=True)


def _git(repo: Path, *args: str, allow_failure: bool = False) -> None:
    done = gitutil.run(repo, *args)
    if done.returncode and not allow_failure:
        raise RuntimeError(done.stderr.strip() or "git worktree command failed")


def _commit(repo: Path) -> str:
    done = gitutil.run(repo, "rev-parse", "HEAD")
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or "cannot resolve HEAD")
    return done.stdout.strip()


def _checkout_tree(checkout: Path) -> str:
    """Return the tree of tracked files only; untracked run artifacts are ignored."""
    done = gitutil.run(checkout, "add", "-u")
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or "cannot stage test checkout")
    done = gitutil.run(checkout, "write-tree")
    if done.returncode:
        raise RuntimeError(done.stderr.strip() or "cannot calculate test checkout tree")
    return done.stdout.strip()
