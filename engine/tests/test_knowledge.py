"""Knowledge folders: shared and per project, read by every cage, written only through accepted proposals."""

import json
import os
from pathlib import Path

import pytest

from sakura import engine_home, knowledge, repository
from sakura.flow import cage, config, knowledge_folders

HOME = Path("/home/someone")


def _values(shared="", project=""):
    return {"knowledge": {"shared": str(shared), "project": str(project)}}


@pytest.fixture
def notes(tmp_path):
    folder = tmp_path / "notes"
    folder.mkdir()
    (folder / "style.md").write_text("Use plain words.\n")
    return folder


def _configured(values, forbidden=(), home=HOME):
    return knowledge.configured(values, home, forbidden)


def test_Knowledge_Unset_NoFolders():
    assert _configured(_values()) == {}
    assert _configured({}) == {}


def test_Knowledge_SharedAndProject_ReturnedByNameAsRealPaths(tmp_path, notes):
    link = tmp_path / "link"
    link.symlink_to(notes)
    other = tmp_path / "other"
    other.mkdir()
    assert _configured(_values(link, other)) == {"shared": notes.resolve(), "project": other.resolve()}


def test_Knowledge_TildeFolder_ExpandedAgainstTheGivenHome(tmp_path):
    (tmp_path / "kb").mkdir()
    assert _configured(_values("~/kb"), home=tmp_path) == {"shared": (tmp_path / "kb").resolve()}


@pytest.mark.parametrize("name", [".env", ".env.local", ".npmrc", ".netrc", "id_rsa", "id_ed25519.pub", "server.pem",
                                  "token.p12"])
def test_Knowledge_FolderHoldingSecret_Refused(notes, name):
    (notes / "deep").mkdir()
    (notes / "deep" / name).write_text("x")
    with pytest.raises(knowledge.KnowledgeRefused, match="credential"):
        _configured(_values(notes))


def test_Knowledge_LinkLeavingTheFolder_Refused(tmp_path, notes):
    outside = tmp_path / "outside.txt"
    outside.write_text("x")
    (notes / "leak.md").symlink_to(outside)
    with pytest.raises(knowledge.KnowledgeRefused, match="link"):
        _configured(_values(notes))


def test_Knowledge_LinkInsideTheFolder_Allowed(notes):
    (notes / "alias.md").symlink_to(notes / "style.md")
    assert _configured(_values(notes)) == {"shared": notes.resolve()}


def test_Knowledge_TooManyEntries_Refused(notes, monkeypatch):
    monkeypatch.setattr(knowledge, "MAX_ENTRIES", 1)
    (notes / "second.md").write_text("x")
    with pytest.raises(knowledge.KnowledgeRefused, match="more than"):
        _configured(_values(notes))


def test_Knowledge_FolderIsAForbiddenPath_Refused(notes):
    with pytest.raises(knowledge.KnowledgeRefused, match="secret or engine"):
        _configured(_values(notes), forbidden=[notes])


def test_Knowledge_FolderInsideAForbiddenPath_Refused(tmp_path):
    inside = tmp_path / "engine" / "sub"
    inside.mkdir(parents=True)
    with pytest.raises(knowledge.KnowledgeRefused, match="secret or engine"):
        _configured(_values(inside), forbidden=[tmp_path / "engine"])


def test_Knowledge_FolderHoldingAForbiddenPath_Refused(notes):
    (notes / "secret").mkdir()
    with pytest.raises(knowledge.KnowledgeRefused, match="secret or engine"):
        _configured(_values(notes), forbidden=[notes / "secret"])


@pytest.mark.parametrize("folder", ["home", "above-home"])
def test_Knowledge_HomeOrAboveIt_Refused(tmp_path, folder):
    home = tmp_path / "people" / "me"
    home.mkdir(parents=True)
    chosen = home if folder == "home" else home.parent
    with pytest.raises(knowledge.KnowledgeRefused, match="home"):
        _configured(_values(chosen), home=home)


@pytest.mark.parametrize("raw", ["relative/folder", "~other/x"])
def test_Knowledge_NotAbsolute_Refused(raw):
    with pytest.raises(knowledge.KnowledgeRefused, match="absolute"):
        _configured(_values(raw))


def test_Knowledge_NotAFolder_Refused(tmp_path):
    (tmp_path / "file.md").write_text("x")
    for chosen in (tmp_path / "missing", tmp_path / "file.md"):
        with pytest.raises(knowledge.KnowledgeRefused, match="not a folder"):
            _configured(_values(chosen))


def test_Knowledge_RefusalNamesTheKey(notes):
    (notes / ".env").write_text("x")
    with pytest.raises(knowledge.KnowledgeRefused, match=r"knowledge\.project"):
        _configured(_values(project=notes))


# -- configuration ---------------------------------------------------------------------------------------------------


def test_Config_KnowledgeDefaults_BothEmpty(repo):
    assert config.load(repo).values["knowledge"] == {"shared": "", "project": ""}


def test_Config_KnowledgeShared_PersonalLayerOnly(repo):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text('[knowledge]\nshared = "/somewhere"\n')
    with pytest.raises(config.FlowConfigError, match="knowledge.shared"):
        config.load(repo)


def test_Config_KnowledgeProject_NotFromARepositoryLayer(repo):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text('[knowledge]\nproject = "/somewhere"\n')
    with pytest.raises(config.FlowConfigError, match="knowledge.project"):
        config.load(repo)


def test_Config_KnowledgeFromThePersonalLayer_Loads(repo):
    (engine_home.root() / "config.toml").write_text('[knowledge]\nshared = "/somewhere"\n')
    assert config.load(repo).values["knowledge"]["shared"] == "/somewhere"


@pytest.mark.parametrize("line", ["shared = 3", 'shared = "not/absolute"'])
def test_Config_KnowledgeNotAnAbsolutePath_Refused(repo, line):
    (engine_home.root() / "config.toml").write_text(f"[knowledge]\n{line}\n")
    with pytest.raises(config.FlowConfigError, match="knowledge.shared"):
        config.load(repo)


# -- the cage --------------------------------------------------------------------------------------------------------


@pytest.fixture
def no_writable_roots(monkeypatch):
    """The test folders live under the system temp folder, which is a root a role can write; the check is its own test."""
    monkeypatch.setattr(cage, "writable_roots", lambda: ())


def _site(repo, folders):
    roots = (engine_home.root(), engine_home.account_root())
    return cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "build-1", "builder", repo,
                        repository.common_dir(repo), roots, repo, knowledge={name: str(path) for name, path in folders.items()})


def test_Knowledge_BuilderCage_ReadsSharedFolderCannotWrite(repo, notes):
    run_cage = cage.prepare(_site(repo, {"shared": notes.resolve()}), cage.Policy("default"), 2, HOME)
    settings = json.loads(run_cage.settings.read_text())
    folder = str(notes.resolve())
    assert folder in settings["sandbox"]["filesystem"]["allowRead"]
    assert folder not in settings["sandbox"]["filesystem"]["allowWrite"]
    assert f"Read(/{folder}/**)" in settings["permissions"]["allow"]
    assert not any(rule.startswith(("Edit", "Write")) and folder in rule for rule in settings["permissions"]["allow"])
    profile = run_cage.seatbelt.read_text()
    assert f'(subpath "{folder}")' in profile.split("(deny file-write*")[0]
    assert folder not in "".join(line for line in profile.splitlines() if line.startswith("(deny file-write* (require-not"))
    assert any(folder in argument for argument in run_cage.codex_args)


def test_Knowledge_CageWithoutFolders_ReadsNoMore(repo):
    before = cage.prepare(_site(repo, {}), cage.Policy("default"), 2, HOME)
    assert not [rule for rule in json.loads(before.settings.read_text())["permissions"]["allow"] if rule.startswith("Read(")]


def test_KnowledgeFolders_ChecksAgainstSecretsEngineAndWritableRoots(repo, notes, tmp_path):
    values = config.load(repo).values
    values["knowledge"]["shared"] = str(notes)
    with pytest.raises(knowledge.KnowledgeRefused, match="role can write"):
        knowledge_folders.of(values, HOME)
    values["knowledge"]["shared"] = str(engine_home.root())
    with pytest.raises(knowledge.KnowledgeRefused):
        knowledge_folders.of(values, HOME)
    values["knowledge"]["shared"] = str(HOME / ".ssh")
    with pytest.raises(knowledge.KnowledgeRefused):
        knowledge_folders.of(values, HOME)


def test_KnowledgeFolders_AFolderOutsideEveryWritableRoot_Accepted(repo, notes, no_writable_roots):
    values = config.load(repo).values
    values["knowledge"]["shared"] = str(notes)
    assert knowledge_folders.of(values, HOME) == {"shared": notes.resolve()}


def test_Runner_KnowledgeFolderThatHoldsACredential_RefusesTheRun(repo, notes, no_writable_roots):
    from flow_support import make_context
    from sakura.flow import roles, runner

    ctx = make_context(repo, overrides={"knowledge": {"shared": str(notes)}})
    request = runner.RunRequest("builder", "build", "prompt", {}, repo)
    assert runner._knowledge(ctx.config, request, repo / ".git") == {"shared": str(notes.resolve())}
    (notes / ".env").write_text("x")
    with pytest.raises(roles.RoleRefused, match=r"knowledge\.shared"):
        runner._knowledge(ctx.config, request, repo / ".git")
