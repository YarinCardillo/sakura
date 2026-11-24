from __future__ import annotations

import json
from pathlib import Path

import pytest

from sakura import agent_hooks, codex_trust, guard, install, repo_hooks

from conftest import git


def test_guard_denies_bypass_variants_case_insensitively(repo):
    assert guard.command_decision(repo, "git -c CORE.HOOKSPATH=/tmp push")
    assert guard.command_decision(repo, "git -c COMMIT.GPGSIGN=false commit -m x")
    assert guard.command_decision(repo, "GIT_CONFIG_PARAMETERS=x git status")
    assert guard.command_decision(repo, "git $(printf config) core.hooksPath x")
    assert guard.command_decision(repo, "git push --no-verify")


def test_guard_normalizes_wrappers_options_and_draft_booleans(repo):
    assert guard.command_decision(repo, "gh pr create --title x")[0] == "deny"
    assert guard.command_decision(repo, "gh pr create --draft --title x") is None
    assert guard.command_decision(repo, "gh pr create --draft=false --title x")[0] == "deny"
    assert guard.command_decision(repo, "env git push --no-verify")[0] == "deny"
    assert guard.command_decision(repo, "git -ccore.hooksPath=/tmp push")[0] == "deny"
    assert guard.command_decision(repo, "git commit -n -m x")[0] == "deny"
    assert guard.command_decision(repo, "/usr/bin/gh pr ready 1")[0] == "ask"


def test_guard_denies_control_syntax_and_abbreviated_bypass_options(repo):
    commands = [
        "git commit -an -m x",
        "git push --no-veri",
        "git --config-en=core.hooksPath=VALUE push",
        "gh pr create --dra=false --title x",
        "git status & whoami",
        "git status <(printf x)",
        "git status >(cat)",
    ]
    for command in commands:
        assert guard.command_decision(repo, command)[0] == "deny"


def test_guard_denies_git_and_gh_global_option_bypasses(repo):
    assert guard.command_decision(repo, "git -C . push --no-verify")[0] == "deny"
    assert guard.command_decision(repo, "git --git-dir .git commit -n -m x")[0] == "deny"
    assert guard.command_decision(repo, "VAR=/tmp git --config-env=core.hooksPath=VAR push")[0] == "deny"
    assert guard.command_decision(repo, "gh --repo owner/repo pr create --title x")[0] == "deny"
    assert guard.command_decision(repo, "gh --repo owner/repo pr ready 1")[0] == "ask"


def test_guard_fingerprint_fails_closed_when_missing_or_changed(repo):
    assert guard.changed_since_snapshot(repo, "s")
    guard.save_snapshot(repo, "s")
    assert not guard.changed_since_snapshot(repo, "s")
    target = repo / ".sakura/guarded"
    target.parent.mkdir()
    target.write_text("changed")
    assert guard.changed_since_snapshot(repo, "s")


def test_guard_retains_baseline_and_detects_mode_and_git_config_changes(repo):
    target = repo / ".codex/hooks.json"
    target.parent.mkdir()
    target.write_text("{}")
    guard.save_snapshot(repo, "s")
    target.chmod(0o600)
    assert guard.changed_since_snapshot(repo, "s")
    guard.save_snapshot(repo, "s")
    assert guard.changed_since_snapshot(repo, "s")


def test_guard_fingerprint_includes_git_configuration(repo):
    guard.save_snapshot(repo, "config")
    git(repo, "config", "core.hooksPath", "/tmp/other-hooks")
    assert guard.changed_since_snapshot(repo, "config")


def test_guard_fingerprint_ignores_branch_upstream_tracking_metadata(repo):
    guard.save_snapshot(repo, "upstream")
    git(repo, "config", "branch.main.remote", "origin")
    git(repo, "config", "branch.main.merge", "refs/heads/main")
    assert not guard.changed_since_snapshot(repo, "upstream")


def test_guarded_worktree_snapshot_is_checked_against_its_own_repository(repo, tmp_path):
    worktree = tmp_path / "worktree"
    git(repo, "config", "extensions.worktreeConfig", "true")
    git(repo, "worktree", "add", "-qb", "feature/snapshot", str(worktree), "HEAD")
    git(worktree, "config", "--worktree", "core.hooksPath", ".hooks")
    guard.save_snapshot(worktree, "worktree-session")
    assert not guard.any_snapshot_changed(repo)


def test_guard_fingerprint_ignores_git_worktree_environment_from_hook(repo, tmp_path, monkeypatch):
    other = tmp_path / "other"
    git(tmp_path, "init", "-q", "-b", "main", str(other))
    git(other, "config", "core.hooksPath", "/tmp/other-hooks")
    guard.save_snapshot(repo, "hook-env")
    monkeypatch.setenv("GIT_DIR", str(other / ".git"))
    assert not guard.changed_since_snapshot(repo, "hook-env")


@pytest.mark.parametrize("change", ["content", "mode"])
def test_guard_fingerprints_symlink_referent_content_and_mode(repo, change):
    referent = repo / "outside-settings.json"
    referent.write_text("{}")
    settings = repo / ".claude/settings.json"
    settings.parent.mkdir()
    settings.symlink_to(referent)
    guard.save_snapshot(repo, "symlink")
    if change == "content":
        referent.write_text('{"changed": true}')
    else:
        referent.chmod(0o600)
    assert guard.changed_since_snapshot(repo, "symlink")


def test_guarded_paths_include_all_agent_configuration(repo):
    paths = {str(path) for path in guard.guarded_paths(repo)}
    assert str(repo / ".claude") in paths
    assert str(repo / ".claude/settings.json") in paths
    assert str(repo / ".codex/config.toml") in paths
    assert str(Path.home() / ".claude/settings.json") in paths
    assert str(Path.home() / ".codex/hooks.json") in paths
    assert str(Path.home() / ".codex/config.toml") in paths


@pytest.mark.parametrize("tool", ["Edit", "Write", "apply_patch"])
def test_agent_hooks_guard_edit_operations_and_latch_fingerprint_failures(repo, tool):
    payload = {"tool_name": tool, "session_id": f"{tool}-edit", "tool_input": {}}
    assert agent_hooks._dispatch(repo, "PreToolUse", payload) is None
    target = repo / ".sakura/guarded"
    target.parent.mkdir()
    target.write_text("changed")
    assert agent_hooks._dispatch(repo, "PostToolUse", payload) == {
        "decision": "block",
        "reason": "sakura guarded-file fingerprint changed or is unavailable",
    }
    assert agent_hooks._dispatch(
        repo,
        "PreToolUse",
        {"tool_name": "Bash", "session_id": f"{tool}-edit", "tool_input": {"command": "git status"}},
    )["hookSpecificOutput"]["permissionDecision"] == "deny"


def test_agent_hook_denies_codex_ready_and_asks_claude(repo, monkeypatch, capsys):
    codex_payload = {"cwd": str(repo), "session_id": "codex", "tool_name": "Bash", "tool_input": {"command": "gh pr ready 1"}}
    monkeypatch.setattr("sys.stdin", type("Input", (), {"read": lambda self: json.dumps(codex_payload)})())
    agent_hooks.run("codex", "PreToolUse")
    assert json.loads(capsys.readouterr().out)["hookSpecificOutput"]["permissionDecision"] == "deny"
    claude_payload = {**codex_payload, "session_id": "claude"}
    monkeypatch.setattr("sys.stdin", type("Input", (), {"read": lambda self: json.dumps(claude_payload)})())
    agent_hooks.run("claude", "PreToolUse")
    assert json.loads(capsys.readouterr().out)["hookSpecificOutput"]["permissionDecision"] == "ask"


def test_repo_hooks_need_matching_explicit_trust(repo):
    settings = repo / ".claude/settings.json"
    settings.parent.mkdir()
    settings.write_text('{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}')
    assert repo_hooks.approved_commands(repo) == []
    repo_hooks.trust(repo)
    assert repo_hooks.approved_commands(repo) == ["true"]
    settings.write_text("{}")
    assert repo_hooks.approved_commands(repo) == []


def test_codex_trust_accepts_only_installer_owned_exact_command(repo):
    install.install(repo)
    commands = codex_trust.eligible_commands(repo)
    assert len(commands) == 2
    config = repo / ".codex/hooks.json"
    data = json.loads(config.read_text())
    data["hooks"]["PreToolUse"][0]["hooks"][0]["command"] += " ; false"
    config.write_text(json.dumps(data))
    assert len(codex_trust.eligible_commands(repo)) == 1


def test_codex_trust_rejects_newline_command_injection(repo):
    install.install(repo)
    config = repo / ".codex/hooks.json"
    data = json.loads(config.read_text())
    command = data["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
    data["hooks"]["PreToolUse"][0]["hooks"][0]["command"] = command.replace(" hook", "\nhook", 1)
    config.write_text(json.dumps(data))
    assert len(codex_trust.eligible_commands(repo)) == 1


def test_guard_denies_silent_check_chain(repo):
    for command in ("make check >/dev/null && git push", "make check &>/dev/null && git push", "pytest 2>/dev/null && git push"):
        decision = guard.command_decision(repo, command)
        assert decision and decision[0] == "deny" and "silent" in decision[1], command
    assert "silent" not in (guard.command_decision(repo, "git push --no-verify") or ("", ""))[1]


def test_guard_denies_scan_skip_env(repo):
    for command in ("SKIP=gitleaks git push", "GITLEAKS_ENABLE=false git commit -m x", "env SKIP=gitleaks git commit -m x"):
        decision = guard.command_decision(repo, command)
        assert decision and decision[0] == "deny" and "secret scan bypass" in decision[1], command
    assert guard.command_decision(repo, "FOO=1 git status") is None


def test_write_targets_cover_redirects_programs_and_in_place_edits(repo):
    targets = guard.write_targets(repo, "echo x > a.txt")
    assert targets == [(repo / "a.txt").resolve()]
    assert guard.write_targets(repo, "sed -i s/a/b/ b.txt") == [(repo / "b.txt").resolve()]
    assert guard.write_targets(repo, "perl -pi -e s/a/b/ c.txt") == [(repo / "c.txt").resolve()]
    assert guard.write_targets(repo, "rm -f d.txt") == [(repo / "d.txt").resolve()]
    assert guard.write_targets(repo, "ls") == []


def _hook_payload(repo, monkeypatch, tool="Read"):
    payload = {"cwd": str(repo), "session_id": "s", "tool_name": tool, "tool_input": {}}
    monkeypatch.setattr("sys.stdin", type("Input", (), {"read": lambda self: json.dumps(payload)})())


@pytest.mark.parametrize("event", ["PreToolUse", "PostToolUse"])
def test_engine_hook_marks_every_call_it_handles_for_the_run(repo, monkeypatch, event):
    from flow_support import make_context, task_workspace
    from sakura.flow import task_repository
    # An engine run's hook opens its task from the repositories the engine names, in a workspace the engine made.
    ctx = make_context(repo, own_repository=True, state={"workspaces": {}})
    workspace = task_workspace(ctx)
    task = ctx.store
    for key, value in task_repository.helper_environment(ctx.store).items():
        monkeypatch.setenv(key, value)
    monkeypatch.setenv("SAKURA_FLOW_TASK", "demo")
    monkeypatch.setenv("SAKURA_FLOW_RUN", "build-1")
    _hook_payload(workspace, monkeypatch)
    agent_hooks.run("codex", event)
    assert task.hook_mark("build-1").exists()
    assert not task.hook_mark("build-2").exists()
    assert not task.root.exists() or not list(task.root.rglob("*.hook-mark"))
    assert Path.home() in task.hook_mark("build-1").parents


def test_hook_outside_a_flow_run_marks_nothing(repo, monkeypatch):
    from sakura.flow import store
    monkeypatch.delenv("SAKURA_FLOW_TASK", raising=False)
    monkeypatch.delenv("SAKURA_FLOW_RUN", raising=False)
    _hook_payload(repo, monkeypatch)
    agent_hooks.run("codex", "PreToolUse")
    assert not store.TaskStore(repo, "demo").runs_dir.exists()


def test_engine_hook_in_a_role_repository_of_its_own_opens_the_task_and_allows_the_call(repo, monkeypatch, capsys):
    # Review 11: a reviewer or the reporter works in a repository of its own; its hooks open the task from the
    # repositories the engine names for that run and accept the workspace the state records for it.
    from flow_support import make_context, task_workspace
    from sakura.flow import task_repository
    ctx = make_context(repo, own_repository=True, state={"workspaces": {}})
    task_workspace(ctx)
    folder, worktree = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, "review")
    for key, value in task_repository.helper_environment(ctx.store, folder).items():
        monkeypatch.setenv(key, value)
    monkeypatch.setenv("SAKURA_FLOW_TASK", "demo")
    monkeypatch.setenv("SAKURA_FLOW_RUN", "review-1")
    _hook_payload(worktree, monkeypatch, tool="Bash")
    agent_hooks.run("claude", "PreToolUse")
    assert "failed closed" not in capsys.readouterr().out
    assert ctx.store.hook_mark("review-1").exists()
    task_repository.remove_role_repository(ctx.store, folder)
    _hook_payload(worktree.parent, monkeypatch, tool="Bash")
    agent_hooks.run("claude", "PreToolUse")
    assert "failed closed" in capsys.readouterr().out  # once forgotten, the folder is no workspace of the task


def _bash(command):
    return {"cwd": "", "session_id": "s", "tool_name": "Bash", "tool_input": {"command": command}}


def test_agent_hook_refuses_a_codex_command_that_starts_a_program_the_cage_denies_to_every_role(repo):
    payload = {**_bash("osascript -e 'return 1'"), "cwd": str(repo)}
    denied = agent_hooks._dispatch(repo, "PreToolUse", payload, "codex")
    assert denied["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert "osascript" in denied["hookSpecificOutput"]["permissionDecisionReason"]
    assert agent_hooks._dispatch(repo, "PreToolUse", {**payload, "tool_input": {"command": "ls"}}, "codex") is None


def test_agent_hook_leaves_a_claude_command_to_its_own_deny_rules_and_sandbox(repo):
    payload = {**_bash("osascript -e 'return 1'"), "cwd": str(repo)}
    assert agent_hooks._dispatch(repo, "PreToolUse", payload, "claude") is None
