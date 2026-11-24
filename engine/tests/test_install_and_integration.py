from __future__ import annotations

import json
import subprocess

import pytest

from sakura import install, integration

from conftest import git


def test_install_sets_worktree_hooks_excludes_overlay_and_writes_configs(repo):
    install.install(repo)
    hooks = repo / ".sakura/git-hooks"
    assert (hooks / "pre-commit").is_symlink()
    assert git(repo, "config", "--worktree", "core.hooksPath") == str(hooks)
    assert ".sakura/local.toml" in (repo / ".git/info/exclude").read_text()
    assert "hooks" in json.loads((repo / ".claude/settings.local.json").read_text())
    assert "hooks" in json.loads((repo / ".codex/hooks.json").read_text())
    git(repo, "switch", "-qc", "feat/hook-smoke")
    (repo / "hook-smoke").write_text("ok")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "hook smoke")


def test_install_merges_existing_agent_hook_entries(repo):
    claude = repo / ".claude/settings.local.json"
    codex = repo / ".codex/hooks.json"
    claude.parent.mkdir()
    codex.parent.mkdir()
    claude.write_text(json.dumps({"hooks": {"PreToolUse": [{"hooks": [{"command": "existing-claude"}]}]}}))
    codex.write_text(json.dumps({"hooks": {"PreToolUse": [{"hooks": [{"command": "existing-codex"}]}]}}))
    install.install(repo)
    assert json.loads(claude.read_text())["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == "existing-claude"
    assert json.loads(codex.read_text())["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == "existing-codex"


def test_installed_pre_push_accepts_git_remote_arguments_and_succeeds(repo, tmp_path):
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", "-q", str(remote)], check=True)
    install.install(repo)
    git(repo, "switch", "-qc", "feat/push")
    (repo / "push").write_text("ok\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "push")
    git(repo, "remote", "add", "origin", str(remote))
    done = subprocess.run(["git", "-C", str(repo), "push", "origin", "feat/push"], capture_output=True, text=True)
    assert done.returncode == 0, done.stderr


def test_installed_pre_push_refuses_protected_destination(repo, tmp_path):
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", "-q", str(remote)], check=True)
    install.install(repo)
    git(repo, "switch", "-qc", "feat/push")
    (repo / "push").write_text("no\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "push")
    git(repo, "remote", "add", "origin", str(remote))
    done = subprocess.run(["git", "-C", str(repo), "push", "origin", "HEAD:main"], capture_output=True, text=True)
    assert done.returncode != 0
    assert "protected" in done.stderr


def test_integra_merges_local_pull_refs_and_sets_nonexistent_push_remote(repo, tmp_path):
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", "-q", str(remote)], check=True)
    git(repo, "remote", "add", "origin", str(remote))
    git(repo, "push", "-q", "origin", "main")
    git(repo, "switch", "-qc", "feat/pr")
    (repo / "change").write_text("x")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "change")
    sha = git(repo, "rev-parse", "HEAD")
    git(repo, "push", "-q", "origin", f"{sha}:refs/pull/7/head")
    git(repo, "switch", "-q", "main")
    assert integration.run(repo, ["7"], False) == 0
    assert git(repo, "branch", "--show-current") == "sakura-integration/local"
    assert git(repo, "config", "branch.sakura-integration/local.pushRemote") == "sakura-no-push"


def test_integra_lists_conflicting_files(repo, tmp_path, capsys):
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", "-q", str(remote)], check=True)
    git(repo, "remote", "add", "origin", str(remote))
    git(repo, "push", "-q", "origin", "main")
    (repo / "README").write_text("base\n")
    git(repo, "commit", "-am", "base")
    git(repo, "push", "-q", "origin", "main")
    git(repo, "switch", "-qc", "feat/conflict", "HEAD~1")
    (repo / "README").write_text("pr\n")
    git(repo, "commit", "-am", "pr")
    git(repo, "push", "-q", "origin", f"HEAD:refs/pull/8/head")
    git(repo, "switch", "-q", "main")
    assert integration.run(repo, ["8"], False) == 1
    assert "README" in capsys.readouterr().out


def test_install_files_writes_the_hooks_and_changes_no_git_config_or_exclude(repo):
    config_before = (repo / ".git/config").read_bytes()
    exclude_before = (repo / ".git/info/exclude").read_bytes()

    install.install_files(repo)

    assert (repo / ".sakura/git-hooks/pre-commit").is_symlink()
    assert "hooks" in json.loads((repo / ".claude/settings.local.json").read_text())
    assert (repo / ".git/config").read_bytes() == config_before
    assert (repo / ".git/info/exclude").read_bytes() == exclude_before
    assert not (repo / ".git/config.worktree").exists()


def _outside_settings(tmp_path):
    """A folder of the person's, outside every checkout, with a settings file the engine must never touch."""
    outside = tmp_path / "person-claude"
    outside.mkdir()
    (outside / "settings.local.json").write_text('{"hooks": {}, "mine": true}\n')
    return outside


def _snapshot(folder):
    return {path.relative_to(folder): (path.is_symlink(), path.read_bytes() if path.is_file() else b"")
            for path in sorted(folder.rglob("*"))}


@pytest.mark.parametrize("linked", [".claude", ".codex", ".sakura", ".sakura/git-hooks"])
def test_InstallFiles_ACommittedLinkToAnOutsideFolder_LeavesItByteIdenticalAndIsRefused(repo, tmp_path, linked):
    # Review 14, finding 1: a checkout's committed link must not carry the engine's writes outside it.
    outside = _outside_settings(tmp_path)
    before = _snapshot(outside)
    target = repo / linked
    target.parent.mkdir(parents=True, exist_ok=True)
    target.symlink_to(outside)
    git(repo, "add", linked)
    git(repo, "commit", "-qm", "a link to the person's folder")

    with pytest.raises(install.InstallRefused, match=linked):
        install.install_files(repo)
    assert _snapshot(outside) == before


def test_InstallFiles_ASettingsFileLinkedOutside_IsRefusedAndLeftAlone(repo, tmp_path):
    outside = _outside_settings(tmp_path)
    before = _snapshot(outside)
    (repo / ".claude").mkdir()
    (repo / ".claude/settings.local.json").symlink_to(outside / "settings.local.json")

    with pytest.raises(install.InstallRefused, match="settings.local.json"):
        install.install_files(repo)
    assert _snapshot(outside) == before


def test_InstallFiles_AnUntrackedLinkedHooksFolder_IsRefused(repo, tmp_path):
    outside = _outside_settings(tmp_path)
    before = _snapshot(outside)
    (repo / ".sakura").mkdir()
    (repo / ".sakura/git-hooks").symlink_to(outside)

    with pytest.raises(install.InstallRefused, match="git-hooks"):
        install.install_files(repo)
    assert _snapshot(outside) == before and not (outside / "pre-commit").exists()
