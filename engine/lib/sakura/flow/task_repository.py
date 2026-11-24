"""The task's own Git repository: every workspace of a task is a worktree of it, never of the person's repository.

The person's repository keeps their local Git config (`config`, `config.worktree`), and a secret there (a password for
`sendemail`, a token in a remote URL) would reach a model's context once a role could read that file; no list of key
names tells every secret apart. So no role and no caged command is granted the person's Git folder. When a task starts,
the engine, outside every cage and through the single Git entry point (`gitutil`), makes the task's own bare
repository beside its workspaces, `<workspace root>/<task>.git` (mode 0700, no template), and every lane, the
integration workspace and the evaluator's base are worktrees of it. Its config is the engine's: what `git init --bare`
writes for the volume and the object format, and the person's commit identity (`user.name`, `user.email`, read from
the person's effective config). Nothing else: no remote, no credential, `url`, `http`, `sendemail` or `include` key.
Its digest is recorded when it is made and every engine Git call on it is refused once it differs; the cage denies
writes to it as to the hooks, alternates, pointer files, replacement refs, grafts and `shallow` of this repository.

Objects are copied, never borrowed: the base commit and its history come in through Git's file transport (a fetch, as
`clone --no-local --no-hardlinks` does, every object hashed on the way in) as `refs/heads/<base branch>`, and nothing
else of the person's repository (no other branch, tag or stash). Measured on 2026-11-13 (Git 2.51, APFS): the copy takes
0.2 s and 2.5 MB for a 46 MB repository whose base history is small, and 3.5 s and 355 MB for a 360 MB one; one
alternate to the person's objects folder takes 0.05 s and 0.2 MB for either. The copy is the choice: an alternate would
grant every role the person's whole object database (every branch, whatever was never pushed), let the person's own
`gc` prune objects the task still needs, and need an exception in the seal's refusal of alternates (review 7); the copy
costs seconds and disk once per task, given back at cleanup.

A record at the repository's top (`engine_home.TASK_REPOSITORY_RECORD`) names the person's common Git folder, so a
workspace of the task belongs to that project: its task state, config, trust records and object store. A task
stacked on another one (`start --after`) gets its base from the engine's store, where the other task's head was sealed.

The reporter does not work in a workspace of this repository: every earlier role wrote it, and a detached survivor
may still write there (review 11, finding 2). Nor does a code reviewer: the objects it would read there are the
ones the role it reviews (and that role's survivor) can rewrite. Each gets a repository of its own
(`make_role_repository`), made the same way at a fresh path beside the workspaces, after the role-writable step it
follows: the reporter's holds one empty commit, so its worktree is an empty folder where the engine copies the sealed
inputs; a reviewer's holds the commits it reviews, fetched from the engine's store (`flow/sealed.py`, every object
hashed on the way in twice), with its worktree at the head under review. The run's cage is given that repository's
Git folder and nothing of the task's (`runner.RunRequest.repository`). The engine removes both once the run ended.
"""

from __future__ import annotations

import os
import shutil
import uuid
from pathlib import Path

from sakura import engine_home, git_program, install, repository
from sakura.flow import git_config, gitutil, pinned_roots, sealed
from sakura.flow.store import TASK_REPOSITORY_KEY

STATE_KEY = TASK_REPOSITORY_KEY
# The repositories of a role's own (the reporter's, a reviewer's) and their worktrees while they exist, by name.
ROLE_REPOSITORIES_KEY = "role_repositories"
# Every role repository and worktree ever made for the task, kept after they are removed: a detached survivor of a run
# can make its folders again, so each stays denied to every later isolated run (`runner.AgentRunner._isolation`).
ROLE_PATHS_KEY = "role_repository_paths"
# Role repositories kept after their review ended because a message for the reviewer waits for its continuation
# (`flow/agent_messages.needs_workspace`); the worker that delivers it removes them.
RETAINED_KEY = "role_repositories_retained"
ROLE_BRANCH = "role"
# What the engine names for a run's uncaged helpers (the evaluator's row tool, the agent's hooks): the person's main
# worktree and the task's own repository. A helper opens the task from these, never from its workspace's `.git`.
PERSON_REPOSITORY_ENV = "SAKURA_FLOW_REPOSITORY"
TASK_REPOSITORY_ENV = "SAKURA_FLOW_TASK_REPOSITORY"
REPOSITORY_SUFFIX = ".git"
FOLDER_MODE = 0o700
FETCH_CONFIG = ("-c", "protocol.version=2", "-c", "gc.auto=0", "-c", "maintenance.auto=false")
FETCH_OPTIONS = ("--no-tags", "--no-write-fetch-head", "--no-recurse-submodules", "--no-auto-maintenance", "--quiet")


class TaskRepositoryError(RuntimeError):
    """The task's repository cannot be made or found; the message says why and what to do."""


def workspace_root(values: dict, repo: Path) -> Path:
    """The folder of a task's workspaces and repository: `workspace.root` with `{engine}` and `{repo}` filled in."""
    root = engine_home.with_engine_root(values["workspace"]["root"]).replace("{repo}", repo.name)
    return Path(os.path.expanduser(root))


def workspaces_folder(values: dict) -> Path:
    """The folder that holds the workspaces of every repository under this `workspace.root`: the folder that holds
    the first path component with `{repo}` in it (`ws-{repo}/x` in `<folder>/ws-{repo}/x` gives `<folder>`), and the
    root itself when the setting names no `{repo}`. The engine placeholder and `~` are expanded first.

    Raises TaskRepositoryError when that is not an absolute folder: a folder that cannot be named is never guessed.
    """
    template = values["workspace"]["root"]
    before, found, _ = template.partition("{repo}")
    expanded = os.path.expanduser(engine_home.with_engine_root(before if found else template))
    folder = os.path.dirname(expanded) if found else expanded
    if not os.path.isabs(folder):
        raise TaskRepositoryError(f"workspace.root {template!r} does not name an absolute folder for the workspaces; "
                                  f"start it with a folder, `~` or {{engine}}")
    return Path(folder)


def path_for(values: dict, repo: Path, task_id: str) -> Path:
    """Where the repository of task `task_id` is made: `<workspace root>/<task>.git`."""
    return workspace_root(values, repo) / f"{task_id}{REPOSITORY_SUFFIX}"


def path(state: dict) -> Path:
    """The task's repository as its state records it; TaskRepositoryError for a task that has none."""
    recorded = state.get(STATE_KEY)
    if not isinstance(recorded, dict) or not isinstance(recorded.get("path"), str):
        raise TaskRepositoryError(f"task {state.get('task', '')} has no repository of its own yet")
    return Path(recorded["path"])


def common_dir(store) -> Path:
    """The Git folder every cage of the task's runs is given: its own repository's, never the person's."""
    return path(store.read_state())


def ensure(store, values: dict, repo: Path) -> Path:
    """The task's repository, made from the person's repository `repo` the first time, trusted and pinned here.

    A folder at its place that the state does not record was left by a crash while it was made and is made again.
    Raises TaskRepositoryError for a recorded repository that is gone, for a task whose workspaces are worktrees of
    the person's repository (started before tasks had their own) and for a base that cannot be copied.
    """
    state = store.read_state()
    if state.get(STATE_KEY):
        return _reopened(state)
    if state.get("workspaces"):
        raise TaskRepositoryError(f"task {store.task_id} has workspaces made in the repository itself, before tasks "
                                  "had their own repository; start it again with --new")
    folder = path_for(values, repo, store.task_id)
    _discard_unrecorded(folder)
    _make(folder, repo, state)
    record = {"path": str(folder), "config_sha256": git_config.digest(folder)}
    return _reopened(store.update_state(lambda current: current.update({STATE_KEY: record})))


def pin(state: dict) -> None:
    """Trust the recorded task repository and every role repository the state records in this process, and refuse
    Git calls on each once its config differs from the digest recorded when it was made; nothing for a task without
    one."""
    recorded = [state[STATE_KEY]] if state.get(STATE_KEY) else []
    recorded += list((state.get(ROLE_REPOSITORIES_KEY) or {}).values())
    for entry in recorded:
        gitutil.trust_repository(Path(entry["path"]))
        gitutil.pin_config(Path(entry["path"]), entry["config_sha256"])


def make_role_repository(store, values: dict, repo: Path, purpose: str, sealed_store: Path | None = None,
                         revisions: tuple[str, ...] = ()) -> tuple[Path, Path]:
    """Make a repository of a role's own and its worktree at fresh paths in the workspace root; returns (repository,
    worktree). Without `revisions` it holds one empty commit; with them, those commits fetched from the engine's
    store `sealed_store`, its worktree at the last one. The engine's hook files are installed in the worktree, as in
    every workspace. See the module text."""
    token = f"{store.task_id}-{purpose}-{uuid.uuid4().hex[:12]}"
    root = workspace_root(values, repo)
    folder, worktree = root / f"{token}{REPOSITORY_SUFFIX}", root / token
    _init(folder, repo)
    head = _sealed_into(folder, sealed_store, revisions) if revisions else _empty_commit(folder)
    gitutil.git(folder, "update-ref", f"refs/heads/{ROLE_BRANCH}", head)
    record = {"path": str(folder), "config_sha256": git_config.digest(folder), "workspace": str(worktree)}
    made = [os.path.realpath(folder), os.path.realpath(worktree)]

    def change(current: dict) -> None:
        current.setdefault(ROLE_REPOSITORIES_KEY, {})[token] = record
        current[ROLE_PATHS_KEY] = [*current.get(ROLE_PATHS_KEY, []), *made]

    pin(store.update_state(change))
    gitutil.git(folder, "worktree", "add", "--quiet", str(worktree), ROLE_BRANCH)
    install.install_files(worktree)
    return folder, worktree


def remove_role_repository(store, folder: Path) -> None:
    """Delete the role repository `folder` and its worktree and forget them; nothing when the state does not record
    it."""
    found = [(name, entry) for name, entry in (store.read_state().get(ROLE_REPOSITORIES_KEY) or {}).items()
             if os.path.realpath(entry["path"]) == os.path.realpath(folder)]
    _forget_retained(store, folder)
    for name, entry in found:
        for made in (Path(entry["workspace"]), Path(entry["path"])):
            if made.is_symlink():
                made.unlink()
            elif made.exists():
                shutil.rmtree(made)
            pinned_roots.release(made)
        store.update_state(lambda current: current.get(ROLE_REPOSITORIES_KEY, {}).pop(name, None))


def _forget_retained(store, folder: Path) -> None:
    def change(current: dict) -> None:
        if current.get(RETAINED_KEY):
            current[RETAINED_KEY] = [kept for kept in current[RETAINED_KEY]
                                     if os.path.realpath(kept) != os.path.realpath(folder)]

    store.update_state(change)


def retain(store, folder: Path) -> None:
    """Keep the role repository `folder` after its review: a message for the reviewer waits for it."""
    store.update_state(lambda current: current.setdefault(RETAINED_KEY, []).append(str(folder)))


def release_retained(store, workspace: str) -> None:
    """Remove the retained role repository whose worktree is `workspace`, if there is one."""
    state = store.read_state()
    kept = {os.path.realpath(folder) for folder in state.get(RETAINED_KEY, [])}
    for entry in (state.get(ROLE_REPOSITORIES_KEY) or {}).values():
        if os.path.realpath(entry["path"]) in kept and os.path.realpath(entry["workspace"]) == os.path.realpath(workspace):
            remove_role_repository(store, Path(entry["path"]))


def remove(store) -> None:
    """Delete the task's repository (after its workspaces) and forget it; nothing for a task without one."""
    state = store.read_state()
    if not state.get(STATE_KEY):
        return
    folder = path(state)
    if folder.is_dir() and not folder.is_symlink():
        shutil.rmtree(folder)
    store.update_state(lambda current: current.pop(STATE_KEY, None))


def helper_environment(store, repository: Path | None = None) -> dict[str, str]:
    """The repositories the engine names to a run's uncaged helpers: the person's main worktree and the repository
    the run works in: `repository` (a role's own) or, once it is made, the task's own as the state records it."""
    state = store.read_state()
    env = {PERSON_REPOSITORY_ENV: str(store.person_repository)}
    if repository is not None:
        env[TASK_REPOSITORY_ENV] = str(repository)
    elif state.get(STATE_KEY):
        env[TASK_REPOSITORY_ENV] = str(path(state))
    return env


def recorded_repositories(state: dict) -> dict[str, list[str]]:
    """Each repository the state records a role may work in, by real path, with the workspaces made of it."""
    found: dict[str, list[str]] = {}
    if state.get(STATE_KEY):
        found[os.path.realpath(path(state))] = [
            os.path.realpath(entry["path"]) for entry in state.get("workspaces", {}).values()
            if isinstance(entry, dict) and isinstance(entry.get("path"), str)]
    for entry in (state.get(ROLE_REPOSITORIES_KEY) or {}).values():
        found[os.path.realpath(entry["path"])] = [os.path.realpath(entry["workspace"])]
    return found


def open_for_helper(env, task_id: str, location: Path):
    """The task's store, opened from the person's repository the engine named in `env`, and the workspace `location`
    lies in, verified against the state (`verified_workspace`) before any Git runs there.

    An uncaged helper runs in a folder a role writes, so nothing of it (its `.git` marker, a project record, a nested
    repository) may say which repository or task it belongs to (review 10, finding 1). Raises TaskRepositoryError
    when `env` names no repositories or the task or the workspace does not check out.
    """
    from sakura.flow.store import TaskStore

    person, claimed = env.get(PERSON_REPOSITORY_ENV, ""), env.get(TASK_REPOSITORY_ENV, "")
    if not (os.path.isabs(person) and os.path.isabs(claimed)):
        raise TaskRepositoryError(f"the engine named no repositories for this run ({PERSON_REPOSITORY_ENV}, "
                                  f"{TASK_REPOSITORY_ENV}); a run's helper opens its task only from what the engine "
                                  "names, never from the workspace")
    store = TaskStore(Path(person), task_id)
    if not store.exists():
        raise TaskRepositoryError(f"the repository the engine named has no task {task_id}")
    return store, verified_workspace(store.read_state(), Path(claimed), location)


def verified_workspace(state: dict, claimed: Path, location: Path) -> Path:
    """The workspace `location` lies in, accepted only as the engine made it: `claimed` is a repository the state
    records (the task's own, or a role's own while it exists), the workspace is one the state records of it, its
    `.git` (named exactly so) names its own entry of that repository with no link on the way, and the entry names it
    back. The recorded repositories are trusted and their config pinned first, so no Git call of this process can run
    in another one. Raises TaskRepositoryError."""
    path(state)
    repositories = recorded_repositories(state)
    recorded = os.path.realpath(claimed)
    if recorded not in repositories:
        raise TaskRepositoryError(f"{claimed} is not the repository recorded for task {state.get('task', '')}")
    pin(state)
    worktree = git_program.worktree_of(location)
    if worktree is None or os.path.realpath(worktree) not in repositories[recorded]:
        raise TaskRepositoryError(f"{location} is not in a workspace the engine made for task {state.get('task', '')}")
    try:
        common = gitutil.verified_common_dir(worktree)
    except gitutil.GitError as error:
        raise TaskRepositoryError(f"the Git folder of {worktree} is not the task repository's own: {error}") from None
    if os.path.realpath(common) != os.path.realpath(recorded):
        raise TaskRepositoryError(f"the Git folder of {worktree} is not the task repository's own")
    return worktree


def operator_repository(location: Path) -> Path:
    """The person's repository an operator command run in `location` works on (review 10, finding 4).

    In the person's own repository: that worktree, as before. In a workspace of a task's own repository: the main
    worktree of the person's repository, once the workspace checks out against the task the person's repository
    records for that task repository (`verified_workspace`): its configuration, workspace roots, transport, `gh`,
    profiles and reports are the person's, never the sanitized task repository's. Nothing found there is trusted
    before that: the record only says which project to look in. Raises TaskRepositoryError for a workspace no task
    records, and RepositoryError outside every repository.
    """
    worktree = git_program.worktree_of(location)
    common = git_program.common_dir_of(worktree) if worktree is not None else None
    if common is None or not os.path.lexists(common / engine_home.TASK_REPOSITORY_RECORD):
        return repository.root(location)
    person = engine_home.main_worktree(engine_home.project_common_dir(common))
    verified_workspace(_state_recording(person, common), common, worktree)
    return person


def _state_recording(person: Path, common: Path) -> dict:
    """The state of the task of `person`'s repository whose own repository is `common`."""
    from sakura.flow import store as task_store

    root = task_store.flow_root(person)
    for folder in sorted(root.iterdir()) if root.is_dir() else []:
        try:
            state = task_store.TaskStore(person, folder.name).read_state()
        except task_store.StateError:
            continue
        recorded = state.get(STATE_KEY)
        if isinstance(recorded, dict) and os.path.realpath(str(recorded.get("path", ""))) == os.path.realpath(common):
            return state
    raise TaskRepositoryError(f"{common} is not the repository of any task of {person}; run the command in the "
                              "repository itself")


def person_common_dir(location: Path) -> Path:
    """The common Git folder of the person's repository `location` belongs to, also from a workspace of a task's
    own repository (its record)."""
    return engine_home.project_common_dir(repository.common_dir(location))


def _reopened(state: dict) -> Path:
    folder = path(state)
    if not folder.is_dir():
        raise TaskRepositoryError(f"the task's repository {folder} is gone, and its workspaces with it; start the "
                                  "task again with --new")
    pin(state)
    return folder


def _discard_unrecorded(folder: Path) -> None:
    """Remove what a crash left at the repository's place before the state recorded it (no workspace was made from
    it); a link is unlinked, never followed."""
    if folder.is_symlink():
        folder.unlink()
    elif folder.exists():
        shutil.rmtree(folder)


def _make(folder: Path, repo: Path, state: dict) -> None:
    """Make the bare repository: no template, the person's object format and identity, the project record and the
    base history; nothing else is written into its config."""
    _init(folder, repo)
    _copy_base(folder, repo, state["base_commit"], f"refs/heads/{state['base_branch']}")


def _empty_commit(folder: Path) -> str:
    tree = gitutil.git(folder, "mktree").strip()
    return gitutil.git(folder, "commit-tree", tree, "-m", "a role's own workspace").strip()


def _sealed_into(folder: Path, sealed_store: Path | None, revisions: tuple[str, ...]) -> str:
    """Fetch `revisions` from the engine's store into `folder` (each under a ref of its own); returns the last one."""
    if sealed_store is None:
        raise TaskRepositoryError("a role repository with commits needs the engine's store to fetch them from")
    refspecs = [f"+{revision}:refs/sakura/{index}" for index, revision in enumerate(revisions)]
    done = gitutil.run(folder, *FETCH_CONFIG, "fetch", *FETCH_OPTIONS, sealed.file_url(sealed_store), *refspecs,
                       local_transfer=True)
    if done.returncode:
        raise TaskRepositoryError(f"the engine's store did not give {', '.join(r[:12] for r in revisions)}: "
                                  f"{(done.stderr or done.stdout).strip()}")
    return revisions[-1]


def _init(folder: Path, repo: Path) -> None:
    """A new bare repository at `folder` (refused when something is there): no template, the person's object format
    and identity and the project record; nothing else is written into its config."""
    folder.parent.mkdir(parents=True, exist_ok=True)
    folder.mkdir(mode=FOLDER_MODE)
    gitutil.init_bare(folder, gitutil.git(repo, "rev-parse", "--show-object-format").strip())
    gitutil.trust_repository(folder)
    for key, value in gitutil.person_identity(repo).items():
        gitutil.git(folder, "config", key, value)
    # The person's common Git folder exactly as the engine's own task store resolves it, so a store opened on a
    # workspace keys its state, digests and hook marks the same way (`engine_home.project_common_dir`).
    (folder / engine_home.TASK_REPOSITORY_RECORD).write_text(f"{repository.common_dir(repo)}\n")


def _copy_base(folder: Path, repo: Path, base: str, ref: str) -> None:
    """Fetch `base` and its history into `ref` of the task repository through the file transport."""
    done = gitutil.run(folder, *FETCH_CONFIG, "fetch", *FETCH_OPTIONS, sealed.file_url(_base_source(repo, base)),
                       f"+{base}:{ref}", local_transfer=True)
    if done.returncode:
        raise TaskRepositoryError(f"the base commit {base[:12]} could not be copied into the task's repository: "
                                  f"{(done.stderr or done.stdout).strip()}")
    if gitutil.git(folder, "rev-parse", "--verify", f"{ref}^{{commit}}").strip() != base:
        raise TaskRepositoryError(f"the task's repository does not hold the base commit {base[:12]} after the copy")


def _base_source(repo: Path, base: str) -> Path:
    """The Git folder that holds `base`: the person's repository, else the engine's store (a stacked task's base,
    the head of the task it follows, sealed when it started)."""
    if gitutil.run(repo, "cat-file", "-e", f"{base}^{{commit}}").returncode == 0:
        return git_program.common_dir_of(repo)
    store = sealed.path(repo)
    if store.is_dir():
        gitutil.trust_store(store)
        if gitutil.run(store, "cat-file", "-e", f"{base}^{{commit}}").returncode == 0:
            return store
    raise TaskRepositoryError(f"the base commit {base[:12]} is neither in the repository nor in the engine's store")
