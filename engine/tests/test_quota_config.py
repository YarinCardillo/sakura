from pathlib import Path

import pytest

from sakura.quota import config

TEXT = """
port = 9000
codex_model = "codex-x"
[roles]
planner = "opus:high"
[[projects]]
name = "work"
paths = ["~/work/app", "/srv/other"]
claude = ["work", "personal"]
codex = ["personal"]
[default]
claude = ["personal"]
"""


def write(tmp_path, text):
    path = tmp_path / "quota.toml"
    path.write_text(text)
    return path


def test_Load_MissingFile_GivesDefaults(tmp_path):
    loaded = config.load(tmp_path / "nope.toml", tmp_path)
    assert (loaded.port, loaded.projects, loaded.default) == (8799, (), config.Allowed())


def test_Load_Full_ReadsEveryRule(tmp_path):
    loaded = config.load(write(tmp_path, TEXT), tmp_path)
    assert (loaded.port, loaded.codex_model, loaded.roles) == (9000, "codex-x", {"planner": "opus:high"})
    assert loaded.default == config.Allowed(("personal",), None)
    assert loaded.projects[0].allowed == config.Allowed(("work", "personal"), ("personal",))


def test_AllowedFor_RepoUnderProjectPath_MatchesTheProject(tmp_path):
    loaded = config.load(write(tmp_path, TEXT), tmp_path)
    name, allowed = loaded.allowed_for(tmp_path / "work/app/worktrees/x")
    assert (name, allowed.claude) == ("work", ("work", "personal"))


def test_AllowedFor_SiblingWithSamePrefix_IsNotTheProject(tmp_path):
    loaded = config.load(write(tmp_path, TEXT), tmp_path)
    assert loaded.allowed_for(tmp_path / "work/app2")[0] == "default"


def test_AllowedFor_UnmatchedRepo_UsesTheDefault(tmp_path):
    loaded = config.load(write(tmp_path, TEXT), tmp_path)
    assert loaded.allowed_for(Path("/elsewhere")) == ("default", config.Allowed(("personal",), None))


@pytest.mark.parametrize("text", ["port = 0", 'port = "x"', "bogus = 1", '[roles]\nbuilder2 = "x"',
                                  '[[projects]]\nname = "a"', "[[projects]]\nname = 1\npaths = []",
                                  '[default]\nclaude = "personal"'])
def test_Load_Malformed_RaisesNamingTheProblem(tmp_path, text):
    with pytest.raises(config.QuotaConfigError):
        config.load(write(tmp_path, text), tmp_path)
