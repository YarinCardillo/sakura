from __future__ import annotations

from pathlib import Path

import pytest

from sakura.lanes import config, identity


def make_group(tmp_path: Path, **overrides) -> config.LaneGroup:
    fields = dict(
        name="work",
        repo=tmp_path / "repo",
        session="work",
        worktree_root=tmp_path / "trees",
        branch="feat/{name}",
        base="staging",
        setup=("setup.sh",),
        up="ws up",
        down="ws down",
        status="ws dc ps",
        agent="claude-work",
    )
    fields.update(overrides)
    return config.LaneGroup(**fields)


def write_config(tmp_path: Path, *groups: str) -> Path:
    path = tmp_path / "lanes.toml"
    path.write_text("\n".join(groups))
    return path


def group_toml(name: str) -> str:
    return (
        f'[{name}]\nrepo = "~/{name}"\nsession = "{name}"\nworktree_root = "~/trees/{name}"\n'
        'branch = "feat/{name}"\nbase = "main"\nsetup = ["setup.sh"]\n'
        'up = "up"\ndown = "down"\nstatus = "st"\nagent = "claude-work"\n'
    )


@pytest.mark.parametrize("name", ["../x", "-rf", "a/b", "A", "", "a" * 41, "a b", "x\n"])
def test_rejects_traversal_option_and_uppercase(tmp_path, name):
    with pytest.raises(identity.LaneNameError):
        identity.resolve(make_group(tmp_path), name)


def test_accepts_plain_name_and_builds_identity(tmp_path):
    lane = identity.resolve(make_group(tmp_path), "fix-login-2")
    assert lane.group == "work" and lane.name == "fix-login-2"
    assert lane.branch == "feat/fix-login-2"
    assert lane.window_marker == "work/fix-login-2"
    assert lane.worktree == (tmp_path / "trees").resolve() / "fix-login-2"


def test_worktree_must_resolve_under_root(tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    root = tmp_path / "trees"
    root.mkdir()
    (root / "escape").symlink_to(outside)
    with pytest.raises(identity.LaneNameError, match="outside"):
        identity.resolve(make_group(tmp_path), "escape")


@pytest.mark.parametrize("template", ["feat/{name}.lock", "-x/{name}", "feat/{name}..", "fe at/{name}"])
def test_branch_ref_format_checked(tmp_path, template):
    with pytest.raises(identity.LaneNameError, match="branch"):
        identity.resolve(make_group(tmp_path, branch=template), "ok")


def test_group_required_when_ambiguous(tmp_path):
    path = write_config(tmp_path, group_toml("work"), group_toml("other"))
    groups = config.load(path)
    with pytest.raises(config.LaneConfigError, match="--group"):
        config.select(groups, None)
    assert config.select(groups, "other").name == "other"
    with pytest.raises(config.LaneConfigError, match="unknown"):
        config.select(groups, "nope")


def test_single_group_is_selected_without_a_name(tmp_path):
    groups = config.load(write_config(tmp_path, group_toml("work")))
    assert config.select(groups, None).name == "work"


def test_load_expands_home_and_reads_every_field(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "h"))
    group = config.load(write_config(tmp_path, group_toml("work")))["work"]
    assert group.repo == tmp_path / "h" / "work"
    assert group.worktree_root == tmp_path / "h" / "trees" / "work"
    assert group.setup == ("setup.sh",) and group.agent == "claude-work"


def test_load_reports_missing_field_and_missing_file(tmp_path):
    bad = write_config(tmp_path, '[work]\nrepo = "x"\n')
    with pytest.raises(config.LaneConfigError, match="is required"):
        config.load(bad)
    with pytest.raises(config.LaneConfigError, match="lanes.toml"):
        config.load(tmp_path / "absent" / "lanes.toml")
