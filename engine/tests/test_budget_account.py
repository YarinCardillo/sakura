import pytest

from sakura.budget import account
from sakura.usage.archive.settings import Source


def _claude(name, directory):
    return Source("claude", name, directory / "projects", directory)


def test_empty_account_returns_the_environment_unchanged(tmp_path):
    env = {"CLAUDE_CONFIG_DIR": "/elsewhere", "PATH": "/bin"}
    assert account.apply(env, "", [_claude("work", tmp_path / ".claude-work")]) == env


def test_account_directory_other_than_the_default_is_exported(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    env = account.apply({"PATH": "/bin"}, "work", [_claude("work", tmp_path / ".claude-work")])
    assert env == {"PATH": "/bin", "CLAUDE_CONFIG_DIR": str(tmp_path / ".claude-work")}


def test_default_directory_removes_an_inherited_variable(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    env = account.apply({"CLAUDE_CONFIG_DIR": "/elsewhere"}, "personal", [_claude("personal", tmp_path / ".claude")])
    assert "CLAUDE_CONFIG_DIR" not in env


def test_unknown_account_names_the_account_and_the_known_ones(tmp_path):
    with pytest.raises(account.UnknownAccountError, match=r"'ghost'.*work"):
        account.apply({}, "ghost", [_claude("work", tmp_path / ".claude-work")])


def test_codex_sources_and_claude_sources_without_a_directory_do_not_match(tmp_path):
    sources = [Source("codex", "work", tmp_path / "sessions"), Source("claude", "work", tmp_path / "odd")]
    with pytest.raises(account.UnknownAccountError, match="'work'"):
        account.apply({}, "work", sources)


def test_a_named_account_resolves_to_itself(tmp_path):
    assert account.resolve("work", []) == "work"


def test_an_empty_account_is_the_source_of_the_default_directory(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    sources = [_claude("work", tmp_path / ".claude-work"), _claude("personal", tmp_path / ".claude")]
    assert account.resolve("", sources, {}) == "personal"


def test_an_empty_account_is_the_source_of_the_engines_config_directory(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    sources = [_claude("work", tmp_path / ".claude-work"), _claude("personal", tmp_path / ".claude")]
    assert account.resolve("", sources, {"CLAUDE_CONFIG_DIR": str(tmp_path / ".claude-work")}) == "work"


def test_an_empty_account_without_a_matching_source_names_the_directory(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    with pytest.raises(account.UnknownAccountError, match=r"budget.account is empty.*\.claude"):
        account.resolve("", [_claude("work", tmp_path / ".claude-work")], {})


def _codex(name, tmp_path):
    return Source("codex", name, tmp_path / name / "sessions")


def test_codex_account_is_the_codex_source_named_like_the_claude_account(tmp_path):
    sources = [_claude("work", tmp_path), _codex("home", tmp_path), _codex("work", tmp_path)]
    assert account.codex_account("work", sources) == "codex:work"


def test_codex_account_is_the_only_codex_source_when_no_name_matches(tmp_path):
    assert account.codex_account("work", [_claude("work", tmp_path), _codex("home", tmp_path)]) == "codex:home"
    assert account.codex_account(None, [_codex("home", tmp_path)]) == "codex:home"


def test_codex_account_is_none_without_a_match_or_with_several_candidates(tmp_path):
    assert account.codex_account("work", [_claude("work", tmp_path)]) is None
    assert account.codex_account("work", [_codex("a", tmp_path), _codex("b", tmp_path)]) is None
