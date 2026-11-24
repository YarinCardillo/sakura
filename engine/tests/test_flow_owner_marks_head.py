"""Review 17, finding 7: an owner mark counts only at the engine's head, so waits and marking agree with delivery."""

import pytest

from conftest import git
from sakura.flow import cli, exits, owner_marks, store, waits
from test_flow_owner_continue import ACTOR, owner_task
from test_flow_provers import STEPS_ON


def _move_workspace_head(repo):
    git(repo, "commit", "-q", "--allow-empty", "-m", "survivor")


def test_Mark_WorkspaceHeadDiffersFromTheEnginesHead_IsRefusedAndRecordsNothing(repo, capsys, monkeypatch):
    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    _move_workspace_head(repo)

    assert cli.main(["mark", "demo", "R1", "ok", f"--actor={ACTOR}"]) == exits.USAGE

    assert "engine's head" in capsys.readouterr().out
    state = task.read_state()
    assert owner_marks.MARKS_KEY not in state
    assert not any(row.passes for row in owner_marks.contract.load(task, state["contract_sha256"]))


def test_Mark_WorkspaceHeadDiffersFromTheEnginesHead_RefusalNamesTheTasksRestoreCommand(repo, capsys, monkeypatch):
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    _move_workspace_head(repo)

    assert cli.main(["mark", "demo", "R1", "ok", f"--actor={ACTOR}"]) == exits.USAGE

    out = capsys.readouterr().out
    assert "sakura flow restore-head demo" in out
    assert "{" not in out


def test_Waits_AMarkGivenAtAnotherHeadThanTheEnginesHead_LeavesTheMarkActionOpen(repo):
    task = owner_task(repo, ("R1",))
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    assert "owner_row" not in [wait.kind for wait in waits.collect([repo])]
    # The engine's head moves on (a later fix is integrated): the mark no longer names it.
    moved = git(repo, "rev-parse", "HEAD")
    task.update_state(lambda state: state.update(base_commit="0" * 40))
    assert moved != "0" * 40

    found = waits.collect([repo])

    assert [(wait.kind, wait.title) for wait in found if wait.kind == "owner_row"] == [("owner_row", "Try it yourself: look at R1")]
    assert owner_marks.all_marked(task, task.read_state(), STEPS_ON) is False


def test_OpenRows_AMarkForTheEnginesHead_IsNotOpen(repo):
    task = owner_task(repo, ("R1", "R2"))
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    state = task.read_state()
    rows = owner_marks.contract.load(task, state["contract_sha256"])

    assert [row.id for row in owner_marks.open_rows(rows, STEPS_ON, state)] == ["R2"]


# -- review 18, finding 4: a way out from the cockpit when the mark is refused for a moved head -------------------


def test_RestoreHead_WorkspaceMovedFromHToM_PutsItBackAndTheMarkWorksAgain(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    engine_head = git(repo, "rev-parse", "HEAD")
    _move_workspace_head(repo)
    assert cli.main(["mark", "demo", "R1", "ok", f"--actor={ACTOR}"]) == exits.USAGE

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.OK

    assert git(repo, "rev-parse", "HEAD") == engine_head
    assert "engine's head" in capsys.readouterr().out
    assert cli.main(["mark", "demo", "R1", "ok", f"--actor={ACTOR}"]) == exits.OK
    fresh = store.TaskStore(repo, "demo")
    assert owner_marks.all_marked(fresh, fresh.read_state(), STEPS_ON)


def test_RestoreHead_WorkspaceAlreadyOnTheEnginesHead_ChangesNothing(repo, monkeypatch):
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    before = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.OK

    assert git(repo, "rev-parse", "HEAD") == before


def test_Waits_OwnerRowWithAMovedWorkspaceHead_OffersTheRestoreAndAnUnmovedOneDoesNot(repo):
    from sakura.cockpit import actions
    from sakura.cockpit.wait_actions import target_of

    owner_task(repo, ("R1",))

    def keys():
        return [[action.key for action in actions.applicable(target_of(wait))]
                for wait in waits.collect([repo]) if wait.kind == "owner_row"]

    assert keys() == [["k", "read", "unread", "snooze"]]

    _move_workspace_head(repo)

    assert keys() == [["k", "H", "read", "unread", "snooze"]]
    wait = next(wait for wait in waits.collect([repo]) if wait.kind == "owner_row")
    invocation = actions.plan("H", target_of(wait), {}, actors_for_tests())
    assert invocation.argv[1:3] == ["flow", "restore-head"] and invocation.argv[-2:] == ["--", "demo"]


def actors_for_tests():
    from sakura import actors

    return actors.Actor("overlay", "owner@example.com", "nPHONE1", "phone")


# -- review 19: the restore asks first, rechecks what the person saw under the lock, and keeps ignored files --------


def _restore_arguments(repo, task, workspace_head, engine_head):
    revision = waits.owner_revision(task.read_state(), "R1")
    return ["restore-head", "demo", f"--expect-workspace={workspace_head}", f"--expect-head={engine_head}",
            "--row=R1", f"--revision={revision}", f"--actor={ACTOR}"]


def test_RestoreHead_AnOldFormAfterAnotherDeviceRestoredAndThePersonCommitted_IsRefusedAndMovesNothing(
        repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    engine_head = git(repo, "rev-parse", "HEAD")
    _move_workspace_head(repo)
    moved = git(repo, "rev-parse", "HEAD")
    arguments = _restore_arguments(repo, task, moved, engine_head)
    assert cli.main(arguments) == exits.OK  # another device restores with the same form
    git(repo, "commit", "-q", "--allow-empty", "-m", "the person's own work P")
    work = git(repo, "rev-parse", "HEAD")

    assert cli.main(arguments) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out
    assert git(repo, "rev-parse", "HEAD") == work and git(repo, "rev-parse", "--abbrev-ref", "HEAD") == "main"


def test_RestoreHead_TheEnginesHeadMovedSinceTheFormWasDrawn_IsRefused(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    _move_workspace_head(repo)
    arguments = _restore_arguments(repo, task, git(repo, "rev-parse", "HEAD"), "0" * 40)

    assert cli.main(arguments) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out


def test_RestoreHead_AnIgnoredPersonalFileTheEnginesHeadTracks_IsKeptAndTheRestoreRefused(repo, monkeypatch, capsys):
    (repo / "notes.txt").write_text("tracked at the engine's head\n")
    git(repo, "add", "notes.txt")
    git(repo, "commit", "-qm", "track notes")
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    git(repo, "rm", "-q", "notes.txt")
    (repo / ".gitignore").write_text("notes.txt\n")
    git(repo, "add", ".gitignore")
    git(repo, "commit", "-qm", "survivor: untrack and ignore notes")
    (repo / "notes.txt").write_text("my own work\n")
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert (repo / "notes.txt").read_text() == "my own work\n"
    assert git(repo, "rev-parse", "HEAD") == moved and "cannot restore" in capsys.readouterr().out


def test_RestoreHead_ThePathComesBackAsARename_TheIgnoredPersonalFileIsKept(repo, monkeypatch, capsys):
    # Git's rename detection would report the engine's head adding notes.txt as a rename of other.txt, not an add.
    (repo / "notes.txt").write_text("tracked at the engine's head\n")
    git(repo, "add", "notes.txt")
    git(repo, "commit", "-qm", "track notes")
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    git(repo, "mv", "notes.txt", "other.txt")
    (repo / ".gitignore").write_text("notes.txt\n")
    git(repo, "add", ".gitignore")
    git(repo, "commit", "-qm", "survivor: rename and ignore notes")
    (repo / "notes.txt").write_text("my own work\n")
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert (repo / "notes.txt").read_text() == "my own work\n"
    assert git(repo, "rev-parse", "HEAD") == moved and "cannot restore" in capsys.readouterr().out


def test_RestoreHead_AnUntrackedFileWhereTheEnginesHeadNeedsAFolder_IsKeptAndTheRestoreRefused(repo, monkeypatch, capsys):
    (repo / "docs").mkdir()
    (repo / "docs" / "guide.md").write_text("tracked at the engine's head\n")
    git(repo, "add", "docs/guide.md")
    git(repo, "commit", "-qm", "track docs")
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    git(repo, "rm", "-q", "-r", "docs")
    git(repo, "commit", "-qm", "survivor: drop docs")
    (repo / "docs").write_text("my own file named docs\n")
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert (repo / "docs").read_text() == "my own file named docs\n"
    assert git(repo, "rev-parse", "HEAD") == moved and "cannot restore" in capsys.readouterr().out


def test_RestoreHead_ALinkWhereTheEnginesHeadNeedsAFolder_IsKeptAndTheRestoreRefused(repo, monkeypatch, capsys, tmp_path):
    (repo / "docs").mkdir()
    (repo / "docs" / "guide.md").write_text("tracked at the engine's head\n")
    git(repo, "add", "docs/guide.md")
    git(repo, "commit", "-qm", "track docs")
    monkeypatch.chdir(repo)
    owner_task(repo, ("R1",))
    git(repo, "rm", "-q", "-r", "docs")
    git(repo, "commit", "-qm", "survivor: drop docs")
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (repo / "docs").symlink_to(elsewhere)
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert list(elsewhere.iterdir()) == [] and (repo / "docs").is_symlink()
    assert git(repo, "rev-parse", "HEAD") == moved and "cannot restore" in capsys.readouterr().out


def test_RestoreHead_ThePersonCommitsPBetweenTheCheckAndTheMove_TheBranchStaysOnP(repo, monkeypatch, capsys):
    # Review 20: the check passes for M, then P is committed in the workspace, which takes no task lock. The branch
    # move is conditional on the confirmed old value, so it fails and nothing moves.
    from sakura.flow import final_head

    monkeypatch.chdir(repo)
    task = owner_task(repo, ("R1",))
    engine_head = git(repo, "rev-parse", "HEAD")
    _move_workspace_head(repo)
    moved = git(repo, "rev-parse", "HEAD")
    arguments = _restore_arguments(repo, task, moved, engine_head)
    real_publish = final_head.publish

    def publish_after_the_person_commits(ctx, head):
        git(repo, "commit", "-q", "--allow-empty", "-m", "the person's own work P")
        real_publish(ctx, head)

    monkeypatch.setattr(final_head, "publish", publish_after_the_person_commits)
    work_after = None

    assert cli.main(arguments) == exits.USAGE

    assert "no longer waiting" in capsys.readouterr().out
    assert git(repo, "log", "-1", "--format=%s") == "the person's own work P"
    assert git(repo, "rev-parse", "--abbrev-ref", "HEAD") == "main"


# -- D6: the state a blocked contract left, the head's own file as an uncommitted edit over the flows commit ------------------


def _head_file_edited_over_an_older_commit(repo, local_bytes):
    """The engine's head H adds flow.py; workspace 0 is on M (flow.py with older bytes, committed) with flow.py edited."""
    (repo / "flow.py").write_text("head bytes\n")
    git(repo, "add", "flow.py")
    git(repo, "commit", "-qm", "the engine's flows commit")
    task = owner_task(repo, ("R1",))
    head = git(repo, "rev-parse", "HEAD")
    git(repo, "checkout", "-q", "-B", "main", f"{head}~1")
    (repo / "flow.py").write_text("old bytes\n")
    git(repo, "add", "flow.py")
    git(repo, "commit", "-qm", "an earlier flows commit")
    (repo / "flow.py").write_text(local_bytes)
    return task, head


def test_RestoreHead_TheHeadsFileAsAnUncommittedEdit_IsAdoptedAndTheWorkspaceFollowsTheHead(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    _, head = _head_file_edited_over_an_older_commit(repo, "head bytes\n")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.OK

    assert git(repo, "rev-parse", "HEAD") == head and git(repo, "status", "--porcelain") == ""
    assert (repo / "flow.py").read_text() == "head bytes\n"


def test_RestoreHead_AnEditThatIsNotTheHeadsBytes_IsStillNeverOverwritten(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    _head_file_edited_over_an_older_commit(repo, "the person's own edit\n")
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert (repo / "flow.py").read_text() == "the person's own edit\n"
    assert git(repo, "rev-parse", "HEAD") == moved and "cannot restore" in capsys.readouterr().out


# -- review 25, finding 5: a restore never replaces staged work and keeps the original index when it fails -------------------


def test_RestoreHead_AFileWithStagedContent_IsNotAdoptedAndTheStagedContentStays(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    _head_file_edited_over_an_older_commit(repo, "head bytes\n")  # the work tree holds the head's bytes
    (repo / "flow.py").write_text("staged by the person\n")
    git(repo, "add", "flow.py")
    (repo / "flow.py").write_text("head bytes\n")
    moved = git(repo, "rev-parse", "HEAD")

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert git(repo, "show", ":flow.py") == "staged by the person"
    assert git(repo, "rev-parse", "HEAD") == moved and "staged" in capsys.readouterr().out


def test_RestoreHead_ARestoreThatFailsAfterAdopting_PutsTheOriginalIndexBack(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    (repo / "a.py").write_text("head a\n")
    (repo / "b.py").write_text("head b\n")
    git(repo, "add", "a.py", "b.py")
    git(repo, "commit", "-qm", "the engine's head")
    owner_task(repo, ("R1",))
    head = git(repo, "rev-parse", "HEAD")
    git(repo, "checkout", "-q", "-B", "main", f"{head}~1")
    for name in ("a.py", "b.py"):
        (repo / name).write_text("old\n")
    git(repo, "add", "a.py", "b.py")
    git(repo, "commit", "-qm", "earlier")
    moved = git(repo, "rev-parse", "HEAD")
    (repo / "a.py").write_text("head a\n")  # adoptable
    (repo / "b.py").write_text("the person's own edit\n")  # refused later, by the work tree update

    assert cli.main(["restore-head", "demo", f"--actor={ACTOR}"]) == exits.USAGE

    assert git(repo, "diff", "--cached", "--name-only") == "" and git(repo, "rev-parse", "HEAD") == moved
    assert (repo / "a.py").read_text() == "head a\n" and (repo / "b.py").read_text() == "the person's own edit\n"
