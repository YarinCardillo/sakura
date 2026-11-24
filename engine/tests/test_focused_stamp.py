from __future__ import annotations


import subprocess

from sakura import engine_home, git_checks, repository, skips, stamps, testing

from conftest import git

FOCUSED = 'python3 -m pytest -q -rs -p no:cacheprovider tests'
SKIPPED_TEST = 'import pytest\n\n\n@pytest.mark.skip(reason="needs DB")\ndef test_db():\n    assert True\n'


def configure(repo, commands=(FOCUSED,)):
    config = repo / ".sakura/flow.toml"
    config.parent.mkdir(exist_ok=True)
    rendered = ", ".join(f'"{command}"' for command in commands)
    config.write_text(f"[tests]\nfocused = [{rendered}]\n")
    git(repo, "switch", "-qc", "feat/x")
    (repo / ".gitignore").write_text("__pycache__/\n")
    (repo / "tests").mkdir(exist_ok=True)
    (repo / "tests/test_ok.py").write_text("def test_ok():\n    assert True\n")
    git(repo, "add", ".")


def stage(repo, relative, text):
    path = repo / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    git(repo, "add", relative)


def test_focused_stamp_is_for_index_tree(repo):
    configure(repo)
    assert testing.run(repo, "focused") == 0
    tree = git(repo, "write-tree")
    assert stamps.read(repo, tree, "focused", [FOCUSED]) == {"skipped": 0, "entries": []}
    stage(repo, "other.txt", "x\n")
    assert stamps.read(repo, git(repo, "write-tree"), "focused", [FOCUSED]) is None


def test_pre_commit_refuses_without_focused_stamp(repo):
    configure(repo)
    ok, reason = git_checks.pre_commit(repo)
    assert not ok and "run sakura test focused" in reason
    assert testing.run(repo, "focused") == 0
    assert git_checks.pre_commit(repo) == (True, "")


def test_pre_commit_refuses_skip_in_touched_area(repo):
    configure(repo)
    stage(repo, "tests/test_db.py", SKIPPED_TEST)
    assert testing.run(repo, "focused") == 0
    ok, reason = git_checks.pre_commit(repo)
    assert not ok and "skipped tests in touched area: tests/test_db.py: needs DB" in reason


def test_pre_commit_allows_skip_elsewhere(repo):
    configure(repo)
    stage(repo, "other/test_db.py", SKIPPED_TEST)
    git(repo, "commit", "-qm", "seed", "--no-verify")
    (repo / "docs").mkdir()
    stage(repo, "docs/readme.txt", "hello\n")
    assert testing.run(repo, "focused") == 0
    assert git_checks.pre_commit(repo) == (True, "")


def test_pre_commit_skips_stamp_when_focused_is_empty(repo, capsys):
    git(repo, "switch", "-qc", "feat/x")
    assert git_checks.pre_commit(repo) == (True, "")
    assert "tests.focused is empty, stamp check skipped" in capsys.readouterr().out


def test_skips_touch_matches_path_directory_stem_and_unknown():
    entries = [skips.SkipEntry("tests/test_pages.py", "r"), skips.SkipEntry("web/button.spec.ts", "r"),
               skips.SkipEntry("far/test_z.py", "r")]
    assert git_checks.skips_touch(["tests/test_pages.py"], entries) == entries[:1]
    assert git_checks.skips_touch(["tests/other.py"], entries) == entries[:1]
    assert git_checks.skips_touch(["src/button.tsx"], entries) == [entries[1]]
    assert git_checks.skips_touch(["docs/a.md"], entries) == []
    unknown = skips.SkipEntry("", "reason not reported")
    assert git_checks.skips_touch(["docs/a.md"], [unknown]) == [unknown]


def test_fast_suite_reports_skips(repo, capsys):
    stage(repo, ".gitignore", "__pycache__/\n")
    stage(repo, "tests/test_db.py", SKIPPED_TEST)
    git(repo, "commit", "-qm", "skip")
    config = repo / ".sakura/profile.toml"
    config.parent.mkdir(exist_ok=True)
    config.write_text('[tests]\nfast = ["python3 -m pytest -q -rs -p no:cacheprovider tests"]\n')
    assert testing.run(repo, "fast") == 0
    out = capsys.readouterr().out
    assert "sakura test fast: 1 skipped" in out
    assert "skipped: tests/test_db.py: needs DB" in out
    tree = git_checks._tree(repo, "HEAD")
    assert stamps.has(repo, tree, "fast", ["python3 -m pytest -q -rs -p no:cacheprovider tests"])


def test_untracked_run_artifacts_do_not_refuse_the_stamp(repo):
    configure(repo, ('python3 -m pytest -q -rs tests',))
    (repo / ".gitignore").unlink()
    git(repo, "add", "-A")
    assert testing.run(repo, "focused") == 0
    assert stamps.read(repo, git(repo, "write-tree"), "focused", ["python3 -m pytest -q -rs tests"]) is not None


def test_run_modifying_tracked_file_refuses_the_stamp(repo):
    command = "echo changed >> tests/test_ok.py"
    configure(repo, (command,))
    assert testing.run(repo, "focused") == 1
    assert stamps.read(repo, git(repo, "write-tree"), "focused", [command]) is None


def test_corrupt_or_legacy_focused_stamp_refuses_commit(repo):
    configure(repo)
    tree = git(repo, "write-tree")
    target = stamps.path(repo, tree, "focused", [FOCUSED])
    target.parent.mkdir(parents=True)
    for content in ("passed\n", '{"skipped": 0, "ent'):
        target.write_text(content)
        ok, reason = git_checks.pre_commit(repo)
        assert not ok and "run sakura test focused" in reason


def test_legacy_fast_stamp_is_still_accepted_by_has(repo):
    tree = git(repo, "rev-parse", "HEAD^{tree}")
    target = stamps.path(repo, tree, "fast", ["true"])
    target.parent.mkdir(parents=True)
    target.write_text("passed\n")
    assert stamps.has(repo, tree, "fast", ["true"])


def test_run_hook_pre_commit_fails_closed_on_broken_flow_config(repo, monkeypatch, capsys):
    configure(repo)
    (repo / ".sakura/flow.toml").write_text("[tests\nbroken")
    monkeypatch.setattr(repository, "root", lambda: repo)
    assert git_checks.run_hook("pre-commit") == 1
    assert "sakura check failed closed" in capsys.readouterr().err


def test_run_hook_pre_commit_fails_closed_when_write_tree_fails(repo, monkeypatch, capsys):
    configure(repo)
    git(repo, "update-index", "--add", "--cacheinfo", f"100644,{'1' * 40},bad.txt")
    monkeypatch.setattr(repository, "root", lambda: repo)
    assert git_checks.run_hook("pre-commit") == 1
    assert "sakura check failed closed" in capsys.readouterr().err


def test_commit_a_checks_stamp_for_tree_actually_committed(repo):
    from sakura import install

    configure(repo)
    git(repo, "commit", "-qm", "seed", "--no-verify")
    install.install(repo)
    (repo / "tests/test_ok.py").write_text("def test_ok():\n    assert 1 == 1\n")
    refused = subprocess.run(["git", "-C", str(repo), "commit", "-qam", "edit"], capture_output=True, text=True)
    assert refused.returncode != 0 and "focused" in refused.stderr
    git(repo, "add", "-u")
    assert testing.run(repo, "focused") == 0
    (repo / "tests/test_ok.py").write_text("def test_ok():\n    assert 2 == 2\n")
    still = subprocess.run(["git", "-C", str(repo), "commit", "-qam", "edit"], capture_output=True, text=True)
    assert still.returncode != 0, "stamp for the staged tree must not cover the tree committed with -a"


def test_non_ascii_staged_paths_match_skip_entries(repo):
    entries = [skips.SkipEntry("tests/tëst_é.py", "r")]
    assert git_checks.skips_touch(["tests/tëst_é.py"], entries) == entries


def _forged_home(tmp_path, monkeypatch):
    forged = tmp_path / "forged-engine-home"
    forged.mkdir(mode=0o700)
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(forged))
    return forged


def test_a_stamp_forged_under_an_overridden_engine_home_is_not_trusted(repo, tmp_path, monkeypatch):
    configure(repo)
    trusted = stamps.path(repo, git(repo, "write-tree"), "focused", [FOCUSED])
    forged = _forged_home(tmp_path, monkeypatch) / trusted.relative_to(engine_home.root_for_trust())
    forged.parent.mkdir(parents=True)
    forged.write_text('{"skipped": 0, "entries": []}')
    assert stamps.path(repo, git(repo, "write-tree"), "focused", [FOCUSED]) == trusted
    ok, reason = git_checks.pre_commit(repo)
    assert not ok and "run sakura test focused" in reason


def test_a_personal_config_forged_under_an_overridden_engine_home_cannot_skip_the_stamp(repo, tmp_path, monkeypatch):
    configure(repo)
    forged = _forged_home(tmp_path, monkeypatch)
    (forged / "config.toml").write_text('relax = ["tests.focused"]\n\n[tests]\nfocused = []\n')
    ok, reason = git_checks.pre_commit(repo)
    assert not ok and "run sakura test focused" in reason
