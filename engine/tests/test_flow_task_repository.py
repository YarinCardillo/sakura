"""The task's own repository: workspaces are worktrees of an engine-made repository, never of the person's."""

import os
import subprocess
from pathlib import Path

import pytest

from conftest import git, trust_root_child_environment
from flow_support import engine_head, make_context
from sakura import engine_home, repository
from sakura.flow import engine, git_config, gitutil, store, task_repository, workspaces

# Fake secrets planted in the person's local Git config: none of them may appear anywhere a role can read.
SMTP_PASSWORD = "correct-horse-battery-staple-91"
URL_TOKEN = "ghp_faketoken000000000000000000000000"
HEADER_TOKEN = "Bearer fake-header-token-77"
SECRETS = (SMTP_PASSWORD, URL_TOKEN, HEADER_TOKEN)
# What the engine writes into a task repository's config: what `git init --bare` probes for the volume, the object
# format and the person's commit identity.
ALLOWED_KEYS = {"core.repositoryformatversion", "core.filemode", "core.bare", "core.ignorecase",
                "core.precomposeunicode", "core.logallrefupdates", "extensions.objectformat", "user.name", "user.email"}


def _plant_secrets(repo: Path) -> None:
    git(repo, "config", "sendemail.smtpPass", SMTP_PASSWORD)
    git(repo, "remote", "add", "origin", f"https://x-access-token:{URL_TOKEN}@example.invalid/owner/repo.git")
    git(repo, "config", "http.https://example.invalid/.extraheader", f"Authorization: {HEADER_TOKEN}")


def _context(repo: Path, tmp_path: Path, task: str = "demo"):
    base = git(repo, "rev-parse", "HEAD")
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []}}
    state = {"base_commit": base, "base_branch": "main", "branch": f"feat/flow-{task}", "workspaces": {}}
    return make_context(repo, overrides=overrides, task=task, state=state), base


def _task_files(path: Path) -> bytes:
    return b"".join(item.read_bytes() for item in path.rglob("*") if item.is_file() and not item.is_symlink())


def _local_keys(common: Path) -> set[str]:
    done = subprocess.run(["git", "config", "--file", str(common / "config"), "--name-only", "--list"],
                          capture_output=True, text=True, check=True)
    return set(done.stdout.split())


def test_TaskRepository_MadeAtFirstWorkspace_HoldsOnlyAnEngineWrittenConfig(repo, tmp_path):
    _plant_secrets(repo)
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    common = repository.common_dir(workspace.path)
    assert common == tmp_path / "trees" / "repo" / "demo.git"
    assert common == task_repository.path(ctx.store.read_state())
    assert _local_keys(common) <= ALLOWED_KEYS
    assert git(workspace.path, "config", "user.email") == "test@example.invalid"
    assert git(workspace.path, "remote") == ""
    contents = _task_files(common) + _task_files(workspace.path)
    assert not any(secret.encode() in contents for secret in SECRETS)
    assert stat_mode(common) == 0o700


def stat_mode(path: Path) -> int:
    return os.stat(path).st_mode & 0o777


def test_TaskRepository_PersonsRepository_GetsNoWorktreeEntryOrBranch(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert not (repo / ".git" / "worktrees").exists()
    assert git(repo, "branch", "--list", "feat/flow-demo") == ""
    assert git(workspace.path, "rev-parse", "--abbrev-ref", "HEAD") == "feat/flow-demo"
    assert git(workspace.path, "rev-parse", "HEAD") == base


def test_TaskRepository_Objects_AreCopiedNeverBorrowedOrHardlinked(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    common = repository.common_dir(workspaces.create(ctx, "0", "feat/flow-demo", base).path)
    assert not (common / "objects" / "info" / "alternates").exists()
    person = {(info.st_dev, info.st_ino) for info in map(os.stat, (repo / ".git" / "objects").rglob("*"))}
    copied = [item for item in (common / "objects").rglob("*") if item.is_file()]
    assert copied and not any((os.stat(item).st_dev, os.stat(item).st_ino) in person for item in copied)


def test_TaskRepository_HoldsTheBaseHistoryOnly_UnderTheBaseBranch(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    git(repo, "checkout", "-qb", "private-work")
    (repo / "private.txt").write_text("unpushed\n")
    git(repo, "add", "private.txt")
    git(repo, "commit", "-qm", "private")
    private = git(repo, "rev-parse", "HEAD")
    common = repository.common_dir(workspaces.create(ctx, "0", "feat/flow-demo", base).path)
    assert git(common, "rev-parse", "refs/heads/main") == base
    missing = subprocess.run(["git", "-C", str(common), "cat-file", "-e", private], capture_output=True)
    assert missing.returncode != 0


def test_TaskRepository_LanesAndIntegration_ShareTheTaskRepository(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    zero = workspaces.create(ctx, "0", "feat/flow-demo", base)
    lane = workspaces.create(ctx, "1", "feat/flow-demo-t1", base)
    (lane.path / "lane.txt").write_text("lane\n")
    git(lane.path, "add", "lane.txt")
    git(lane.path, "commit", "-qm", "lane work")
    head = git(lane.path, "rev-parse", "HEAD")
    gitutil.git(zero.path, "merge", "--no-ff", "-m", "merge lane", head)
    assert (zero.path / "lane.txt").read_text() == "lane\n"
    assert repository.common_dir(lane.path) == repository.common_dir(zero.path)
    assert "lane work" in git(zero.path, "log", "--format=%s")


def test_TaskRepository_Resume_FindsTheRecordedRepository(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    config_identity = os.stat(repository.common_dir(workspace.path) / "config").st_ino
    gitutil._trusted_repositories[:] = []
    resumed = engine.open_context(repo, store.TaskStore(repo, ctx.store.task_id))
    again = workspaces.create(resumed, "0", "feat/flow-demo", base)
    assert again.path == workspace.path
    assert os.stat(repository.common_dir(again.path) / "config").st_ino == config_identity
    assert gitutil.git(again.path, "rev-parse", "HEAD").strip() == base


def test_TaskRepository_LeftUnrecordedByACrash_IsMadeAgain(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    partial = task_repository.path_for(ctx.config.values, repo, ctx.store.task_id)
    partial.mkdir(parents=True)
    (partial / "leftover").write_text("half made\n")
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert repository.common_dir(workspace.path) == partial
    assert not (partial / "leftover").exists()


def test_TaskRepository_RecordedButGone_RefusesWithTheWayOut(repo, tmp_path):
    import shutil

    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    shutil.rmtree(repository.common_dir(workspace.path))
    shutil.rmtree(workspace.path)
    with pytest.raises(workspaces.WorkspaceError, match="--new"):
        workspaces.create(ctx, "0", "feat/flow-demo", base)


def test_TaskRepository_TaskWithWorkspacesOfThePersonsRepository_IsRefused(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    old = tmp_path / "old-lane"
    git(repo, "worktree", "add", "-q", "-b", "feat/flow-demo", str(old), base)
    ctx.store.update_state(lambda state: state["workspaces"].update(
        {"0": {"path": str(old), "branch": "feat/flow-demo", "ports": [42000, 42009]}}))
    with pytest.raises(workspaces.WorkspaceError, match="--new"):
        workspaces.create(ctx, "1", "feat/flow-demo-t1", base)


def test_TaskRepository_ConfigChangedAfterCreation_StopsEngineGit(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    engine.open_context(repo, ctx.store)
    common = repository.common_dir(workspace.path)
    subprocess.run(["git", "config", "--file", str(common / "config"), "diff.external", "/bin/true"], check=True)
    with pytest.raises(gitutil.GitError, match="local Git config changed"):
        gitutil.git(workspace.path, "status")


def test_TaskRepository_Workspace_BelongsToThePersonsProject(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert engine_home.project_id(workspace.path) == engine_home.project_id(repo)
    from_hook = store.TaskStore(repository.root(workspace.path), ctx.store.task_id)
    assert from_hook.root == ctx.store.root
    assert from_hook.hook_mark("build-1") == ctx.store.hook_mark("build-1")
    # A hook reads the state the engine wrote, and a store opened on the repository reads what a store opened on a
    # workspace wrote: one digest list, one task.
    assert from_hook.read_state() == ctx.store.read_state()
    from_hook.update_state(lambda state: state.update(reason="written from a workspace"))
    assert store.TaskStore(repo, ctx.store.task_id).read_state()["reason"] == "written from a workspace"


def test_TaskRepository_RemoveAfterItsWorkspaces_LeavesNothingAndGitStillWorks(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base)
    common = repository.common_dir(workspace.path)
    workspaces.remove(ctx, "0")
    task_repository.remove(ctx.store)
    assert not common.exists() and not workspace.path.exists()
    assert task_repository.STATE_KEY not in ctx.store.read_state()
    assert gitutil.git(repo, "rev-parse", "HEAD").strip() == base


def test_TaskRepository_ConfigDigest_IsRecordedAtCreation(repo, tmp_path):
    ctx, base = _context(repo, tmp_path)
    common = repository.common_dir(workspaces.create(ctx, "0", "feat/flow-demo", base).path)
    recorded = ctx.store.read_state()[task_repository.STATE_KEY]
    assert recorded["config_sha256"] == git_config.digest(common)


VALID_SPEC = "# Spec\n\n## Goal\nx\n\n## Journeys\n1. open the page\n"


def test_Intake_RunsThePlannerInWorkspaceZeroOfTheTaskRepository_UnpreparedUntilTheContract(repo, tmp_path):
    from flow_support import FakeRunner
    from sakura.flow.phases import intake

    (repo / "specs").mkdir()
    (repo / "specs" / "demo.md").write_text("build a thing\n")
    base = git(repo, "rev-parse", "HEAD")
    planner = FakeRunner()
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": ["echo ready > setup.out"]}}
    ctx = make_context(repo, runner=planner, overrides=overrides,
                       state={"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}})
    planner.behaviour = lambda _request: ctx.store.spec.write_text(VALID_SPEC)
    assert intake.run(ctx).gate == "spec"
    cwd = planner.requests[0].cwd
    assert repository.common_dir(cwd) == task_repository.path(ctx.store.read_state())
    assert not (cwd / "setup.out").exists() and not (cwd / ".sakura").exists()
    prepared = workspaces.create(ctx, "0", "feat/flow-demo", base)
    assert prepared.path == cwd
    assert (cwd / "setup.out").read_text() == "ready\n" and (cwd / ".sakura" / "git-hooks" / "pre-commit").is_symlink()


def test_Intake_APlannerThatChangesItsWorkspace_IsBlocked(repo, tmp_path):
    from flow_support import FakeRunner
    from sakura.flow.phases import intake

    (repo / "specs").mkdir()
    (repo / "specs" / "demo.md").write_text("build a thing\n")
    planner = FakeRunner()
    ctx, _base = _context(repo, tmp_path)
    object.__setattr__(ctx, "runner", planner)

    def behaviour(request):
        ctx.store.spec.write_text(VALID_SPEC)
        (request.cwd / "planner-was-here.txt").write_text("x\n")

    planner.behaviour = behaviour
    ctx.store.update_state(lambda state: state.update(ref="specs/demo.md"))
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "planner-was-here.txt" in outcome.reason


def test_StackedStart_OnATaskWithItsOwnRepository_GetsItsBaseThroughTheEnginesStore(repo, tmp_path, monkeypatch):
    from sakura.flow import cli, exits

    parent, base = _context(repo, tmp_path, task="parent")
    zero = workspaces.create(parent, "0", "feat/flow-parent", base)
    (zero.path / "parent.txt").write_text("parent\n")
    git(zero.path, "add", "parent.txt")
    git(zero.path, "commit", "-qm", "parent work")
    head = engine_head(parent)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *_args, **_kwargs: exits.OK)
    assert cli.main(["start", "-m", "Build on the parent task", "--after", "parent"]) == exits.OK
    child = store.TaskStore(repo, "build-on-the-parent-task")
    state = child.read_state()
    assert (state["base_branch"], state["base_commit"]) == ("feat/flow-parent", head)
    folder = task_repository.path(state)
    assert git(folder, "rev-parse", "refs/heads/feat/flow-parent") == head
    # The person's repository never held the parent's commit: it came from the store.
    assert subprocess.run(["git", "-C", str(repo), "cat-file", "-e", head], capture_output=True).returncode != 0


def test_StackedStart_ACommitASurvivorAddedToTheParentsBranch_IsNotTheChildsBase(repo, tmp_path, monkeypatch):
    # Review 11, finding 1: the child is based on the parent's engine head, never on a branch a role can move.
    from sakura.flow import cli, exits

    parent, base = _context(repo, tmp_path, task="parent")
    zero = workspaces.create(parent, "0", "feat/flow-parent", base)
    (zero.path / "parent.txt").write_text("parent\n")
    git(zero.path, "add", "parent.txt")
    git(zero.path, "commit", "-qm", "parent work")
    head = engine_head(parent)
    (zero.path / "survivor.txt").write_text("never reviewed\n")
    git(zero.path, "add", "survivor.txt")
    git(zero.path, "commit", "-qm", "a survivor's commit")
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *_args, **_kwargs: exits.OK)
    assert cli.main(["start", "-m", "Build on the parent task", "--after", "parent"]) == exits.OK
    state = store.TaskStore(repo, "build-on-the-parent-task").read_state()
    assert state["base_commit"] == head
    folder = task_repository.path(state)
    assert git(folder, "ls-tree", "--name-only", "refs/heads/feat/flow-parent").splitlines().count("survivor.txt") == 0


def test_Cage_OfARoleAndOfACheck_GrantsTheTaskRepositoryAndNeverThePersonsGitFolder(repo, tmp_path, monkeypatch):
    import json

    from flow_support import MonitorLauncher
    from sakura.flow import cage, checks, process, runner

    _plant_secrets(repo)
    base = git(repo, "rev-parse", "HEAD")
    overrides = {"workspace": {"root": str(tmp_path / "trees" / "{repo}")}, "dial": "save",
                 "roles": {"builder": "claude:haiku"}}
    ctx = make_context(repo, overrides=overrides,
                       state={"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}})
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base).path
    person, task_git = str(repo / ".git"), str(task_repository.path(ctx.store.read_state()))
    launched = {}
    monkeypatch.setattr(runner.budget_guard, "require", lambda *_args, **_kwargs: None)
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(
        lambda argv, **_kwargs: launched.update(argv=argv) or process.ProcessOutcome(0, False, False, False, 1)))
    agent.run(runner.RunRequest("builder", "build", "x", {}, workspace, {}))
    folder = ctx.store.root / cage.CAGE_DIR / "build-1"
    sandbox = json.loads((folder / "settings.json").read_text())["sandbox"]["filesystem"]
    assert task_git in sandbox["allowWrite"] and task_git in sandbox["allowRead"]
    assert launched["argv"][launched["argv"].index("--add-dir") + 1] == task_git
    check = checks.checks_cage(ctx.store, workspace, "probe", cage.Policy("default"))
    for text in (json.dumps(sandbox), (folder / "sandbox.sb").read_text(), check.seatbelt.read_text(),
                 " ".join(launched["argv"])):
        assert person not in text
    assert task_git in check.seatbelt.read_text()


def test_Seatbelt_ALaneOfTheTaskRepository_ReadsNoSecretOfThePersonsConfig_AndItsGitWorks(repo, tmp_path):
    from sakura.flow import cage, checks

    _plant_secrets(repo)
    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base).path
    check = checks.checks_cage(ctx.store, workspace, "probe", cage.Policy("default"))
    env = {**{key: value for key, value in os.environ.items() if not key.startswith("SAKURA_FLOW_")}, **check.env}
    script = (f"cat {repo}/.git/config; git config --get sendemail.smtpPass; git config --list --show-origin; "
              f"git -C {repo} config --get sendemail.smtpPass; git remote -v")
    probe = subprocess.run(check.wrap(["sh", "-c", script]), cwd=workspace, env=env, capture_output=True, text=True,
                           timeout=60, stdin=subprocess.DEVNULL)
    assert not any(secret in probe.stdout + probe.stderr for secret in SECRETS)
    assert "Operation not permitted" in probe.stderr
    work = ("echo lane > lane.txt && git add lane.txt && git -c core.hooksPath=/dev/null commit -qm lane && "
            "git status --short && git log --oneline -1 && git diff HEAD~1 --stat")
    done = subprocess.run(check.wrap(["sh", "-c", work]), cwd=workspace, env=env, capture_output=True, text=True,
                          timeout=60, stdin=subprocess.DEVNULL)
    assert done.returncode == 0, done.stderr
    assert "lane" in done.stdout and "lane.txt" in done.stdout


# -- review 10, finding 1: an uncaged helper never takes its repository from what a role writes -----------------------

def _row_task(repo, tmp_path):
    """A task with an evaluator row, its own repository and workspace 0, as the evaluator's row tool finds it."""
    from sakura.flow import contract

    ctx, base = _context(repo, tmp_path)
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base, prepared=False).path
    row = contract.Row("C1", "local", "inspect", "cli", None, "true", False, False, contract.EVALUATOR)
    contract.save(ctx.store, [row])
    import hashlib
    digest = hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest))
    return ctx, workspace


def _forge(workspace: Path, task_git: Path, person_common: Path, canary: Path) -> Path:
    """What an evaluator could plant before the cage denied it: a bare repository nested in the task's Git folder,
    borrowing its objects, with a forged project record, an executable clean filter on every file and an entry the
    workspace's `.git` is redirected to (its `gitdir` points back, as a real entry's does)."""
    fake = task_git / "refs" / "fake.git"
    subprocess.run(["git", "init", "-q", "--bare", str(fake)], check=True)
    (fake / "objects" / "info" / "alternates").write_text(f"{task_git / 'objects'}\n")
    (fake / engine_home.TASK_REPOSITORY_RECORD).write_text(f"{person_common}\n")
    script = fake / "filter.sh"
    script.write_text(f"#!/bin/sh\ntouch {canary}\ncat\n")
    script.chmod(0o755)
    subprocess.run(["git", "config", "--file", str(fake / "config"), "filter.evil.clean", str(script)], check=True)
    (fake / "info").mkdir(exist_ok=True)
    (fake / "info" / "attributes").write_text("* filter=evil\n")
    real_entry = Path((workspace / ".git").read_text().removeprefix("gitdir:").strip())
    entry = fake / "worktrees" / "x"
    entry.mkdir(parents=True)
    (entry / "HEAD").write_text(git(workspace, "rev-parse", "HEAD") + "\n")
    (entry / "index").write_bytes((real_entry / "index").read_bytes())
    (entry / "commondir").write_text("../..\n")
    (entry / "gitdir").write_text(f"{workspace / '.git'}\n")
    (workspace / ".git").write_text(f"gitdir: {entry}\n")
    os.utime(workspace / "README", (1, 1))  # a stat change makes `git status` run the clean filter
    return fake


def _row_tool(ctx, workspace, **env_changes):
    """The evaluator's `run_row` tool as the engine serves it: `sakura flow run-row` in a process of its own, in the
    workspace, with the environment the engine's runner gives the tool (`changes` replace or drop entries).
    The policy is the one an evaluator granted `run_row` gets: the tool is refused to any other role or without the grant."""
    from sakura.flow import cage_server, runner

    request = runner.RunRequest("evaluator", "calibrate", "x", {}, workspace, {})
    env = runner.AgentRunner(ctx.store, ctx.config)._row_env(request, "calibrate-1")
    env.update(trust_root_child_environment())
    env.update({key: value for key, value in env_changes.items() if value is not None})
    for key in [key for key, value in env_changes.items() if value is None]:
        env.pop(key, None)
    policy = {"role": "evaluator", "tools": ["run_row"], "run_row": {"cwd": str(workspace), "env": env}}
    return cage_server.run_row(policy, "C1")


def test_RunRow_AForgedWorkspaceMarker_NeverRunsGitInTheForgedRepository(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    task_git = task_repository.path(ctx.store.read_state())
    canary = tmp_path / "filter-ran"
    _forge(workspace, task_git, repository.common_dir(repo), canary)
    answer = _row_tool(ctx, workspace)
    assert not canary.exists(), answer
    assert "not the task repository's own" in answer


def test_RunRow_WithoutTheEnginesNamedRepositories_IsRefusedBeforeAnyGit(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    answer = _row_tool(ctx, workspace, SAKURA_FLOW_REPOSITORY=None, SAKURA_FLOW_TASK_REPOSITORY=None)
    assert "SAKURA_FLOW_REPOSITORY" in answer and "exit 0" not in answer


def test_RunRow_ANamedTaskRepositoryThatIsNotTheRecordedOne_IsRefused(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    answer = _row_tool(ctx, workspace, SAKURA_FLOW_TASK_REPOSITORY=str(tmp_path / "other.git"))
    assert "recorded" in answer and "exit 0" not in answer


def test_RunRow_InAWorkspaceOfTheRecordedRepository_RunsTheRow(repo, tmp_path):
    from sakura.flow import completion

    ctx, workspace = _row_task(repo, tmp_path)
    ctx.store.update_state(lambda state: state.update(approvals={"checks": {"commands": ["true"]}}))
    with completion.base_build(ctx):  # a calibration row runs only on the engine's checkout of the base build
        assert "run-row C1: exit 0" in _row_tool(ctx, workspace)


def _hook(ctx, workspace, cwd=None, **env_changes):
    """`sakura hook claude PreToolUse` for a builder's `ls` in `cwd`, in a process of its own as Claude Code runs it,
    with the environment the engine's runner gives the role; returns the decision object or {}."""
    import json

    from sakura.flow import runner
    from sakura.flow.surface import KIT_BIN

    env = {**{key: value for key, value in os.environ.items() if not key.startswith("SAKURA_FLOW_")},
           "SAKURA_FLOW_TASK": ctx.store.task_id, "SAKURA_FLOW_RUN": "build-1", "SAKURA_FLOW_ROLE": "builder",
           "SAKURA_FLOW_PHASE": "build", **task_repository.helper_environment(ctx.store)}
    env.update(trust_root_child_environment())
    env.update({key: value for key, value in env_changes.items() if value is not None})
    for key in [key for key, value in env_changes.items() if value is None]:
        env.pop(key, None)
    payload = {"session_id": "s1", "cwd": str(cwd or workspace), "hook_event_name": "PreToolUse", "tool_name": "Bash",
               "tool_input": {"command": "ls"}}
    done = subprocess.run([str(KIT_BIN), "hook", "claude", "PreToolUse"], input=json.dumps(payload), cwd=cwd or workspace,
                          env=env, capture_output=True, text=True, timeout=60)
    return json.loads(done.stdout) if done.stdout.strip() else {}


def _denied(decision: dict) -> str:
    output = decision.get("hookSpecificOutput", {})
    return output.get("permissionDecisionReason", "") if output.get("permissionDecision") == "deny" else ""


def test_Hook_InAWorkspaceOfTheRecordedRepository_LetsTheCallThrough(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    (workspace / "sub").mkdir()
    assert not _denied(_hook(ctx, workspace))
    assert not _denied(_hook(ctx, workspace, cwd=workspace / "sub"))


def test_Hook_AForgedWorkspaceMarker_FailsClosedWithoutRunningGitThere(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    canary = tmp_path / "filter-ran"
    _forge(workspace, task_repository.path(ctx.store.read_state()), repository.common_dir(repo), canary)
    reason = _denied(_hook(ctx, workspace))
    assert "not the task repository's own" in reason and not canary.exists()


def test_Hook_OfARunWithoutTheEnginesNamedRepositories_FailsClosed(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    reason = _denied(_hook(ctx, workspace, SAKURA_FLOW_REPOSITORY=None, SAKURA_FLOW_TASK_REPOSITORY=None))
    assert "SAKURA_FLOW_REPOSITORY" in reason


def test_Hook_AFolderTheEngineDidNotMake_FailsClosed(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    other = tmp_path / "other"
    subprocess.run(["git", "init", "-q", str(other)], check=True)
    assert "not in a workspace the engine made" in _denied(_hook(ctx, workspace, cwd=other))


def _case_insensitive(folder: Path) -> bool:
    probe = folder / "CaseProbe"
    probe.write_text("")
    try:
        return (folder / "caseprobe").exists()
    finally:
        probe.unlink()


def _rename_case(path: Path, name: str) -> None:
    """Rename `path` to a case variant `name` of itself (two steps: a case-insensitive volume sees one name)."""
    middle = path.with_name(path.name + ".renaming")
    path.rename(middle)
    middle.rename(path.with_name(name))


def test_EngineGit_AWorkspaceMarkerNotNamedExactlyGit_IsRefused(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    if not _case_insensitive(tmp_path):
        pytest.skip("a case-sensitive volume has no case variant of .git for Git to find")
    _rename_case(workspace / ".git", ".GIT")
    with pytest.raises(gitutil.GitError, match="exactly"):
        gitutil.git(workspace, "status")


def test_ProjectRecord_NotNamedExactly_IsRefused(repo, tmp_path):
    ctx, _workspace = _row_task(repo, tmp_path)
    if not _case_insensitive(tmp_path):
        pytest.skip("a case-sensitive volume has no case variant of the record for the engine to find")
    common = task_repository.path(ctx.store.read_state())
    _rename_case(common / engine_home.TASK_REPOSITORY_RECORD, "Sakura-Project")
    with pytest.raises(engine_home.EngineHomeError, match="exactly"):
        engine_home.project_common_dir(common)


def test_EngineGit_AMarkerNamingItsEntryThroughALink_IsRefused(repo, tmp_path):
    ctx, workspace = _row_task(repo, tmp_path)
    common = task_repository.path(ctx.store.read_state())
    entry = Path((workspace / ".git").read_text().removeprefix("gitdir:").strip())
    link = tmp_path / "link"
    link.symlink_to(common)
    (workspace / ".git").write_text(f"gitdir: {link / 'worktrees' / entry.name}\n")
    with pytest.raises(gitutil.GitError, match="link"):
        gitutil.git(workspace, "status")


# -- review 10, finding 4: an operator command in a task workspace works on the person's repository -------------------

FAKE_GH = """#!/bin/sh
# A gh that, like the real one, finds its GitHub host in the remotes of the repository it runs in.
git remote -v 2>/dev/null | grep -q 'github.com' || { echo 'no git remotes found' >&2; exit 1; }
case "$2" in
  list) echo '[]' ;;
  create) echo 'https://github.com/owner/demo/pull/7' ;;
esac
"""


def _delivery_ready(repo, tmp_path, monkeypatch):
    """A task whose workspace 0, in its own repository, holds the verified change, the person's repository a GitHub
    origin, the push going to a fake bare remote and a gh that finds its host in the repository it runs in."""
    from sakura.flow import checks, contract
    from sakura.flow.phases import integrate

    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
    git(repo, "remote", "add", "origin", "https://github.com/owner/demo.git")
    ctx, workspace = _row_task(repo, tmp_path)
    (workspace / "work.txt").write_text("the task's change\n")
    git(workspace, "add", "work.txt")
    git(workspace, "-c", "core.hooksPath=/dev/null", "commit", "-qm", "the task's change")
    head = engine_head(ctx)
    row = contract.load(ctx.store, ctx.store.read_state()["contract_sha256"])[0]
    rows = [contract.Row(**{**row.__dict__, "passes": True})]
    contract.save(ctx.store, rows)
    import hashlib
    digest = hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest, full_green_head=head, push_url=str(remote),
                                                      phase="pr", pr={}, preset="standard"))
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256=integrate.full_tests_inputs_hash(ctx)))
    evidence = ctx.store.evidence_dir / "fixture.log"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text("exit 0\n")
    checks.record_receipt(ctx.store, rows[0], contract.rows_digest(rows), head, 0, evidence,
                          checks.EVALUATOR_RECEIPTS_KEY)
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "gh").write_text(FAKE_GH)
    (bin_dir / "gh").chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return ctx, workspace, head


def _resumed_from(location, task, monkeypatch):
    """The repository and store `sakura flow resume` drives when run in `location` (the engine itself is not run)."""
    from sakura.flow import cli, exits

    driven = {}
    monkeypatch.setattr(cli, "_drive", lambda repo, task_store, *_args: driven.update(repo=repo, store=task_store)
                        or exits.OK)
    monkeypatch.chdir(location)
    assert cli.main(["resume", task]) == exits.OK
    return driven["repo"], driven["store"]


def test_ResumeFromATaskWorkspace_DrivesThePersonsRepositoryAndOpensTheDraft(repo, tmp_path, monkeypatch):
    from sakura.flow.phases import pull_request

    ctx, workspace, head = _delivery_ready(repo, tmp_path, monkeypatch)
    person, task_store = _resumed_from(workspace, ctx.store.task_id, monkeypatch)
    assert person == repo and task_store.root == ctx.store.root
    resumed = engine.open_context(person, task_store)
    assert task_repository.workspace_root(resumed.config.values, resumed.repo) == \
        task_repository.workspace_root(ctx.config.values, repo)
    outcome = pull_request.run(resumed)
    assert outcome.kind == "next", outcome.reason
    assert task_store.read_state()["pr"] == {"url": "https://github.com/owner/demo/pull/7", "head": head}


def test_ResumeFromATaskWorkspace_IsTheSameAsFromThePersonsRepository(repo, tmp_path, monkeypatch):
    ctx, workspace, _head = _delivery_ready(repo, tmp_path, monkeypatch)
    from_workspace = _resumed_from(workspace, ctx.store.task_id, monkeypatch)
    from_repository = _resumed_from(repo, ctx.store.task_id, monkeypatch)
    assert from_workspace[0] == from_repository[0] == repo
    assert from_workspace[1].root == from_repository[1].root


def test_ResumeFromAWorkspaceWithAForgedMarker_IsRefused(repo, tmp_path, monkeypatch, capsys):
    from sakura.flow import cli

    ctx, workspace = _row_task(repo, tmp_path)
    _forge(workspace, task_repository.path(ctx.store.read_state()), repository.common_dir(repo), tmp_path / "canary")
    monkeypatch.setattr(cli, "_drive", lambda *_args: pytest.fail("resumed from a forged workspace"))
    monkeypatch.chdir(workspace)
    assert cli.main(["resume", ctx.store.task_id]) != 0
    assert not (tmp_path / "canary").exists()


def test_PreparingAWorkspaceARoleUsedBefore_RefusesAForgedMarkerBeforeAnyUncagedStep(repo, tmp_path, monkeypatch):
    # Intake's planner works in workspace 0 before the contract phase prepares it: the engine's uncaged install step
    # runs there only once the workspace checks out against the task repository.
    ctx, workspace = _row_task(repo, tmp_path)
    _forge(workspace, task_repository.path(ctx.store.read_state()), repository.common_dir(repo), tmp_path / "canary")
    ran = []
    monkeypatch.setattr(workspaces, "_run_logged", lambda *args, **_kwargs: ran.append(args[2]))
    with pytest.raises(workspaces.WorkspaceError, match="not the task repository's own"):
        workspaces.create(ctx, "0", "feat/flow-demo", ctx.store.read_state()["base_commit"])
    assert ran == [] and not (tmp_path / "canary").exists()


def test_StackAndPresetCommandsInATaskWorkspace_WorkOnThePersonsRepository(repo, tmp_path, monkeypatch, capsys):
    from sakura.flow import preset_cli, stack_cli

    ctx, workspace = _row_task(repo, tmp_path)
    monkeypatch.chdir(workspace)
    monkeypatch.delenv("SAKURA_FLOW_ROLE", raising=False)
    assert stack_cli.main(["up"]) != 0
    assert capsys.readouterr().out.strip() == f"no stack.up command is configured for {repo.name}"
    assert preset_cli._effective()["preset"] == config_preset(repo)


def config_preset(repo):
    from sakura.flow import config

    return config.load(repo).values["preset"]


# -- the folder that holds every workspace of a `workspace.root` template ----------------------------------------


def _root_values(template):
    return {"workspace": {"root": template}}


def test_WorkspacesRoot_RepoLastComponent_IsParent(tmp_path):
    root = task_repository.workspaces_folder(_root_values(f"{tmp_path}/trees/{{repo}}"))
    assert root == tmp_path / "trees"


def test_WorkspacesRoot_EnginePlaceholderAndHome_Expanded():
    root = task_repository.workspaces_folder(_root_values("{engine}-workspaces/{repo}"))
    assert root == Path(f"{engine_home.root()}-workspaces")
    home = task_repository.workspaces_folder(_root_values("~/trees/{repo}"))
    assert home == Path.home() / "trees"


def test_WorkspacesRoot_RepoInsideComponent_IsThatComponentsParent(tmp_path):
    values = _root_values(f"{tmp_path}/area/ws-{{repo}}/x")
    assert task_repository.workspaces_folder(values) == tmp_path / "area"
    assert "{repo}" not in str(task_repository.workspaces_folder(values))


def test_WorkspacesRoot_RepoInMiddle_CoversEveryWorkspace(tmp_path):
    values = _root_values(f"{tmp_path}/ws/{{repo}}/lanes")
    root = task_repository.workspaces_folder(values)
    for name in ("alpha", "beta"):
        workspace = task_repository.workspace_root(values, Path(f"/somewhere/{name}"))
        assert root == workspace.parents[1] and root in workspace.parents


def test_WorkspacesRoot_NoRepo_IsTemplate(tmp_path):
    assert task_repository.workspaces_folder(_root_values(f"{tmp_path}/shared")) == tmp_path / "shared"


def test_WorkspacesRoot_EnginePlaceholderPrefix_IsEngineRootsParent():
    root = task_repository.workspaces_folder(_root_values("{engine}-{repo}"))
    assert root == engine_home.root().parent and root.is_absolute()


def test_WorkspacesRoot_TildeRepo_IsHome():
    assert task_repository.workspaces_folder(_root_values("~/{repo}")) == Path.home()


@pytest.mark.parametrize("template", ["{repo}", "trees/{repo}", "relative/path", "", "ws-{repo}"])
def test_WorkspacesRoot_RelativeTemplate_Refused(template):
    with pytest.raises(task_repository.TaskRepositoryError, match="workspace.root"):
        task_repository.workspaces_folder(_root_values(template))


def test_task_store_rejects_invalid_ids_before_repository_lookup(repo, monkeypatch):
    from sakura.flow import gitutil, store

    calls = []
    monkeypatch.setattr(gitutil, "trust_repository", lambda path: calls.append(path))

    with pytest.raises(store.StateError, match="invalid task id"):
        store.TaskStore(repo, "invalid/name")

    assert calls == []
