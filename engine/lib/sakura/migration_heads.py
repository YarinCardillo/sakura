"""Alembic head checks read directly from Git objects."""

from __future__ import annotations

import ast
import subprocess
from pathlib import Path, PurePosixPath

from sakura.flow import gitutil


class MigrationError(RuntimeError):
    """Raised when migration files cannot be read or parsed."""


def heads(commit: str, versions: str, repo: Path = Path.cwd()) -> list[str]:
    """Return Alembic heads under ``versions`` in ``commit`` without a checkout."""
    prefix = str(PurePosixPath(versions)).rstrip("/") + "/"
    paths = _paths(repo, commit, prefix)
    revisions: dict[str, tuple[str, ...]] = {}
    for path in paths:
        revision, down = _revision(_show(repo, commit, path), path)
        if revision:
            if revision in revisions:
                raise MigrationError(f"duplicate revision {revision!r}")
            revisions[revision] = down
    _validate_graph(revisions)
    parents = {parent for down in revisions.values() for parent in down}
    return sorted(set(revisions) - parents)


def _paths(repo: Path, commit: str, prefix: str) -> list[str]:
    done = _run(repo, "ls-tree", "-r", "--name-only", commit, prefix)
    return [path for path in done.stdout.splitlines() if path.endswith(".py")]


def _show(repo: Path, commit: str, path: str) -> str:
    return _run(repo, "show", f"{commit}:{path}").stdout


def _revision(source: str, path: str) -> tuple[str | None, tuple[str, ...]]:
    try:
        nodes = ast.parse(source).body
    except SyntaxError as error:
        raise MigrationError(f"cannot parse {path}: {error}") from error
    values = _metadata_values(nodes, path)
    revision = values.get("revision")
    if revision is not None and not isinstance(revision, str):
        raise MigrationError(f"invalid revision in {path}")
    down = values.get("down_revision")
    if down is not None and not isinstance(down, (str, tuple, list)):
        raise MigrationError(f"invalid down_revision in {path}")
    parents = () if down is None else ((down,) if isinstance(down, str) else tuple(down))
    if not all(isinstance(parent, str) for parent in parents):
        raise MigrationError(f"invalid down_revision in {path}")
    return revision, parents


def _metadata_values(nodes: list[ast.stmt], path: str) -> dict[str, object]:
    values: dict[str, object] = {}
    for node in nodes:
        for name, value in _metadata_assignments(node, path):
            if name in values:
                raise MigrationError(f"cannot resolve {name} unambiguously in {path}")
            try:
                values[name] = ast.literal_eval(value)
            except ValueError as error:
                raise MigrationError(f"cannot resolve {name} in {path}") from error
        if _has_nested_metadata_binding(node):
            raise MigrationError(f"cannot resolve migration metadata unambiguously in {path}")
    return values


def _metadata_assignments(node: ast.AST, path: str) -> list[tuple[str, ast.expr]]:
    names = {"revision", "down_revision"}
    if isinstance(node, ast.Assign):
        assigned = [target.id for target in node.targets if isinstance(target, ast.Name) and target.id in names]
        if _has_complex_metadata_target(node.targets, names):
            raise MigrationError(f"cannot resolve migration metadata unambiguously in {path}")
        return [(name, node.value) for name in assigned]
    if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name) and node.target.id in names:
        if node.value is None:
            raise MigrationError(f"cannot resolve {node.target.id} in {path}")
        return [(node.target.id, node.value)]
    if isinstance(node, (ast.AugAssign, ast.NamedExpr)) and isinstance(node.target, ast.Name) and node.target.id in names:
        raise MigrationError(f"cannot resolve {node.target.id} in {path}")
    return []


def _has_complex_metadata_target(targets: list[ast.expr], names: set[str]) -> bool:
    for target in targets:
        if not isinstance(target, ast.Name) and any(node.id in names for node in ast.walk(target) if isinstance(node, ast.Name)):
            return True
    return False


def _has_nested_metadata_binding(node: ast.stmt) -> bool:
    children = _value_children(node)
    return any(_binds_metadata(child) for child in children for child in ast.walk(child))


def _value_children(node: ast.stmt) -> list[ast.AST]:
    if isinstance(node, ast.Assign):
        return [node.value]
    if isinstance(node, ast.AnnAssign):
        return [node.value] if node.value else []
    return list(ast.iter_child_nodes(node))


def _binds_metadata(node: ast.AST) -> bool:
    names = {"revision", "down_revision"}
    if isinstance(node, ast.Name):
        return node.id in names and isinstance(node.ctx, (ast.Store, ast.Del))
    if isinstance(node, ast.arg):
        return node.arg in names
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return node.name in names
    if isinstance(node, ast.ExceptHandler):
        return node.name in names
    if isinstance(node, (ast.Import, ast.ImportFrom)):
        return any((alias.asname or alias.name.rpartition(".")[2]) in names for alias in node.names)
    if isinstance(node, ast.MatchAs):
        return node.name in names
    if isinstance(node, ast.MatchStar):
        return node.name in names
    if isinstance(node, ast.MatchMapping):
        return node.rest in names
    return False


def _validate_graph(revisions: dict[str, tuple[str, ...]]) -> None:
    if not revisions:
        raise MigrationError("no Alembic revisions found")
    unknown = {parent for parents in revisions.values() for parent in parents if parent not in revisions}
    if unknown:
        raise MigrationError("unknown down_revision: " + ", ".join(sorted(unknown)))
    for revision in revisions:
        _visit(revision, revisions, set(), set())


def _visit(revision: str, graph: dict[str, tuple[str, ...]], active: set[str], complete: set[str]) -> None:
    if revision in complete:
        return
    if revision in active:
        raise MigrationError("cycle in Alembic revisions")
    active.add(revision)
    for parent in graph[revision]:
        _visit(parent, graph, active, complete)
    active.remove(revision)
    complete.add(revision)


def _run(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    try:
        done = gitutil.run(repo, *args)
    except gitutil.GitError as error:
        raise MigrationError(str(error)) from error
    if done.returncode:
        raise MigrationError(done.stderr.strip() or "git object read failed")
    return done
