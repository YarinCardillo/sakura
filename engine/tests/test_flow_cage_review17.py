"""Review 17: a replaced build root never reopens engine storage, and a role never reads the engine's Git settings."""

import dataclasses
import json
import os
import shutil
from pathlib import Path

import pytest

from sakura import engine_home, repository
from sakura.flow import cage

HOME = Path("/home/someone")


def _site(repo, role="checks", run="checks-1", **fields):
    roots = (engine_home.root(), engine_home.account_root())
    site = cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", run, role, repo, repository.common_dir(repo),
                        roots, repo)
    return dataclasses.replace(site, **fields)


def _replace_with_link(folder: Path, target: Path) -> None:
    folder.rename(folder.with_name(folder.name + "-moved"))
    folder.symlink_to(target, target_is_directory=True)


def _restore(build: Path) -> None:
    if build.is_symlink():
        build.unlink()
    shutil.rmtree(build.with_name(build.name + "-moved"), ignore_errors=True)
    shutil.rmtree(build, ignore_errors=True)


def test_Seatbelt_ABuildFolderLinkedToTheEngineHome_StillHasTheEngineHomeDeniedLast(repo):
    build = cage.new_build_dir()
    site = _site(repo, build_dir=build)
    run_cage = cage.prepare(site, cage.Policy("default"), 0, HOME)
    target = engine_home.root()
    _replace_with_link(build, target)
    try:
        profile = cage.seatbelt_profile(site, cage.Policy("default"), run_cage, HOME)
    finally:
        _restore(build)
    lines = profile.splitlines()
    real = str(target.resolve())
    grant = max(index for index, line in enumerate(lines) if line.startswith("(allow file-read* file-write*") and real in line)
    denial = max(index for index, line in enumerate(lines)
                 if line.startswith("(deny file-read* file-write*") and f'(subpath "{real}")' in line)
    assert denial > grant, "a denial of the engine home must follow every allowance that can resolve inside it"


def test_Seatbelt_TheEngineRootDenialsFollowEveryAllowance(repo):
    build = cage.new_build_dir()
    try:
        site = _site(repo, build_dir=build)
        profile = cage.prepare(site, cage.Policy("default"), 0, HOME).seatbelt.read_text()
    finally:
        _restore(build)
    lines = profile.splitlines()
    denial = max(index for index, line in enumerate(lines) if line.startswith("(deny file-read* file-write*"))
    later = [line for line in lines[denial + 1:] if line.startswith("(allow file-read* file-write*")]
    assert later == []


def test_Cage_ABuildFolderReplacedWhileTheFilesAreGenerated_IsRefusedAndWritesNothing(repo, monkeypatch):
    build = cage.new_build_dir()
    site = _site(repo, build_dir=build)
    first = cage.prepare(site, cage.Policy("default"), 0, HOME)
    written = {path.name: path.read_text() for path in (first.settings, first.seatbelt, first.policy)}
    real_codex_args = cage.codex_args

    def replacing(*arguments):
        _replace_with_link(build, engine_home.root())
        return real_codex_args(*arguments)

    monkeypatch.setattr(cage, "codex_args", replacing)
    try:
        with pytest.raises(cage.CageError, match="replaced"):
            cage.prepare(site, cage.Policy("default"), 0, HOME)
    finally:
        _restore(build)
    assert {path.name: path.read_text() for path in (first.settings, first.seatbelt, first.policy)} == written


# -- finding 2: the engine install's read grant never reaches its Git settings ----------------------------


def _install(root: Path, linked: bool = False) -> Path:
    """An engine installation: its runtime folders and the Git metadata of an ordinary clone or a linked worktree."""
    for name in ("bin", "lib/sakura", "git-hooks"):
        (root / name).mkdir(parents=True)
    (root / "bin" / "sakura").write_text("#!/bin/sh\n")
    (root / "docs").mkdir()
    if not linked:
        (root / ".git").mkdir()
        (root / ".git" / "config").write_text('[remote "origin"]\n\turl = https://token@example.invalid/demo.git\n')
        return root
    common = root.parent / "common" / ".git"
    (common / "worktrees" / "demo").mkdir(parents=True)
    (common / "config").write_text("[core]\n\tbare = false\n")
    (common / "worktrees" / "demo" / "config.worktree").write_text("[sendemail]\n\tsmtpPass = secret\n")
    (common / "worktrees" / "demo" / "commondir").write_text("../..\n")
    (root / ".git").write_text(f"gitdir: {common / 'worktrees' / 'demo'}\n")
    return root


def _reads_of(site, run_cage):
    settings = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]
    return settings["allowRead"], settings["denyRead"], run_cage.seatbelt.read_text(), " ".join(run_cage.codex_args)


@pytest.mark.parametrize("linked", [False, True])
def test_Cage_AnEngineRunFromAClone_GrantsNoRoleReadOfItsGitSettings(repo, tmp_path, monkeypatch, linked):
    install = _install(tmp_path / "install", linked)
    monkeypatch.setattr(cage, "KIT_BIN", install / "bin" / "sakura")
    site = _site(repo, "builder", "build-1")
    allow, deny, profile, codex = _reads_of(site, cage.prepare(site, cage.Policy("default"), 0, HOME))

    assert str(install / "bin") in allow and str(install / "lib") in allow and str(install / "git-hooks") in allow
    assert str(install) not in allow and str(install / "docs") not in allow
    assert f'(subpath "{os.path.realpath(install)}")' not in profile
    git_entry = os.path.realpath(install / ".git")
    assert git_entry in deny and f'(subpath "{git_entry}")' in profile and f'"{git_entry}" = "none"' in codex
    if linked:
        common = os.path.realpath(tmp_path / "common" / ".git")
        assert common in deny and f'(subpath "{common}")' in profile and f'"{common}" = "none"' in codex


# -- review 18, finding 1: grants come from the path captured when the root was made -----------------------------


def _swapped_while_generating(monkeypatch, build, target):
    """What a surviving child can do: point the build root at `target` while the profile is generated, put it back
    before the final check."""
    real_profile = cage.seatbelt_profile

    def swapping(*arguments):
        _replace_with_link(build, target)
        try:
            return real_profile(*arguments)
        finally:
            build.unlink()
            build.with_name(build.name + "-moved").rename(build)

    monkeypatch.setattr(cage, "seatbelt_profile", swapping)


@pytest.mark.parametrize("victim", ["secret", "install-lib"])
def test_Cage_ARootPointedAtAProtectedFolderWhileTheProfileIsGenerated_NamesOnlyTheCapturedPath(
        repo, tmp_path, monkeypatch, victim):
    target = tmp_path / "victim" / ("lib" if victim == "install-lib" else ".ssh")
    target.mkdir(parents=True)
    build = cage.new_build_dir()
    captured = os.path.realpath(build)
    site = _site(repo, build_dir=build)
    _swapped_while_generating(monkeypatch, build, target)
    try:
        run_cage = cage.prepare(site, cage.Policy("default"), 0, HOME)
        profile = run_cage.seatbelt.read_text()
    finally:
        _restore(build)

    assert str(target.resolve()) not in profile and str(target) not in profile
    assert f'(subpath "{captured}")' in profile
