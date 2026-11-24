"""The engine's immutable checkout of the revision a completion claim names.

Check rows, the evaluator's rows, the application those rows exercise and the full-test gate all run from one
detached checkout of the engine's head (`flow/final_head.py`): the engine's own integration of the approved work, in
its own store, never a workspace's HEAD or a branch a role or a detached survivor can move (review 11). The engine
makes the checkout in the workspace root, outside every agent's sandbox, and every command it runs there (setup,
application, rows, full tests) is caged with the source and the Git folder read-only. Caches, virtual environments,
`node_modules` and build output go to a writable folder the engine makes
outside the checkout, under the rows root that every other cage denies: the cage points the tools' cache and build
variables there (`SAKURA_FLOW_BUILD_DIR` names it), and the dependency folders a project keeps beside its source
(`DEPENDENCY_FOLDERS`, when the revision does not track them) are links into it. `workspace.setup` runs once in the
checkout, caged the same way, to install into those folders.

Nothing of a live workspace is used: an ignored `.venv`, `node_modules` or generated file there never reaches the
application, a row or the full tests. After every run the engine checks the checkout again (HEAD, every file, ignored
ones included, and its links); anything else gives no receipt and no green full gate.

Not even the workspaces' object database is used: the checkout is a worktree of the engine's store, where every object
was hashed on the way in (`flow/sealed.py`). Calibration runs the same way on the engine's checkout of its base build
(`base_build`): the sealed base plus the contract flows the engine committed, never the base workspace (review 12).
"""

from __future__ import annotations

import contextlib
import os
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from sakura.flow import cage, checks, contract, final_head, gitutil, owner_marks, pinned_roots, task_repository, workspaces
from sakura.flow.engine import FlowContext
from sakura.flow.gitutil import GitError, git, run as git_run

# Folders projects keep beside their source for dependencies and build output: links into the build folder.
DEPENDENCY_FOLDERS = ("node_modules", ".venv", "dist", ".next")
CHECKOUT_PREFIX = ".sakura-row-"
STATE_KEY = "completion_checkout"
SETUP_NAME = "completion"


class CompletionRefused(RuntimeError):
    """The revision cannot be captured, or what ran no longer matches it; nothing it produced counts."""


@dataclass(frozen=True)
class Checkout:
    """A read-only checkout of `revision`, the writable folder its runs use and the dependency folders linked there;
    `store` is the engine's object store it was made from."""

    revision: str
    source: Path
    build: Path
    links: tuple[str, ...] = ()
    store: Path | None = None


def row_proven(state: dict, row: contract.Row, steps, digest: str, head: str, root: Path | None) -> bool:
    """A green row whose proof names this contract and this head: the engine's receipt (with `root`, the task folder,
    only while the log it names is unchanged), or the person's ok mark. Delivery and progress share this test."""
    if not row.passes:
        return False
    prover = contract.prover_of(row, steps)
    if prover == contract.OWNER:
        return owner_marks.holds(state, row, digest, head)
    space = checks.RECEIPTS_KEY if prover == contract.CHECK else checks.EVALUATOR_RECEIPTS_KEY
    return checks.receipt_passes(state, row, digest, head, space=space, root=root)


@contextlib.contextmanager
def opened(ctx: FlowContext, candidate: str = "") -> Iterator[Checkout]:
    """The completion checkout of the engine's head, set up and recorded for the evaluator's row tool.

    `candidate`: an evaluation fix that is not integrated yet, checked out instead for the full tests that decide
    whether it is; it must build on the engine's head. Raises CompletionRefused when the head does not check out
    against the store or the candidate does not build on it, and WorkspaceError when a setup command fails. The
    checkout and its build folder are removed afterwards.
    """
    try:
        store, head = final_head.proved(ctx)
        if candidate:
            final_head.sealed_candidate(ctx, candidate)
            if not final_head.builds_on(store, head, candidate):
                raise CompletionRefused(f"{candidate[:12]} does not build on the engine's head {head[:12]}")
    except gitutil.GitError as error:
        raise CompletionRefused(str(error)) from error
    with _held(ctx, store, candidate or head, f"{ctx.store.task_id}-{os.getpid()}") as checkout:
        yield checkout


@contextlib.contextmanager
def base_build(ctx: FlowContext) -> Iterator[Checkout]:
    """The checkout of the engine's base build (`final_head.base_build`) calibration runs on, set up and recorded for
    the evaluator's row tool like `opened`; the base workspace is only the evaluator's view. Raises CompletionRefused
    when the engine's head does not check out against the store, and WorkspaceError when a setup command fails."""
    try:
        store, revision = final_head.base_build(ctx)
    except gitutil.GitError as error:
        raise CompletionRefused(str(error)) from error
    with _held(ctx, store, revision, f"{ctx.store.task_id}-base-{os.getpid()}") as checkout:
        yield checkout


@contextlib.contextmanager
def at(store: Path, revision: str, parent: Path, name: str) -> Iterator[Checkout]:
    """A detached checkout of `revision`, which the store `store` holds, in `parent` with a fresh build folder;
    removed afterwards."""
    source = parent / f"{CHECKOUT_PREFIX}{name}"
    build = cage.new_build_dir()
    try:
        parent.mkdir(parents=True, exist_ok=True)
        git(store, "worktree", "add", "--detach", str(source), revision)
        yield Checkout(revision, source, build, _link_dependency_folders(source, build), store)
    finally:
        _remove(store, source)
        pinned_roots.release(source)
        shutil.rmtree(build, ignore_errors=True)
        pinned_roots.release(build)


def recorded(state: dict) -> Checkout | None:
    """The completion checkout the engine recorded for this task, or None while there is none."""
    entry = state.get(STATE_KEY)
    if not isinstance(entry, dict) or not entry.get("store"):
        return None
    store = Path(entry["store"])
    gitutil.trust_store(store)
    return Checkout(entry["revision"], Path(entry["source"]), Path(entry["build"]), tuple(entry["links"]), store)


def require_unchanged(checkout: Checkout) -> None:
    """Raise CompletionRefused when the checkout differs from its revision."""
    changed = changes(checkout)
    if changed:
        raise CompletionRefused(f"the checkout of {checkout.revision[:12]} changed during the run ({changed})")


def changes(checkout: Checkout) -> str:
    """What makes the checkout differ from its revision, or '': a moved HEAD, a changed, new or removed file (ignored
    ones included), a tracked file whose content differs, or a dependency link that no longer points into the build
    folder."""
    if git(checkout.source, "rev-parse", "HEAD").strip() != checkout.revision:
        return f"its HEAD moved off {checkout.revision[:12]}"
    for name in checkout.links:
        link = checkout.source / name
        if not link.is_symlink() or os.readlink(link) != str(checkout.build / name):
            return f"{name} no longer links into the build folder"
    expected = {f"{code} {name}" for name in checkout.links for code in ("??", "!!")}
    status = [line for line in git(checkout.source, "status", "--porcelain", "--untracked-files=all",
                                   "--ignored").splitlines() if line not in expected]
    if status:
        return ", ".join(line[3:] for line in status[:5])
    if git_run(checkout.source, "diff", "--quiet", "--no-ext-diff", "--no-textconv", "HEAD", "--").returncode:
        return "tracked files differ from the revision"
    return ""


@contextlib.contextmanager
def _held(ctx: FlowContext, store: Path, revision: str, name: str) -> Iterator[Checkout]:
    """A checkout of `revision` in the workspace root, recorded in the state while it is held and set up once."""
    root = task_repository.workspace_root(ctx.config.values, ctx.repo)
    with at(store, revision, root, name) as checkout:
        ctx.store.update_state(lambda state: state.update({STATE_KEY: _record(checkout)}))
        try:
            workspaces.run_setup(ctx, SETUP_NAME, checkout.source, checkout.build)
            yield checkout
        finally:
            ctx.store.update_state(lambda state: state.pop(STATE_KEY, None))


def _record(checkout: Checkout) -> dict:
    return {"revision": checkout.revision, "source": str(checkout.source), "build": str(checkout.build),
            "links": list(checkout.links), "store": str(checkout.store)}


def _link_dependency_folders(source: Path, build: Path) -> tuple[str, ...]:
    """Link every dependency folder the revision does not track to an empty folder of its own in the build folder."""
    linked = []
    for name in DEPENDENCY_FOLDERS:
        if os.path.lexists(source / name):
            continue
        (build / name).mkdir()
        (source / name).symlink_to(build / name)
        linked.append(name)
    return tuple(linked)


def _remove(store: Path, source: Path) -> None:
    if not os.path.lexists(source):
        return
    try:
        git(store, "worktree", "remove", "--force", str(source))
    except GitError:
        shutil.rmtree(source, ignore_errors=True)
        git(store, "worktree", "prune")
