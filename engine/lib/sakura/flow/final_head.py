"""The head the engine proves and delivers: its own integration of the approved work, made in its own store.

Roles write the task's repository (its refs, its objects, every workspace's HEAD and files), and a detached survivor of
a run keeps that access after the run (review 11, finding 1): it can wait for the approvals, add a commit with Git's
plumbing and move the task branch. So no step after an approval reads a ref, a branch or a workspace to learn what to
prove or deliver. The engine records the base and the exact commit each approval named (the SHA the reviewer approved,
never a branch), seals each into its own store (`flow/sealed.py`: every object hashed on the way in) and integrates
them there itself:

* a build task is merged with `--no-ff` in a worktree of the store that the engine makes for that one merge at a fresh
  path under the engine home, which every cage denies, and removes afterwards;
* an evaluation fix, built on the engine's head and approved, becomes the head itself when it descends from it;
* the contract flows are a step too, the first one: the engine commits them itself on the base, in a fresh worktree
  of its store, from the bytes it read once (`flow/phases/contract.py`); that commit is what the base run ran on and
  what becomes the step, never a commit or a file list read from workspace 0. Every flow path must pass the host's
  allowlist and the hard denials (`flow/flow_paths.py`); the step records each file's blob and the allowlist.

`state["integration"]` records each step, and `proved` checks the head against the store before anything is tested,
receipted, delivered or reported: each merge step is a commit whose parents are exactly the previous head and the
approved commit, each fix step is the approved commit itself, descending from the previous head, and the chain starts
at the recorded base. A head that is anything else gets no receipt and is never pushed. Whatever a role or a survivor
wrote into the task's repository after an approval is ignored; workspace 0 is moved to the engine's head (`show`) as a
view for the person and the evaluator only, never read back.
"""

from __future__ import annotations

import contextlib
import os
import shutil
import uuid
from pathlib import Path
from typing import Iterator

from sakura import beneath, engine_home
from sakura.flow import agent_messages, flow_paths, gitutil, sealed, task_repository
from sakura.flow.gitutil import GitError, git

STATE_KEY = "integration"
FOLDER = "integration"
MERGE, FIX, FLOWS = "merge", "fix", "flows"
# Where the engine's head is put in the task's repository, so a workspace can be made at it or moved to it.
PUBLISHED_REF = "refs/sakura/integration"
# No maintenance runs in the engine's store after a merge, as after a seal.
NO_MAINTENANCE = ("-c", "gc.auto=0", "-c", "maintenance.auto=false")
FOLDER_MODE = 0o700
FILE_MODE, EXECUTABLE_MODE = 0o644, 0o755
FLOWS_REVIEW_KEY = "flows_review"
APPROVED = "APPROVE"
REVIEW_OFF = {"off": "contract flows review is off"}
LEGACY_REVIEW_OFF = {"off": "code review is off in this preset"}


class MergeBlocked(RuntimeError):
    """An approved commit cannot be integrated; the message names the cause and the files."""


class MergeConflict(MergeBlocked):
    """An approved commit conflicts with the head in `files`: work for a builder, not a dead end (`phases/build.py`)."""

    def __init__(self, label: str, files: list[str]) -> None:
        super().__init__(f"merging {label} conflicts in {', '.join(files)}")
        self.files = files


class HeadRefused(GitError):
    """The recorded head is not exactly the engine's integration of the approved commits; nothing is proved from it."""


def recorded(state: dict) -> str:
    """The engine's head as its state records it (the base before any step), unchecked: for a start point and a
    digest only. What is proved or delivered goes through `proved`."""
    steps = _steps(state)
    return steps[-1]["head"] if steps else state.get("base_commit", "")


def merge(ctx, label: str, title: str, approved: str) -> str:
    """Merge the approved commit `approved` onto the engine's head with `--no-ff` and make the result the head.

    The commit is sealed from the task's repository first; the merge runs in a fresh worktree of the store under the
    engine home (removed afterwards), so no ref, file or index a role could write takes part. Nothing happens for a
    commit the head already holds (a task that needed no change). Raises MergeBlocked on a conflict, naming the files,
    and SealRefused when a commit cannot be brought into the store intact.
    """
    state = ctx.store.read_state()
    onto = recorded(state)
    store = _seal(ctx.repo, state, onto, approved)
    if _is_ancestor(store, approved, onto):
        return onto
    with _integration_worktree(ctx, store, onto) as tree:
        try:
            git(tree, *NO_MAINTENANCE, "merge", "--no-ff", "-m", f"flow: merge {label} {title}", approved)
        except GitError:
            files = git(tree, "diff", "--name-only", "--diff-filter=U").split()
            if not files:
                raise
            raise MergeConflict(label, files) from None
        head = git(tree, "rev-parse", "HEAD").strip()
    _keep(store, head)
    _record(ctx, {"kind": MERGE, "label": label, "approved": approved, "head": head})
    return head


def conflicts(ctx, store: Path, head: str, other: str) -> list[str]:
    """The files in which merging the commit `other` into `head` conflicts, empty when it merges cleanly. Both are
    commits of the engine's store; the merge runs in a fresh worktree of it, which is removed, and records nothing."""
    with _integration_worktree(ctx, store, head) as tree:
        try:
            git(tree, *NO_MAINTENANCE, "merge", "--no-commit", "--no-ff", other)
        except GitError:
            files = [name for name in git(tree, "diff", "--name-only", "--no-renames", "-z", "--diff-filter=U").split("\0") if name]
            if not files:
                raise
            return files
    return []


def holds(store: Path, revision: str) -> bool:
    """Whether the engine's store holds the commit `revision`."""
    return _held(store, revision)


def descends(store: Path, ancestor: str, descendant: str) -> bool:
    """Whether `descendant` is `ancestor` or builds on it, both read from the engine's store."""
    return _is_ancestor(store, ancestor, descendant)


def fix(ctx, label: str, approved: str) -> str:
    """Make the approved evaluation fix `approved`, built on the engine's head, the new head.

    Raises MergeBlocked when it does not descend from the head (it was not built on what was proved) and SealRefused
    when it cannot be brought into the store intact.
    """
    state = ctx.store.read_state()
    onto = recorded(state)
    store = _seal(ctx.repo, state, onto, approved)
    if not _is_ancestor(store, onto, approved):
        raise MergeBlocked(f"{label} at {approved[:12]} does not build on the engine's head {onto[:12]}; it is not "
                           "integrated")
    _record(ctx, {"kind": FIX, "label": label, "approved": approved, "head": approved})
    return approved


def flows_candidate(ctx, files: dict[str, tuple[bytes, bool]], allowed: list[str], message: str) -> str:
    """Commit the contract flow files `files` (name to bytes and executable bit, read once by the engine) on the
    base, in a fresh worktree of the store, and return the commit (the base for no file): the checkout the base run
    runs on and, once it passed, the flows step (`record_flows`). Nothing is recorded here. Raises MergeBlocked for a
    path the host allowlist or the hard denials refuse (`flow/flow_paths.py`)."""
    refused = [reason for reason in (flow_paths.refusal(name, allowed) for name in sorted(files)) if reason]
    if refused:
        raise MergeBlocked("; ".join(refused))
    state = ctx.store.read_state()
    base = state["base_commit"]
    store = _seal(ctx.repo, state, base)
    if not files:
        return base
    clashes = flow_paths.collisions(sorted(files), _tracked(store, base))
    if clashes:
        raise MergeBlocked("; ".join(clashes))
    with _integration_worktree(ctx, store, base) as tree:
        for name, (data, executable) in sorted(files.items()):
            beneath.write_bytes(tree, name, data, EXECUTABLE_MODE if executable else FILE_MODE)
        git(tree, "add", "--", *sorted(files))
        git(tree, *NO_MAINTENANCE, "commit", "-q", "--no-verify", "-m", message)
        head = git(tree, "rev-parse", "HEAD").strip()
    _keep(store, head)
    return head


def approve_flows(ctx, commit: str) -> dict:
    """Record the code reviewer's approval of the flows commit `commit` on the base, the receipt `record_flows` needs;
    returns it. Called only once that reviewer approved exactly that commit."""
    receipt = flows_receipt(ctx.store.read_state()["base_commit"], commit)
    ctx.store.update_state(lambda state: state.update({FLOWS_REVIEW_KEY: receipt}))
    return receipt


def flows_receipt(base: str, commit: str) -> dict:
    """The code review receipt of the flows commit `commit` on `base`."""
    return {"base": base, "head": commit, "verdict": APPROVED}


def flows_review_required(ctx) -> bool:
    """The contract review belongs to Plan review and uses the lane's enabled code reviewer."""
    return ctx.steps.plan_review and ctx.steps.code_review


def record_flows(ctx, commit: str, allowed: list[str]) -> str:
    """Make the flows candidate `commit` (`flows_candidate`) the first step of the engine's head, once its base run
    passed and, when review is required, the code reviewer approved exactly that commit (`approve_flows`). No action
    when it is already integrated or is the base. Raises MergeBlocked when the approval is missing, the head already
    has steps or the commit is not exactly the engine's additions of allowed flow files on the base."""
    state = ctx.store.read_state()
    base = state["base_commit"]
    steps = _steps(state)
    if commit == base or any(step.get("kind") == FLOWS and step["head"] == commit for step in steps):
        return recorded(state)
    if steps:
        raise MergeBlocked("the contract changed after its flows were integrated; start the task again with --new")
    review = state.get(FLOWS_REVIEW_KEY) if flows_review_required(ctx) else REVIEW_OFF
    if review not in (REVIEW_OFF, flows_receipt(base, commit)):
        raise MergeBlocked(f"the contract flows {commit[:12]} have no code review approval of exactly that commit on "
                           "the base; nothing of them is integrated")
    store = _seal(ctx.repo, state, base)
    names = git(store, "diff", "--name-only", "--no-renames", "-z", base, commit).split("\0")
    blobs = {name: git(store, "rev-parse", f"{commit}:{name}").strip() for name in sorted(filter(None, names))}
    step = {"kind": FLOWS, "label": "contract flows", "approved": commit, "head": commit, "files": blobs,
            "flow_paths": list(allowed), "review": dict(review)}
    problem = _step_problem(store, base, step)
    if problem:
        raise MergeBlocked(f"the contract flows commit is not the engine's: {problem}")
    _record(ctx, step)
    return commit


def flow_blobs(state: dict) -> dict[str, str]:
    """The flow files of the engine's flows step and their blobs ({} without one)."""
    found: dict[str, str] = {}
    for step in _steps(state):
        if step.get("kind") == FLOWS:
            found.update(step.get("files") or {})
    return found


def flows_review_text(state: dict) -> str:
    """What a reviewer is told about the contract flows the engine committed before the build: they ship with the
    change, so they are part of what it approves ('' without a flows step)."""
    head = _flows_head(state)
    if not head:
        return ""
    return (f"The contract flows the engine committed before the build ship with this change too; review them as part "
            f"of it: git diff {state['base_commit']}..{head}\n")


def base_build(ctx) -> tuple[Path, str]:
    """The engine's store and its base build, checked against the store like `proved`: the flows step's commit, the
    sealed base plus the contract flows the engine committed (what the contract's base run ran on), or the base
    without one. Calibration runs there. Raises HeadRefused."""
    state = ctx.store.read_state()
    store, _head = proved_for(state, ctx.repo)
    return store, _flows_head(state) or state["base_commit"]


def proved(ctx) -> tuple[Path, str]:
    """The engine's store and its head, checked against the store (see the module text); raises HeadRefused."""
    return proved_for(ctx.store.read_state(), ctx.repo)


def proved_for(state: dict, repo: Path) -> tuple[Path, str]:
    """`proved` for the task whose state is `state`, of the person's repository `repo` (a task another one is stacked
    on, `start --after`)."""
    store = _seal(repo, state, state["base_commit"])
    previous = state["base_commit"]
    for step in _steps(state):
        problem = _step_problem(store, previous, step)
        if problem:
            raise HeadRefused(f"the recorded head {step['head'][:12]} is not the engine's integration of the approved "
                              f"work ({problem}); nothing is proved or delivered from it")
        previous = step["head"]
    return store, previous


def sealed_candidate(ctx, *revisions: str) -> Path:
    """Seal the base and `revisions`, commits of the task's repository, into the store and return the store: an
    approved commit and a fix candidate are checked there (protected paths, test-only markers), never in a workspace."""
    state = ctx.store.read_state()
    return _seal(ctx.repo, state, state["base_commit"], *revisions)


@contextlib.contextmanager
def review_copy(ctx, purpose: str, start: str, head: str) -> Iterator[tuple[Path, Path]]:
    """A repository of the reviewer's own holding `start` and `head`, sealed into the engine's store first and fetched
    from there, its worktree at `head`; yields (repository, worktree) and removes both afterwards. What a reviewer
    approves is then exactly what the engine integrates: a survivor rewriting the task's objects or moving its refs
    during the review changes nothing the reviewer reads (`task_repository.make_role_repository`). Raises
    SealRefused when a commit cannot be brought into the store intact."""
    store = sealed_candidate(ctx, start, head)
    repository, worktree = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, purpose, store,
                                                                (start, head))
    try:
        yield repository, worktree
    finally:
        _keep_or_remove_review_copy(ctx, repository, worktree)


def _keep_or_remove_review_copy(ctx, repository: Path, worktree: Path) -> None:
    """End of a review: keep the copy when a message for the reviewer waits (its worker removes the copy after delivering it),
    otherwise remove it. Decided under the guard of the reviewer's queue, so a message being held cannot slip in between."""
    from sakura.flow import agent_resume  # imported here: it imports the runner

    with agent_resume.workspace_guard(ctx.store, str(worktree)):
        if agent_messages.needs_workspace(ctx.store.read_state(), str(worktree)):
            task_repository.retain(ctx.store, repository)
        else:
            task_repository.remove_role_repository(ctx.store, repository)


def builds_on(store: Path, head: str, candidate: str) -> bool:
    """Whether `candidate` descends from `head`, both read from the engine's store."""
    return _is_ancestor(store, head, candidate)


def show(ctx, workspace: Path, branch: str) -> None:
    """Put the engine's head into the task's repository and check it out in `workspace` on `branch`, the view the
    person and the evaluator look at; nothing reads it back. Git refuses (GitError) when a change there would be lost."""
    head = recorded(ctx.store.read_state())
    if git(workspace, "rev-parse", "HEAD").strip() == head:
        return
    publish(ctx, head)
    git(workspace, "checkout", "-q", "-B", branch, head)


def _require_no_untracked_in_the_way(workspace: Path, old: str, head: str) -> None:
    """Raise GitError when a file the engine's head adds over `old` exists in the work tree untracked, ignored or not:
    it is the person's own and a restore never overwrites it (a two-way `read-tree` replaces an ignored one)."""
    # --no-renames: with rename detection a path the head gains can be reported as a rename, never as an add.
    added = git(workspace, "diff", "--no-renames", "--name-only", "-z", "--diff-filter=A", old, head).split("\0")
    for name in filter(None, added):
        if os.path.lexists(workspace / name) or _blocked_folder(workspace, name):
            raise GitError(f"{name} exists in the workspace and is not tracked there; move it away, then restore")


def _blocked_folder(workspace: Path, name: str) -> bool:
    """Whether a leading folder of `name` exists in the work tree as a file or a link, where the head needs a folder."""
    folder = workspace
    for part in Path(name).parts[:-1]:
        folder = folder / part
        if folder.is_symlink() or (os.path.lexists(folder) and not folder.is_dir()):
            return True
    return False


class WorkspaceMoved(GitError):
    """The workspace's branch is no longer where the person confirmed it was; nothing was moved."""


def restore(ctx, workspace: Path, branch: str, expected_old: str) -> None:
    """Put `branch` of `workspace` back on the engine's head, only if it still points at `expected_old`.

    The branch moves by Git's own compare-and-swap (`update-ref <ref> <head> <expected_old>`), so a commit the person
    makes in the workspace meanwhile (which takes no task lock) is never reset: a failed swap raises WorkspaceMoved and
    nothing changes. The work tree then follows with a two-way `read-tree -m -u`, which cannot move a ref and refuses to
    overwrite an untracked or ignored file or a local change; when it refuses, the branch is swapped back. Raises GitError
    for anything else Git refuses, WorkspaceMoved for the swap.
    """
    head = recorded(ctx.store.read_state())
    if expected_old == head:
        return
    reference = f"refs/heads/{branch}"
    if git(workspace, "symbolic-ref", "-q", "HEAD").strip() != reference:
        raise GitError(f"workspace 0 is not on the branch {branch}; check it out and try again")
    publish(ctx, head)
    _require_no_untracked_in_the_way(workspace, expected_old, head)
    original = _adopt_edits_that_are_the_heads(workspace, expected_old, head)
    try:
        _move_branch_and_work_tree(workspace, reference, branch, expected_old, head)
    except BaseException:
        _put_back_index(workspace, original)
        raise


def _move_branch_and_work_tree(workspace: Path, reference: str, branch: str, expected_old: str, head: str) -> None:
    if gitutil.run(workspace, "update-ref", reference, head, expected_old).returncode:
        raise WorkspaceMoved(f"the branch {branch} is no longer at {expected_old[:12]}")
    done = gitutil.run(workspace, "read-tree", "-m", "-u", expected_old, head)
    if done.returncode:
        gitutil.run(workspace, "update-ref", reference, expected_old, head)
        raise GitError((done.stderr or done.stdout).strip() or "Git could not update the work tree")


def _adopt_edits_that_are_the_heads(workspace: Path, old: str, head: str) -> dict[str, str]:
    """Stage a file that differs between `old` and `head`, is edited in the work tree and holds exactly the head's bytes
    (the state a blocked contract left: the head's own flow file as an uncommitted edit over an older flows commit):
    nothing of the person's is lost by moving to the head, and Git's two-way merge keeps an index entry equal to the new
    one. An edit with any other bytes stays, and the restore refuses over it. A file with staged changes is never adopted
    (that would replace the person's staged content): the restore refuses (GitError) before anything is staged. Returns
    the index entries the adopted files had (`_put_back_index`), for a restore that fails later."""
    changed = [name for name in git(workspace, "diff", "--no-renames", "--name-only", "-z", old, head).split("\0") if name]
    staged = [name for name in changed if git(workspace, "diff", "--cached", "--name-only", "--", name).strip()]
    if staged:
        raise GitError(f"{', '.join(staged[:5])} has staged changes in the workspace; commit or unstage them, then restore")
    original = {}
    for name in changed:
        if not os.path.isfile(workspace / name) or os.path.islink(workspace / name):
            continue
        wanted = gitutil.run(workspace, "rev-parse", f"{head}:{name}")
        local = gitutil.run(workspace, "hash-object", "--", name)
        if not wanted.returncode and not local.returncode and wanted.stdout.strip() == local.stdout.strip():
            original[name] = git(workspace, "ls-files", "-s", "--", name).rstrip("\n")
            git(workspace, "add", "--", name)
    return original


def _put_back_index(workspace: Path, original: dict[str, str]) -> None:
    """Give the adopted files their index entries back (`_adopt_edits_that_are_the_heads`); the work tree is not touched."""
    for name, entry in original.items():
        if entry:
            description, _, path = entry.partition("\t")
            mode, blob, _stage = description.split()
            gitutil.run(workspace, "update-index", "--add", "--cacheinfo", f"{mode},{blob},{path}")
        else:
            gitutil.run(workspace, "update-index", "--force-remove", "--", name)


def publish(ctx, head: str) -> None:
    """Bring `head` from the engine's store into the task's repository (`PUBLISHED_REF`), so a workspace can be made
    at it. Raises GitError when the fetch fails."""
    state = ctx.store.read_state()
    if head == state["base_commit"]:
        return  # the task's repository was made with it
    done = gitutil.run(_source(state), *task_repository.FETCH_CONFIG, "fetch", *task_repository.FETCH_OPTIONS,
                       sealed.file_url(sealed.path(ctx.repo)), f"+{head}:{PUBLISHED_REF}", local_transfer=True)
    if done.returncode:
        raise GitError(f"the engine's head {head[:12]} could not be put into the task's repository: "
                       f"{(done.stderr or done.stdout).strip()}")


def _flows_head(state: dict) -> str:
    heads = [step["head"] for step in _steps(state) if step.get("kind") == FLOWS]
    return heads[-1] if heads else ""


def _steps(state: dict) -> list[dict]:
    return list((state.get(STATE_KEY) or {}).get("steps") or [])


def _record(ctx, step: dict) -> None:
    def change(state: dict) -> None:
        record = state.setdefault(STATE_KEY, {"steps": []})
        record["steps"] = [*record.get("steps", []), step]

    ctx.store.update_state(change)
    ctx.store.append_ledger("integration", f"{step['label']}: the engine's head is {step['head'][:12]}")
    # Imported here: the overlap notice reads lane heads through this module.
    from sakura.flow import overlap

    overlap.announce(ctx)


def _step_problem(store: Path, previous: str, step: dict) -> str:
    """Why `step` is not what the engine made on `previous`, or ''."""
    head, approved = step.get("head", ""), step.get("approved", "")
    parents = gitutil.run(store, "rev-list", "--no-walk", "--parents", f"{head}^{{commit}}")
    if parents.returncode:
        return f"the store does not hold {head[:12]}"
    if step.get("kind") == MERGE:
        return "" if parents.stdout.split()[1:] == [previous, approved] else f"{head[:12]} is not the merge of " \
                                                                            f"{approved[:12]} onto {previous[:12]}"
    if step.get("kind") == FIX:
        if head != approved or not _is_ancestor(store, previous, head):
            return f"{head[:12]} is not the approved fix {approved[:12]} built on {previous[:12]}"
        return ""
    if step.get("kind") == FLOWS:
        return _flows_problem(store, previous, step, parents.stdout.split()[1:])
    return f"unknown step {step.get('kind')!r}"


def _flows_problem(store: Path, previous: str, step: dict, parents: list[str]) -> str:
    """Why `step` is not the engine's commit of exactly its recorded flow files (names and blobs) on `previous`, with
    the code reviewer's approval of that commit (or a record that the review was off), or ''."""
    head, files = step["head"], step.get("files") or {}
    if head != step.get("approved") or parents != [previous] or not isinstance(files, dict):
        return f"{head[:12]} is not a commit on {previous[:12]} alone"
    if step.get("review") not in (REVIEW_OFF, LEGACY_REVIEW_OFF, flows_receipt(previous, head)):
        return f"the flows step {head[:12]} carries no code review approval of exactly that commit"
    refused = [reason for reason in (flow_paths.refusal(name, step.get("flow_paths") or []) for name in sorted(files))
               if reason]
    refused += flow_paths.collisions(sorted(files), _tracked(store, previous))
    if refused:
        return "; ".join(refused)
    changes = git(store, "diff", "--name-status", "--no-renames", "-z", previous, head).split("\0")
    found = sorted((status, name) for status, name in zip(changes[0::2], changes[1::2]) if name)
    if found != [("A", name) for name in sorted(files)]:
        return f"{head[:12]} changes more than adding its flow files"
    if any(git(store, "rev-parse", f"{head}:{name}").strip() != blob for name, blob in files.items()):
        return f"{head[:12]} does not hold the recorded bytes of its flow files"
    return ""


def _tracked(store: Path, revision: str) -> list[str]:
    return git(store, "ls-tree", "-r", "--name-only", "-z", revision).split("\0")[:-1]


def _seal(repo: Path, state: dict, *revisions: str) -> Path:
    """Seal each of `revisions` the store does not hold yet from the task's repository; returns the store."""
    store = sealed.path(repo)
    for revision in dict.fromkeys(revisions):
        if not (store.is_dir() and _held(store, revision)):
            store = sealed.seal(_source(state), revision)
    gitutil.trust_store(store)
    return store


def _source(state: dict) -> Path:
    """Where approved commits are sealed from: the object database of the task's own repository, which every
    workspace shares, reached through workspace 0 (the seal verifies that its `.git` names its own entry of the
    trusted repository, `sealed.seal`), or the repository itself before workspace 0 exists."""
    zero = (state.get("workspaces", {}).get("0") or {}).get("path")
    return Path(zero) if zero else task_repository.path(state)


def _held(store: Path, revision: str) -> bool:
    gitutil.trust_store(store)
    return gitutil.run(store, "rev-parse", "--verify", "--quiet", f"{sealed.REF_PREFIX}{revision}^{{commit}}") \
        .stdout.strip() == revision


def _keep(store: Path, head: str) -> None:
    """A ref of its own keeps a merge the engine made reachable in the store, like a sealed revision."""
    git(store, "update-ref", f"{sealed.REF_PREFIX}{head}", head)


def _is_ancestor(store: Path, ancestor: str, descendant: str) -> bool:
    return gitutil.run(store, "merge-base", "--is-ancestor", ancestor, descendant).returncode == 0


@contextlib.contextmanager
def _integration_worktree(ctx, store: Path, onto: str) -> Iterator[Path]:
    """A detached worktree of the store at `onto`, at a fresh path under the engine home; removed afterwards."""
    parent = engine_home.ensure_project_dir(ctx.repo) / FOLDER
    parent.mkdir(mode=FOLDER_MODE, exist_ok=True)
    path = parent / f"{ctx.store.task_id}-{uuid.uuid4().hex[:12]}"
    git(store, "worktree", "add", "--detach", str(path), onto)
    try:
        yield path
    finally:
        try:
            git(store, "worktree", "remove", "--force", str(path))
        except GitError:
            if os.path.lexists(path):
                shutil.rmtree(path, ignore_errors=True)
            git(store, "worktree", "prune")
