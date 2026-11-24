from __future__ import annotations

import builtins
import io
import os
from pathlib import Path

import pytest

from sakura import cli
from sakura.accounts import detect, registry


def make_dir(path: Path, *subdirs: str) -> Path:
    for subdir in subdirs or (".",):
        (path / subdir).mkdir(parents=True, exist_ok=True)
    return path


def found(home: Path, env: dict | None = None) -> list[tuple[str, Path]]:
    candidates = detect.detect(home, {"HOME": str(home), **(env or {})})
    return [(candidate.provider, candidate.config_dir) for candidate in candidates]


def write_registry(home: Path, text: str) -> None:
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_detects_default_and_extra_claude_dirs(tmp_path):
    make_dir(tmp_path / ".claude", "projects")
    make_dir(tmp_path / ".work-claude", "projects")
    assert found(tmp_path) == [
        ("claude", (tmp_path / ".claude").resolve()),
        ("claude", (tmp_path / ".work-claude").resolve()),
    ]
    reasons = [candidate.reason for candidate in detect.detect(tmp_path, {"HOME": str(tmp_path)})]
    assert reasons == ["default ~/.claude", "dot-directory with projects/ (needs a check)"]


def test_detects_codex_home_from_env(tmp_path):
    elsewhere = make_dir(tmp_path / "elsewhere" / "codex-two", "sessions")
    candidates = detect.detect(tmp_path, {"HOME": str(tmp_path), "CODEX_HOME": str(elsewhere)})
    assert [(c.provider, c.config_dir, c.reason) for c in candidates] == [
        ("codex", elsewhere.resolve(), "$CODEX_HOME")]


def test_symlinked_dir_is_one_candidate(tmp_path):
    make_dir(tmp_path / ".claude", "projects")
    (tmp_path / ".alias").symlink_to(tmp_path / ".claude")
    env = {"HOME": str(tmp_path), "CLAUDE_CONFIG_DIR": str(tmp_path / ".alias")}
    candidates = detect.detect(tmp_path, env)
    assert [(c.provider, c.config_dir) for c in candidates] == [("claude", (tmp_path / ".claude").resolve())]
    proposals = detect.propose(candidates, registry.Registry((), True), tmp_path)
    assert [p.proposed_id for p in proposals] == ["default"]


def test_default_dir_reached_through_the_env_var_is_still_proposed_as_default(tmp_path):
    make_dir(tmp_path / ".claude", "projects")
    env = {"HOME": str(tmp_path), "CLAUDE_CONFIG_DIR": str(tmp_path / ".claude")}
    candidates = detect.detect(tmp_path, env)
    assert [c.reason for c in candidates] == ["$CLAUDE_CONFIG_DIR"]
    assert [p.proposed_id for p in detect.propose(candidates, registry.Registry((), True), tmp_path)] == ["default"]


def test_dir_without_projects_is_not_a_candidate(tmp_path):
    make_dir(tmp_path / ".claude")
    make_dir(tmp_path / ".cache", "other")
    make_dir(tmp_path / ".codex", "projects")
    assert found(tmp_path, {"CLAUDE_CONFIG_DIR": str(tmp_path / "missing")}) == [
        ("claude", (tmp_path / ".codex").resolve())]


def test_dir_holding_both_markers_is_a_candidate_for_each_provider(tmp_path):
    make_dir(tmp_path / ".both", "projects", "sessions")
    assert [provider for provider, _ in found(tmp_path)] == ["claude", "codex"]


def test_detection_never_opens_a_file(tmp_path, monkeypatch):
    make_dir(tmp_path / ".claude", "projects")
    make_dir(tmp_path / ".extra", "sessions")
    (tmp_path / ".claude" / ".credentials.json").write_text("secret", encoding="utf-8")

    def refuse(*args, **kwargs):
        raise AssertionError("detection opened a file")

    monkeypatch.setattr(builtins, "open", refuse)
    monkeypatch.setattr(io, "open", refuse)
    assert len(found(tmp_path)) == 2


def test_detect_marks_already_registered_dirs(tmp_path):
    make_dir(tmp_path / ".claude", "projects")
    make_dir(tmp_path / ".work", "projects")
    (tmp_path / "link").symlink_to(tmp_path / ".work")
    write_registry(tmp_path, '[[accounts]]\nprovider = "claude"\nid = "mine"\nconfig_dir = "~/link"\n')
    loaded = registry.load(env={"HOME": str(tmp_path)})
    proposals = detect.propose(detect.detect(tmp_path, {"HOME": str(tmp_path)}), loaded, tmp_path)
    assert [(p.proposed_id, p.registered_as) for p in proposals] == [("default", None), ("work", "mine")]


def test_proposed_ids_are_slugs_and_unique_per_provider(tmp_path):
    make_dir(tmp_path / ".Team Claude!", "projects")
    make_dir(tmp_path / ".team-claude-", "projects")
    proposals = detect.propose(detect.detect(tmp_path, {"HOME": str(tmp_path)}), registry.Registry((), True), tmp_path)
    assert [p.proposed_id for p in proposals] == ["team-claude-", "team-claude--2"]


def run_cli(tmp_path, monkeypatch, capsys) -> tuple[int, str]:
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    status = cli.main(["accounts", "detect"])
    return status, capsys.readouterr().out


def test_cli_detect_prints_one_line_per_candidate(tmp_path, monkeypatch, capsys):
    make_dir(tmp_path / ".claude", "projects")
    status, out = run_cli(tmp_path, monkeypatch, capsys)
    config_dir = (tmp_path / ".claude").resolve()
    assert status == 0
    assert out.splitlines() == [f"claude  default  {config_dir}  default ~/.claude"]


def test_cli_detect_with_nothing_found_says_so(tmp_path, monkeypatch, capsys):
    assert run_cli(tmp_path, monkeypatch, capsys) == (0, "no candidates\n")


def test_cli_detect_shows_the_registered_id_and_writes_nothing(tmp_path, monkeypatch, capsys):
    make_dir(tmp_path / ".claude", "projects")
    write_registry(tmp_path, '[[accounts]]\nprovider = "claude"\nid = "mine"\n')
    before = sorted(path for path in tmp_path.rglob("*"))
    status, out = run_cli(tmp_path, monkeypatch, capsys)
    assert status == 0 and out.rstrip().endswith("[registered as mine]")
    assert sorted(path for path in tmp_path.rglob("*")) == before


def test_detect_still_proposes_dot_directories(tmp_path, capsys, monkeypatch):
    make_dir(tmp_path / ".conductor", "projects")
    monkeypatch.setenv("HOME", str(tmp_path))
    assert cli.main(["accounts", "detect"]) == 0
    printed = capsys.readouterr().out
    assert str((tmp_path / ".conductor").resolve()) in printed
    assert "needs a check" in printed


@pytest.mark.skipif(os.geteuid() == 0, reason="root can read a mode 000 directory")
def test_unreadable_dot_directory_is_skipped(tmp_path):
    make_dir(tmp_path / ".claude", "projects")
    locked = make_dir(tmp_path / ".locked")
    (tmp_path / ".loop").symlink_to(tmp_path / ".loop")
    locked.chmod(0)
    try:
        assert found(tmp_path) == [("claude", (tmp_path / ".claude").resolve())]
        assert [a.id for a in detect.implicit_accounts({"HOME": str(tmp_path)})] == ["default"]
    finally:
        locked.chmod(0o700)
