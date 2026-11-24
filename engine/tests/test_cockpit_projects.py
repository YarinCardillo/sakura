"""The projects of the cockpit: listed with no lanes, added by a folder, a lane started in one."""

import json
import os

import pytest

from sakura import actors
from sakura import engine_home, git_program
from sakura.cockpit import api, cli, gate, host_actions, review_waits
from sakura.cockpit.controller import PROOF_SOURCE_OVERLAY, Controller, Refused, overlay_actor
from sakura.cockpit.project_actions import target_of
from sakura.cockpit.snapshot import collect
from sakura.flow import delivery, exits, gitutil, registry

from datetime import datetime, timezone

from conftest import git
from test_cockpit_actions import Spawns
from test_cockpit_snapshot import NoTmux, create_task, make_snapshot, other_repo

NOW = datetime(2026, 11, 17, 9, 0, tzinfo=timezone.utc)
ADD_PROJECT = "add-project"


def run_act(capsys, argv, **kwargs):
    code = cli.main(["act", *argv, "--json"], client=NoTmux(), **kwargs)
    return code, json.loads(capsys.readouterr().out)


def documented_projects():
    return api.snapshot_document(collect(registry.repos(), NOW))["projects"]


def test_Snapshot_RegisteredProjectWithoutLanes_Listed(repo):
    registry.remember(repo)

    (project,) = documented_projects()

    assert project["name"] == repo.name
    assert project["folder"] == str(repo)
    assert project["open_lanes"] == 0
    assert project["id"] == json.dumps(["project", str(repo)], separators=(",", ":"))
    assert [action["key"] for action in project["actions"]] == ["L", "W"]


def test_Snapshot_ProjectWithAnOpenLane_CountsOnlyOpenLanes(repo):
    create_task(repo, "alpha", status="running")
    create_task(repo, "beta", status="cleaned")
    registry.remember(repo)

    (project,) = documented_projects()

    assert project["open_lanes"] == 1


def test_Snapshot_Project_NamesTheBranchItsFolderIsOn(repo):
    git(repo, "checkout", "-q", "-b", "feat/project-branch")
    registry.remember(repo)

    (project,) = documented_projects()

    assert project["branch"] == "feat/project-branch"


def test_Snapshot_ProjectOnADetachedHead_BranchIsEmpty(repo):
    git(repo, "checkout", "-q", "--detach")
    registry.remember(repo)

    (project,) = documented_projects()

    assert project["branch"] == ""


@pytest.mark.parametrize("bare", [False, True])
def test_Snapshot_MissingProject_KeepsLaneAndWaitIdentities(repo, tmp_path, monkeypatch, bare):
    if bare:
        repo = repo.parent / "bare"
        git(repo.parent, "init", "-q", "--bare", str(repo))
    registry.remember(repo)
    create_task(repo, "blocked", status="blocked", reason="run failed")
    create_task(repo, "approval", status="awaiting_approval")
    review = create_task(repo, "review", status="report_ready")
    delivery.record(review, delivery.PULL_REQUEST, "a" * 40, "feature/inbox",
                    "https://github.com/example/project/pull/42")
    monkeypatch.setattr(review_waits, "_pr_state", lambda url, repo: "OPEN")
    before = api.snapshot_document(cli._collect())
    moved = tmp_path / "repo-moved"
    repo.rename(moved)
    monkeypatch.setattr(review_waits, "_pr_state", lambda *args: pytest.fail("looked up a PR from a missing folder"))

    after = api.snapshot_document(cli._collect())

    for section in ("projects", "items", "waits"):
        assert [row["id"] for row in after[section]] == [row["id"] for row in before[section]]
        assert all(row["folder_missing"] is True for row in after[section])
    assert after["projects"][0]["open_lanes"] == 2
    assert after["projects"][0]["actions"] == []
    assert all(row["actions"] == [] for row in after["items"])
    assert all("read" in {action["key"] for action in row["actions"]} for row in after["waits"])
    assert not {"a", "R", "s"} & {action["key"] for row in after["waits"] for action in row["actions"]}

    moved.rename(repo)
    monkeypatch.setattr(review_waits, "_pr_state", lambda url, repo: "OPEN")
    restored = api.snapshot_document(cli._collect())
    assert [row["id"] for row in restored["waits"]] == [row["id"] for row in before["waits"]]
    assert all(row["folder_missing"] is False for section in ("projects", "items", "waits") for row in restored[section])


@pytest.mark.parametrize(("section", "action", "options"), [
    ("projects", "L", ["--text", "Start this lane"]),
    ("items", "R", []),
    ("waits", "R", []),
])
def test_Act_OnMissingProject_RefusesInWords(repo, tmp_path, capsys, section, action, options):
    registry.remember(repo)
    create_task(repo, "blocked", status="blocked", reason="run failed")
    before = api.snapshot_document(cli._collect())
    repo.rename(tmp_path / "repo-moved")
    spawns = Spawns()

    code, answer = run_act(capsys, [action, before[section][0]["id"], *options], run=spawns)

    assert code == exits.USAGE
    assert answer == {"result": "refused", "reason": "The project folder is missing. Restore it and try again."}
    assert spawns.calls == []


def test_Snapshot_NoRegisteredProject_ProjectsIsEmpty():
    assert documented_projects() == []


def test_ActAddProject_GitFolder_RegisteredAndInSnapshot(repo, capsys):
    code, answer = run_act(capsys, [ADD_PROJECT, str(repo)])

    assert code == exits.OK
    assert answer["result"] == "done" and answer["exit_code"] == 0
    assert json.loads(answer["output"]) == {"name": repo.name, "folder": str(repo)}
    assert [project["folder"] for project in documented_projects()] == [str(repo)]


def test_ActAddProject_SubfolderOfGitFolder_RegistersTheRepository(repo, capsys):
    (repo / "docs").mkdir()

    code, answer = run_act(capsys, [ADD_PROJECT, str(repo / "docs")])

    assert code == exits.OK and answer["result"] == "done"
    assert [project["folder"] for project in documented_projects()] == [str(repo)]


def test_ActAddProject_NotAGitFolder_RefusedWithReason(tmp_path, capsys):
    plain = tmp_path / "plain"
    plain.mkdir()

    code, answer = run_act(capsys, [ADD_PROJECT, str(plain)])

    assert code == exits.USAGE
    assert answer["result"] == "refused"
    assert "not a Git repository" in answer["reason"]
    assert documented_projects() == []


def test_ActAddProject_MissingFolder_RefusedWithReason(tmp_path, capsys):
    code, answer = run_act(capsys, [ADD_PROJECT, str(tmp_path / "gone")])

    assert code == exits.USAGE
    assert answer["result"] == "refused" and "not a folder" in answer["reason"]


def test_ActAddProject_RelativeFolder_Refused(capsys):
    code, answer = run_act(capsys, [ADD_PROJECT, "relative/path"])

    assert code == exits.USAGE
    assert answer["result"] == "refused" and "absolute" in answer["reason"]


def test_ActAddProject_Twice_SameProjectNoDuplicate(repo, capsys):
    first = run_act(capsys, [ADD_PROJECT, str(repo)])
    second = run_act(capsys, [ADD_PROJECT, str(repo)])

    assert first[0] == second[0] == exits.OK
    assert first[1] == second[1]
    assert [project["folder"] for project in documented_projects()] == [str(repo)]


def test_ActAddProject_ProjectIsListedAsTargetOfStartLane(repo, capsys):
    run_act(capsys, [ADD_PROJECT, str(repo)])
    (project,) = documented_projects()
    spawns = Spawns(stdout="started\n")

    code, answer = run_act(capsys, ["L", project["id"], "--text", "add a parser", "--actor", "me"], run=spawns)

    assert code == exits.OK and answer["result"] == "done"
    (command, kwargs), = spawns.calls
    assert command[1:4] == ["flow", "start", "--detach"] and "--message=add a parser" in command
    assert kwargs["cwd"] == repo


def test_ActStartLane_RegisteredProject_FlowStartedPersonActor(repo, capsys):
    registry.remember(repo)
    (project,) = documented_projects()
    spawns = Spawns(stdout="started\n")

    code, answer = run_act(capsys, ["L", project["id"], "--text", "add a parser", "--actor", "me"], run=spawns)

    assert code == exits.OK and answer["result"] == "done"
    (command, _), = spawns.calls
    assert "--actor=cli:me" in command
    assert actors.parse(next(word for word in command if word.startswith("--actor="))[len("--actor="):]).source == "cli"


def test_ActStartLane_UnknownProject_Refused(repo, capsys):
    registry.remember(repo)
    spawns = Spawns()
    stranger = json.dumps(["project", "/not/registered"], separators=(",", ":"))

    code, answer = run_act(capsys, ["L", stranger, "--text", "add a parser"], run=spawns)

    assert code == exits.USAGE and answer["result"] == "refused" and "no such target" in answer["reason"]
    assert spawns.calls == []


PHONE = gate.Identity("owner@example.com", "nPHONE1", "phone")
SECRET = b"p" * 32


def remote_perform(controller, key, target, **inputs):
    fields = {"key": key, "target": target, **{f"input.{name}": value for name, value in inputs.items()}}
    return controller.perform(fields, overlay_actor(PHONE), proof_field="proof", proof_source=PROOF_SOURCE_OVERLAY)


def remote_controller(repo, spawns, tmp_path):
    shown = make_snapshot(projects=[repo])
    return Controller(SECRET, lambda: shown, lambda: shown, run=spawns, log_path=tmp_path / "actions.jsonl")


def test_ActAddProject_FromRemoteFace_Refused(repo, tmp_path):
    controller = remote_controller(repo, Spawns(), tmp_path)
    plain = tmp_path / "plain"
    plain.mkdir()
    target = api.target_id(host_actions.PROJECTS)

    answers = [remote_perform(controller, "P", target, folder=str(folder))
               for folder in (repo, plain, tmp_path / "gone", "relative")]

    assert all(isinstance(answer, Refused) for answer in answers)
    assert len({answer.reason for answer in answers}) == 1
    assert registry.repos() == []


def test_ActStartLane_FromRemoteFace_Refused(repo, tmp_path):
    spawns = Spawns()
    controller = remote_controller(repo, spawns, tmp_path)
    known = api.target_id(target_of(repo))
    stranger = json.dumps(["project", "/not/registered"], separators=(",", ":"))

    answers = [remote_perform(controller, "L", target, text="add a parser") for target in (known, stranger)]

    assert all(isinstance(answer, Refused) for answer in answers)
    assert answers[0].reason == answers[1].reason
    assert spawns.calls == []


def raising(error):
    def raise_it(*_):
        raise error
    return raise_it


def said(answer):
    return answer.get("reason") or answer["output"]


def test_ActAddProject_Refusal_NoHostPathOrErrorText(repo, tmp_path, capsys, monkeypatch):
    plain = tmp_path / "plain"
    plain.mkdir()
    refusals = [said(run_act(capsys, [ADD_PROJECT, str(folder)])[1])
                for folder in (plain, tmp_path / "gone", "relative/path")]
    with monkeypatch.context() as patched:
        patched.setattr(git_program, "common_dir_of",
                        raising(git_program.GitProgramError(f"{tmp_path} secret detail")))
        refusals.append(said(run_act(capsys, [ADD_PROJECT, str(repo)])[1]))
    with monkeypatch.context() as patched:
        patched.setattr(registry, "remember", raising(registry.RegistryError(f"cannot write {engine_home.root()}")))
        refusals.append(said(run_act(capsys, [ADD_PROJECT, str(repo)])[1]))

    assert len(refusals) == 5 and all(refusals)
    for reason in refusals:
        assert str(tmp_path) not in reason and "secret detail" not in reason and "engine-home" not in reason
        assert "relative" not in reason and "gone" not in reason and "plain" not in reason


def test_ActAddProject_InsideAgentWritableRoot_Refused(tmp_path, capsys):
    held = engine_home.root() / "held"
    held.parent.mkdir(exist_ok=True)
    inside = other_repo(held.parent, "held")
    link = tmp_path / "link"
    link.symlink_to(inside)
    (inside / "docs").mkdir()
    outside = other_repo(tmp_path, "free")

    direct = run_act(capsys, [ADD_PROJECT, str(inside)])
    through_link = run_act(capsys, [ADD_PROJECT, str(link)])
    through_subfolder = run_act(capsys, [ADD_PROJECT, str(link / "docs")])
    allowed = run_act(capsys, [ADD_PROJECT, str(outside)])

    for code, answer in (direct, through_link, through_subfolder):
        assert code == exits.USAGE and answer["result"] == "refused"
        assert str(tmp_path) not in answer["reason"] and "where an agent can write" in answer["reason"]
    assert direct[1] == through_link[1] == through_subfolder[1]
    assert allowed[0] == exits.OK
    assert [project["folder"] for project in documented_projects()] == [str(outside)]


def test_ActAddProject_SecondIndependentRepository_Registered(repo, tmp_path, capsys):
    assert run_act(capsys, [ADD_PROJECT, str(repo)])[0] == exits.OK
    assert [project["folder"] for project in documented_projects()] == [str(repo)]  # the snapshot trusts the first
    second = other_repo(tmp_path, "second")

    code, answer = run_act(capsys, [ADD_PROJECT, str(second)])

    assert code == exits.OK and answer["result"] == "done", answer
    assert sorted(project["folder"] for project in documented_projects()) == sorted([str(repo), str(second)])


def test_ActStartLane_ReturnsTheCreatedLaneId(repo, capsys):
    registry.remember(repo)
    (project,) = documented_projects()
    spawns = Spawns(stdout="task text from the command line\nlane brand-new\nsakura flow approve brand-new spec\n")

    code, answer = run_act(capsys, ["L", project["id"], "--text", "add a parser"], run=spawns)

    assert code == exits.OK and answer == {"result": "done", "exit_code": 0, "output": "brand-new"}


def test_ActStartLane_WithoutALaneLine_ReturnsNoId(repo, capsys):
    registry.remember(repo)
    (project,) = documented_projects()

    code, answer = run_act(capsys, ["L", project["id"], "--text", "add a parser"], run=Spawns(stdout="started\n"))

    assert code == exits.OK and answer["output"] == ""


def test_ActAddProject_CommondirIntoAgentWritableRoot_Refused(tmp_path, capsys):
    held = engine_home.root() / "held"
    held.parent.mkdir(exist_ok=True)
    inside = other_repo(held.parent, "held")
    candidate = tmp_path / "candidate"
    (candidate / ".git").mkdir(parents=True)
    # Outside every agent-writable root, but its Git folder names another repository's as its common folder.
    (candidate / ".git" / "commondir").write_text(f"{inside / '.git'}\n")
    (candidate / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

    code, answer = run_act(capsys, [ADD_PROJECT, str(candidate)])

    assert code == exits.USAGE and answer["result"] == "refused", answer
    assert "where an agent can write" in answer["reason"] and str(tmp_path) not in answer["reason"]
    assert documented_projects() == []


def test_ActAddProject_EmptyGitFolder_Refused(tmp_path, capsys):
    plain = tmp_path / "plain"
    (plain / ".git").mkdir(parents=True)

    code, answer = run_act(capsys, [ADD_PROJECT, str(plain)])

    assert code == exits.USAGE and answer["result"] == "refused", answer
    assert "not a Git repository" in answer["reason"]
    assert documented_projects() == []


def test_ActAddProject_CommondirChain_Refused(tmp_path, capsys):
    """No second pointer is followed or masked: the common folder a `commondir` names holds no `commondir` itself."""
    last = other_repo(tmp_path, "last")
    middle = other_repo(tmp_path, "middle")
    (middle / ".git" / "commondir").write_text(f"{last / '.git'}\n")
    candidate = tmp_path / "candidate"
    (candidate / ".git").mkdir(parents=True)
    (candidate / ".git" / "commondir").write_text(f"{middle / '.git'}\n")
    (candidate / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

    code, answer = run_act(capsys, [ADD_PROJECT, str(candidate)])

    assert code == exits.USAGE and answer["result"] == "refused", answer
    assert str(tmp_path) not in answer["reason"]
    assert documented_projects() == []


def test_ActAddProject_GitFolderWithoutObjectsAndRefs_Refused(tmp_path, capsys):
    hollow = tmp_path / "hollow"
    (hollow / ".git").mkdir(parents=True)
    (hollow / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

    code, answer = run_act(capsys, [ADD_PROJECT, str(hollow)])

    assert code == exits.USAGE and "not a Git repository" in answer["reason"]
    assert documented_projects() == []


def test_ActAddProject_RunsNoGitProcess(repo, capsys, monkeypatch):
    monkeypatch.setattr(git_program, "run", raising(AssertionError("Git was run")))

    code, answer = run_act(capsys, [ADD_PROJECT, str(repo)])

    assert code == exits.OK, answer


def test_ActAddProject_ThroughALink_EveryStepGetsTheCanonicalPath(repo, tmp_path, capsys, monkeypatch):
    link = tmp_path / "link"
    link.symlink_to(repo)
    seen = {"alike": [], "trust": [], "remember": []}
    alike, trust, remember = git_program.common_dir_of, gitutil.trust_repository, registry.remember
    monkeypatch.setattr(git_program, "common_dir_of", lambda path: seen["alike"].append(str(path)) or alike(path))
    monkeypatch.setattr(gitutil, "trust_repository", lambda path: seen["trust"].append(str(path)) or trust(path))
    monkeypatch.setattr(registry, "remember", lambda path, *rest: seen["remember"].append(str(path)) or remember(path, *rest))

    code, answer = run_act(capsys, [ADD_PROJECT, str(link / ".")])

    canonical = os.path.realpath(repo)
    assert code == exits.OK, answer
    assert canonical in seen["alike"] and not [name for name in seen["alike"] if name.endswith("link")]
    assert seen["trust"] == [canonical] and seen["remember"] == [canonical]


def test_linked_registration_keeps_saved_lanes_when_the_alias_disappears(repo, tmp_path):
    alias = tmp_path / "linked"
    git(repo, "worktree", "add", "-qb", "linked", str(alias))
    create_task(repo, "blocked", status="blocked", reason="run failed")
    registry.remember(alias)
    before = api.snapshot_document(cli._collect())
    assert len(before["items"]) == len(before["waits"]) == 1
    git(repo, "worktree", "remove", str(alias))

    for section in ("projects", "items", "waits"):
        assert api.snapshot_document(cli._collect())[section] == before[section]
