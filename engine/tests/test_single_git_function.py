"""Every Git call of the codebase is built by one function (`git_program.command`), so none can skip the pinned
program, the hardened config, the scrubbed environment (no replacement objects, no graft file) or the refusal of
`info/grafts`. This test reads the source tree and fails when another module starts Git itself."""

import ast
from pathlib import Path

import pytest

from conftest import git
from sakura import git_program
from sakura.flow import gitutil

LIB = Path(__file__).resolve().parents[1] / "lib" / "sakura"
# The module that builds and spawns a Git call, and the one that adds the engine's trust checks around it.
IMPLEMENTATION = {LIB / "git_program.py", LIB / "flow" / "gitutil.py"}
SPAWNERS = {"run", "Popen", "call", "check_call", "check_output", "execv", "execvp", "execve", "execvpe", "spawnv",
            "spawnvp", "system", "popen"}
GIT_RESOLVERS = {"program_for", "for_repository", "program_environment"}


def _sources():
    return [path for path in sorted(LIB.rglob("*.py")) if path not in IMPLEMENTATION]


def _names_git(node: ast.AST) -> bool:
    """Whether `node` is the program `git`: the literal, or a call that resolves it."""
    if isinstance(node, ast.Constant) and node.value == "git":
        return True
    if isinstance(node, ast.Call):
        func = node.func
        name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
        return name in GIT_RESOLVERS or (name == "path" and any(_names_git(arg) for arg in node.args))
    return False


def _violations(path: Path, root: Path = LIB) -> list[str]:
    found = []
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.Call):
            func = node.func
            name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
            first = node.args[0] if node.args else None
            literal = isinstance(first, (ast.List, ast.Tuple)) and first.elts and _names_git(first.elts[0])
            if name in SPAWNERS and (literal or (first is not None and _names_git(first))):
                found.append(f"{path.relative_to(root)}:{node.lineno} starts git itself")
            if name == "which" and first is not None and _names_git(first):
                found.append(f"{path.relative_to(root)}:{node.lineno} looks git up on PATH")
            if name in GIT_RESOLVERS:
                found.append(f"{path.relative_to(root)}:{node.lineno} resolves git itself ({name})")
    return found


def test_NoModuleButTheImplementationStartsGitItself():
    problems = [problem for path in _sources() for problem in _violations(path)]

    assert problems == []


def test_TheCheckerCatchesADirectGitInvocation(tmp_path):
    sample = tmp_path / "sample.py"
    sample.write_text("import subprocess\nsubprocess.run(['git', 'status'])\n"
                      "subprocess.run([gitutil.program_for(repo), 'log'])\n"
                      "subprocess.Popen(('git',))\nsubprocess.run(argv)\n")

    assert len(_violations(sample, tmp_path)) == 4


# -- the one function does what the policy says --------------------------------------------------------------------


def test_Command_NoReplaceNoGraftsAndAPinnedGit(repo):
    argv, env = git_program.command(repo, ("status",), {"HOME": "/h", "AWS_SECRET_ACCESS_KEY": "s"})

    assert argv[0] == git_program.path() and argv[-3:] == ["-C", str(repo), "status"]
    assert env["GIT_NO_REPLACE_OBJECTS"] == "1" and env["GIT_GRAFT_FILE"] == "/dev/null"
    assert "-c" in argv and "core.hooksPath=/dev/null" in argv


def test_Command_RefusesWhileInfoGraftsExists(repo):
    (repo / ".git" / "info").mkdir(exist_ok=True)
    (repo / ".git" / "info" / "grafts").write_text(f"{git(repo, 'rev-parse', 'HEAD')}\n")

    with pytest.raises(git_program.GitProgramError, match="info/grafts"):
        git_program.command(repo, ("status",), {})


def test_Command_FailsClosedWhenTheCommonFolderCannotBeResolved(tmp_path):
    with pytest.raises(git_program.GitProgramError, match="no Git repository"):
        git_program.command(tmp_path, ("status",), {})
    (tmp_path / ".git").write_text("not a gitdir pointer\n")
    with pytest.raises(git_program.GitProgramError):
        git_program.command(tmp_path, ("status",), {})


def test_EveryGitHelperOfTheKitRefusesAGraftedRepository(repo):
    from sakura import git_checks, integration, migration_heads, repository

    (repo / ".git" / "info").mkdir(exist_ok=True)
    (repo / ".git" / "info" / "grafts").write_text(f"{git(repo, 'rev-parse', 'HEAD')}\n")

    with pytest.raises(repository.RepositoryError, match="info/grafts"):
        repository.root(repo)
    with pytest.raises(repository.RepositoryError, match="info/grafts"):
        git_checks._git(repo, "status")
    with pytest.raises(RuntimeError, match="info/grafts"):
        integration._git(repo, "status")
    with pytest.raises(migration_heads.MigrationError, match="info/grafts"):
        migration_heads._run(repo, "status")
    with pytest.raises(gitutil.GitError, match="info/grafts"):
        gitutil.git(repo, "status")
