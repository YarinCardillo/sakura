"""The repository's local Git config: refused at task start when it names a program, pinned for every engine Git call."""

import os
from pathlib import Path

import pytest

from conftest import git
from flow_support import make_context
from sakura import install, repository
from sakura.flow import config, git_config, gitutil, tasks
from sakura.flow.gitutil import GitError


@pytest.fixture(autouse=True)
def fresh_pins(monkeypatch):
    monkeypatch.setattr(gitutil, "_pinned_configs", {})


def _start_state(repo):
    return tasks.initial_state(repo, config.load(repo), "demo", "specs/demo.md", {})


@pytest.mark.parametrize("key, value", [
    ("filter.evil.smudge", "touch /tmp/x"), ("filter.evil.clean", "cat"), ("filter.evil.process", "evil-filter"),
    ("merge.evil.driver", "evil %O %A %B"), ("diff.evil.command", "evil"), ("diff.evil.textconv", "evil"),
    ("diff.external", "evil"), ("core.sshCommand", "ssh -o ProxyCommand=evil"), ("core.gitProxy", "evil"),
    ("core.askPass", "evil"), ("core.fsmonitor", ".git/hooks/fsmonitor"), ("remote.origin.receivepack", "evil"),
    ("remote.origin.uploadpack", "evil"), ("credential.helper", "!sh evil.sh"), ("credential.helper", "./helper"),
    ("credential.https://example.com.helper", "!evil"), ("include.path", "../shared.config"),
    ("includeIf.gitdir:/.path", "../shared.config"), ("protocol.ext.allow", "always"), ("gpg.program", "evil"),
    ("gpg.ssh.program", "evil"), ("gpg.x509.program", "evil"), ("gpg.ssh.defaultKeyCommand", "evil"),
    ("core.editor", "evil"), ("core.alternateRefsCommand", "evil"), ("sequence.editor", "evil")])
def test_TaskStart_ALocalConfigThatNamesAProgram_IsRefusedNamingTheKey(repo, key, value):
    git(repo, "config", key, value)
    with pytest.raises(tasks.TaskStartError, match="Remove them") as refused:
        tasks.check_git_config(repo)
    assert key.lower().split(".")[0] in str(refused.value).lower() and "config" in str(refused.value)


def test_TaskStart_ACredentialHelperInsideTheRepository_IsRefused(repo):
    git(repo, "config", "credential.helper", str(repo / "tools" / "helper"))
    with pytest.raises(tasks.TaskStartError, match="credential.helper"):
        tasks.check_git_config(repo)


def test_TaskStart_AProgramInALinkedWorktreesConfig_IsRefused(repo, tmp_path):
    git(repo, "config", "extensions.worktreeConfig", "true")
    git(repo, "worktree", "add", "-q", "-b", "side", str(tmp_path / "side"))
    git(tmp_path / "side", "config", "--worktree", "filter.evil.smudge", "evil")
    with pytest.raises(tasks.TaskStartError, match=r"filter\.evil\.smudge \(worktrees/side/config.worktree\)"):
        tasks.check_git_config(repo)


@pytest.mark.parametrize("key, value", [("credential.helper", "osxkeychain"), ("core.fsmonitor", "false"),
                                        ("protocol.ext.allow", "never"), ("user.name", "Someone"),
                                        ("credential.helper", "/usr/local/bin/git-credential-manager")])
def test_TaskStart_PlainSettingsAndHostHelpers_AreAccepted(repo, key, value):
    git(repo, "config", key, value)
    tasks.check_git_config(repo)


def test_TaskStart_RecordsTheDigestOfTheLocalConfig(repo):
    assert _start_state(repo)["git_config_sha256"] == git_config.digest(repository.common_dir(repo))


def _pinned_context(repo):
    ctx = make_context(repo, state={"git_config_sha256": git_config.digest(repository.common_dir(repo))})
    gitutil.pin_config(repo, ctx.store.read_state()["git_config_sha256"])
    return ctx


def test_EngineGit_AfterTheLocalConfigChanged_IsRefused(repo):
    _pinned_context(repo)
    gitutil.git(repo, "status")
    git(repo, "config", "core.sshCommand", "ssh -o ProxyCommand=evil")
    with pytest.raises(GitError, match="local Git config changed since the task started"):
        gitutil.git(repo, "status")
    git(repo, "config", "--unset", "core.sshCommand")
    gitutil.git(repo, "status")  # restored: the engine goes on


def test_EngineGit_ALinkedWorktreesConfigChanged_IsRefused(repo, tmp_path):
    git(repo, "config", "extensions.worktreeConfig", "true")
    git(repo, "worktree", "add", "-q", "-b", "side", str(tmp_path / "side"))
    _pinned_context(repo)
    (repository.common_dir(repo) / "worktrees" / "side" / "config.worktree").write_text('[filter "x"]\n\tsmudge = evil\n')
    with pytest.raises(GitError, match="changed since the task started"):
        gitutil.git(tmp_path / "side", "status")


def test_EngineGit_TheEnginesOwnWorkspaceInstall_DoesNotTripThePin(repo, tmp_path):
    _pinned_context(repo)
    gitutil.git(repo, "worktree", "add", "-q", "-b", "lane", str(tmp_path / "lane"))
    install.install(tmp_path / "lane")
    gitutil.git(tmp_path / "lane", "status")


def test_EngineGit_SwitchesOffTheExtTransportSigningProgramsAndSignatureChecks(repo):
    argv, _ = gitutil.command(repo, "status")
    pairs = {argv[index + 1] for index, word in enumerate(argv) if word == "-c"}
    assert {"protocol.ext.allow=never", "protocol.allow=user", "gpg.program=false", "gpg.ssh.program=false",
            "gpg.x509.program=false", "log.showSignature=false", "merge.verifySignatures=false"} <= pairs


def test_TaskStart_AnExtTransportPushUrl_IsRefused(repo):
    git(repo, "remote", "add", "origin", "ext::sh -c evil")
    with pytest.raises(tasks.TaskStartError, match="ext::"):
        _start_state(repo)


def test_Push_ARecordedExtTransportUrl_IsRefusedBeforeGitRuns(repo):
    from sakura.flow.phases import pull_request

    ctx = make_context(repo)
    outcome = pull_request._push(ctx, repo, git(repo, "rev-parse", "HEAD"), "feat/x", "ext::sh -c evil")
    assert outcome.kind == "blocked" and "ext::" in outcome.reason


def test_EngineGit_IgnoresTheSystemConfigAlwaysAndTheGlobalOneOutsideTheNetwork(repo):
    _, env = gitutil.command(repo, "status")
    assert env["GIT_CONFIG_NOSYSTEM"] == "1" and env["GIT_CONFIG_GLOBAL"] == "/dev/null"
    _, push = gitutil.command(repo, "push", "origin", "main")
    assert push["GIT_CONFIG_NOSYSTEM"] == "1" and "GIT_CONFIG_GLOBAL" not in push
    _, commit = gitutil.command(repo, "commit", "-m", "x")
    assert commit["GIT_AUTHOR_NAME"] == "Test User" and commit["GIT_CONFIG_GLOBAL"] == "/dev/null"


def test_EngineGit_ADriverInTheGlobalConfig_NeverRunsOnTheWorkspace(repo, tmp_path):
    marker = tmp_path / "smudged"
    Path(os.environ["HOME"], ".gitconfig").write_text(f'[filter "evil"]\n\tsmudge = touch {marker}; cat\n')
    (repo / ".gitattributes").write_text("* filter=evil\n")
    git(repo, "add", ".gitattributes")
    git(repo, "commit", "-qm", "attributes")
    make_context(repo)
    gitutil.git(repo, "worktree", "add", "-q", "--detach", str(tmp_path / "checkout"), "HEAD")
    assert (tmp_path / "checkout" / "README").read_text() == "initial\n" and not marker.exists()


def test_a_repository_on_another_branch_starts_without_a_profile_file(repo):
    git(repo, "branch", "-m", "trunk")

    state = tasks.initial_state(repo, config.load(repo), "demo", "text:demo", {})

    assert state["base_branch"] == "trunk" and state["base_commit"] == git(repo, "rev-parse", "trunk")


def test_the_remotes_default_branch_beats_the_checked_out_one(repo, tmp_path):
    git(repo, "branch", "develop")
    git(repo, "remote", "add", "origin", str(tmp_path / "unused.git"))
    git(repo, "update-ref", "refs/remotes/origin/develop", "develop")
    git(repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/develop")

    state = tasks.initial_state(repo, config.load(repo), "demo", "text:demo", {})

    assert state["base_branch"] == "develop"


def test_a_profile_integration_branch_beats_the_repositorys_own(repo, tmp_path):
    git(repo, "branch", "release")
    personal = Path(os.environ["HOME"]) / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('integration_branch = "release"\n')

    state = tasks.initial_state(repo, config.load(repo), "demo", "text:demo", {})

    assert state["base_branch"] == "release"


def test_a_missing_integration_branch_tells_the_person_what_to_set(repo):
    personal = Path(os.environ["HOME"]) / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('integration_branch = "nowhere"\n')

    with pytest.raises(tasks.TaskStartError, match="set integration_branch in ~/.config/sakura/profile.toml"):
        tasks.initial_state(repo, config.load(repo), "demo", "text:demo", {})


# -- secrets in the local config: no role reads it, so a task starts and the secret stays with the engine ---------------
# (The name heuristic that refused such a config is gone: the guarantee is that no cage is given the person's Git
# folder, `flow/task_repository.py`.)

SECRET = "ghp_0123456789abcdefSECRETVALUE"
PLANTED = ("planteduser", "PLANTEDTOKEN", "PLANTEDQUERY", "PLANTEDFRAGMENT")
PLANTED_URL = "https://planteduser:PLANTEDTOKEN@github.com/owner/?sig=PLANTEDQUERY#PLANTEDFRAGMENT"


def _assert_nothing_planted(text):
    for part in PLANTED:
        assert part not in text, part


@pytest.mark.parametrize("key, value", [
    ("sendemail.smtpPass", SECRET),
    ("remote.origin.url", f"https://x-access-token:{SECRET}@github.com/owner/repo.git"),
    (f"url.https://{SECRET}@github.com/.insteadOf", "https://github.com/"),
    ("http.https://github.com/.extraheader", f"AUTHORIZATION: bearer {SECRET}"),
    ("github.token", SECRET),
    ("something.endpoint", f"https://example.com/path?access_token={SECRET}"),
])
def test_TaskStart_ALocalConfigWithASecret_StartsAndNoTaskWorkspaceHoldsIt(repo, tmp_path, key, value):
    from sakura.flow import workspaces

    git(repo, "config", key, value)
    tasks.check_git_config(repo)
    base = git(repo, "rev-parse", "HEAD")
    ctx = make_context(repo, overrides={"workspace": {"root": str(tmp_path / "trees")}},
                       state={"base_commit": base, "branch": "feat/flow-demo", "workspaces": {}})
    workspace = workspaces.create(ctx, "0", "feat/flow-demo", base).path
    common = repository.common_dir(workspace)
    held = b"".join(path.read_bytes() for root in (common, workspace) for path in root.rglob("*")
                    if path.is_file() and not path.is_symlink())
    assert SECRET.encode() not in held
    assert gitutil.run(workspace, "config", "--get-regexp", ".").stdout.count(SECRET) == 0


def test_TaskStartFromTheCommandLine_ALocalSecret_IsNeitherRefusedNorPrinted(repo, monkeypatch, capsys):
    from sakura.flow import cli

    git(repo, "config", f"url.{PLANTED_URL}.insteadOf", "https://github.com/")
    git(repo, "config", "sendemail.smtpPass", SECRET)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *_args, **_kwargs: 0)
    code = cli.main(["start", "-m", "Add a greeting to the program"])
    captured = capsys.readouterr()
    assert code == 0, captured.out + captured.err
    _assert_nothing_planted(captured.out + captured.err)
    assert SECRET not in captured.out + captured.err


def test_ExecutableEntries_NeverShowAUrlSubsectionEither(repo):
    git(repo, "config", f"credential.{PLANTED_URL}.helper", "!evil")
    found = git_config.executable_entries(repository.common_dir(repo), repo)
    assert found == ["credential.<redacted>.helper (config)"]


def test_TransportEntries_AreTheCredentialAndHttpSettingsAsWritten_AndNothingElse(repo):
    git(repo, "config", "credential.helper", "osxkeychain")
    git(repo, "config", "credential.https://Example.com.username", "someone")
    git(repo, "config", "http.https://Example.com/.extraHeader", f"AUTHORIZATION: bearer {SECRET}")
    git(repo, "config", "sendemail.smtpPass", SECRET)
    git(repo, "config", "remote.origin.url", f"https://user:{SECRET}@example.com/owner/repo.git")
    found = git_config.transport_entries(repository.common_dir(repo))
    assert found == [("credential.helper", "osxkeychain"), ("credential.https://Example.com.username", "someone"),
                     ("http.https://Example.com/.extraheader", f"AUTHORIZATION: bearer {SECRET}")]
