"""The files a lane changed from its base to the state of its worktree, with counts, and one file's patch."""

import json
import os

import pytest

from conftest import git
from flow_support import make_context
from sakura import install
from sakura.flow import changes, cli, exits

BODY = "".join(f"line {number}\n" for number in range(1, 21))


def _lane(repo, task="demo"):
    """A lane whose workspace 0 is the repository itself, with a base that holds five files."""
    for name, content in (("edited.txt", "one\ntwo\n"), ("gone.txt", "bye\n"), ("moved.txt", BODY),
                          ("binary.dat", "x"), ("same.txt", "same\n")):
        (repo / name).write_text(content)
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "base files")
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [41990, 41999]}},
             "branch": f"feat/{task}", "base_commit": git(repo, "rev-parse", "HEAD")}
    return make_context(repo, task=task, state=state)


def _change_everything(repo):
    (repo / "edited.txt").write_text("one\nTWO\nthree\n")
    (repo / "added.txt").write_text("new\nfile\n")
    git(repo, "add", "added.txt")
    git(repo, "rm", "-q", "gone.txt")
    git(repo, "mv", "moved.txt", "renamed.txt")
    (repo / "binary.dat").write_bytes(b"\x00\x01\x02binary")


def _rows(found):
    return {row["path"]: row for row in found["files"]}


def test_Changes_EditedAddedDeletedRenamedBinary_OneRowEachWithCounts(repo):
    ctx = _lane(repo)
    _change_everything(repo)
    found = changes.listing(ctx.store.read_state())
    rows = _rows(found)
    assert found["gone"] is False and found["base"] == ctx.store.read_state()["base_commit"]
    assert (rows["edited.txt"]["status"], rows["edited.txt"]["added"], rows["edited.txt"]["removed"]) == ("modified", 2, 1)
    assert (rows["added.txt"]["status"], rows["added.txt"]["added"], rows["added.txt"]["removed"]) == ("added", 2, 0)
    assert (rows["gone.txt"]["status"], rows["gone.txt"]["added"], rows["gone.txt"]["removed"]) == ("deleted", 0, 1)
    assert (rows["renamed.txt"]["status"], rows["renamed.txt"]["added"], rows["renamed.txt"]["removed"]) == ("renamed", 0, 0)
    assert rows["renamed.txt"]["old_path"] == "moved.txt"
    assert (rows["binary.dat"]["added"], rows["binary.dat"]["removed"]) == (None, None)
    assert "same.txt" not in rows and (found["added"], found["removed"]) == (4, 2)


def test_Changes_TheEnginesInstalledHookFiles_AreNotChangesOfTheLane(repo):
    ctx = _lane(repo)
    for name in (".codex/hooks.json", ".sakura/git-hooks/pre-commit", ".claude/settings.local.json"):
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text("{}\n")
    (repo / "added.txt").write_text("new\n")
    state = ctx.store.read_state()
    assert list(_rows(changes.listing(state))) == ["added.txt"]
    with pytest.raises(changes.ChangesRefused):
        changes.patch(state, ".codex/hooks.json")


def test_Changes_UncommittedEditOfATrackedFile_IsCounted(repo):
    ctx = _lane(repo)
    (repo / "same.txt").write_text("same\nmore\n")
    assert _rows(changes.listing(ctx.store.read_state()))["same.txt"]["added"] == 1


def test_Changes_NewFileNotYetAdded_ListedAsAddedWithItsLines(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("a\nb\nc\n")
    row = _rows(changes.listing(ctx.store.read_state()))["fresh.txt"]
    assert (row["status"], row["added"], row["removed"]) == ("added", 3, 0)


def test_Changes_WorktreeCleanedUp_GoneWithNoFilesAndNoError(repo):
    ctx = _lane(repo)
    ctx.store.update_state(lambda state: state["workspaces"]["0"].update(path=str(repo / "cleaned-up")))
    found = changes.listing(ctx.store.read_state())
    assert found["gone"] is True and found["files"] == [] and (found["added"], found["removed"]) == (0, 0)


def test_Changes_LaneWithoutWorkspace_GoneWithNoFiles(repo):
    ctx = _lane(repo)
    ctx.store.update_state(lambda state: state.update(workspaces={}))
    assert changes.listing(ctx.store.read_state())["gone"] is True


def test_Patch_ChangedFile_ReturnsTheDiffOfThatFileOnly(repo):
    ctx = _lane(repo)
    _change_everything(repo)
    found = changes.patch(ctx.store.read_state(), "edited.txt")
    assert found["path"] == "edited.txt" and found["truncated"] is False
    assert "+TWO" in found["patch"] and "-two" in found["patch"] and "added.txt" not in found["patch"]


def test_Patch_RenamedFile_ShowsTheRename(repo):
    ctx = _lane(repo)
    _change_everything(repo)
    assert "rename from moved.txt" in changes.patch(ctx.store.read_state(), "renamed.txt")["patch"]


def test_Patch_UntrackedFile_ShowsItsLinesAsAdded(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("a\nb\n")
    assert "+a\n+b" in changes.patch(ctx.store.read_state(), "fresh.txt")["patch"]


def test_Patch_UntrackedFile_HasTheHunkHeaderOfAnAddedFile(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("a\nb\n")
    assert changes.patch(ctx.store.read_state(), "fresh.txt")["patch"] == (
        "--- /dev/null\n+++ b/fresh.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n")


def test_Patch_UntrackedFileOfOneLine_HasTheShortHunkHeader(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("only\n")
    assert changes.patch(ctx.store.read_state(), "fresh.txt")["patch"].endswith("@@ -0,0 +1 @@\n+only\n")


def test_Patch_UntrackedFileWithoutAFinalNewline_SaysSo(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("a\nb")
    assert changes.patch(ctx.store.read_state(), "fresh.txt")["patch"].endswith(
        "@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n")


def test_Patch_EmptyUntrackedFile_HasNoHunk(repo):
    ctx = _lane(repo)
    (repo / "fresh.txt").write_text("")
    assert "@@" not in changes.patch(ctx.store.read_state(), "fresh.txt")["patch"]


def test_Patch_BinaryUntrackedFile_SaysBinaryNotItsBytes(repo):
    ctx = _lane(repo)
    (repo / "fresh.dat").write_bytes(b"\x00\x01\x02binary")
    assert changes.patch(ctx.store.read_state(), "fresh.dat")["patch"] == "Binary files /dev/null and b/fresh.dat differ\n"


def test_Patch_AddedFileGitTracks_HasAHunkHeaderToo(repo):
    ctx = _lane(repo)
    (repo / "tracked.txt").write_text("a\nb\n")
    git(repo, "add", "tracked.txt")
    assert "@@ -0,0 +1,2 @@\n+a\n+b" in changes.patch(ctx.store.read_state(), "tracked.txt")["patch"]


def test_Patch_OverTheCap_CutWithTruncatedTrue(repo):
    ctx = _lane(repo)
    (repo / "same.txt").write_text("".join(f"{number} some line of text\n" for number in range(60000)))
    found = changes.patch(ctx.store.read_state(), "same.txt")
    assert found["truncated"] is True and len(found["patch"].encode()) <= changes.MAX_PATCH_BYTES


def test_Patch_TerminalControlsInTheFile_Removed(repo):
    ctx = _lane(repo)
    (repo / "same.txt").write_text("same\n\x1b]0;title\x07\x1b[31mred\n")
    patch = changes.patch(ctx.store.read_state(), "same.txt")["patch"]
    assert "\x1b" not in patch and "\x07" not in patch and "red" in patch


@pytest.mark.parametrize("path", ["../x", "a/../../x", "/etc/passwd", "", "same.txt/../..", "ok\x00bad", ".", "unchanged-name.txt"])
def test_Patch_PathOutsideOrNotAChangedFile_Refused(repo, path):
    ctx = _lane(repo)
    (repo / "same.txt").write_text("changed\n")
    with pytest.raises(changes.ChangesRefused):
        changes.patch(ctx.store.read_state(), path)


def test_Patch_DirectoryLinkLeavingTheWorktree_Refused(repo, tmp_path):
    ctx = _lane(repo)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.txt").write_text("secret\n")
    os.symlink(outside, repo / "linked")
    with pytest.raises(changes.ChangesRefused):
        changes.patch(ctx.store.read_state(), "linked/secret.txt")


def test_Patch_SymlinkLeavingTheWorktree_ShowsTheLinkNotTheTarget(repo, tmp_path):
    ctx = _lane(repo)
    (tmp_path / "secret.txt").write_text("TOPSECRET\n")
    os.symlink(tmp_path / "secret.txt", repo / "peek")
    git(repo, "add", "peek")
    assert "TOPSECRET" not in changes.patch(ctx.store.read_state(), "peek")["patch"]


def test_Patch_WorktreeCleanedUp_Refused(repo):
    ctx = _lane(repo)
    ctx.store.update_state(lambda state: state["workspaces"]["0"].update(path=str(repo / "cleaned-up")))
    with pytest.raises(changes.ChangesRefused):
        changes.patch(ctx.store.read_state(), "same.txt")


def _operate(repo, monkeypatch):
    """Run the next commands as the person at the repository; installed after the lane's base is committed."""
    monkeypatch.chdir(repo)
    install.install(repo)


def test_FlowChanges_Json_ListsFilesAndPrintsOnePatchWithFile(repo, monkeypatch, capsys):
    _lane(repo)
    _change_everything(repo)
    _operate(repo, monkeypatch)
    assert cli.main(["changes", "demo", "--json"]) == exits.OK
    document = json.loads(capsys.readouterr().out)
    assert document["task"] == "demo" and "edited.txt" in {row["path"] for row in document["files"]}
    assert cli.main(["changes", "demo", "--file", "edited.txt", "--json"]) == exits.OK
    assert "+TWO" in json.loads(capsys.readouterr().out)["patch"]


def test_FlowChanges_BadFileOrUnknownTask_RefusedWithAFixedSentence(repo, monkeypatch, capsys):
    _lane(repo)
    _operate(repo, monkeypatch)
    assert cli.main(["changes", "demo", "--file", "../x", "--json"]) == exits.USAGE
    refusal = capsys.readouterr()
    assert refusal.out.strip() == "that file is not one of this lane's changed files"
    assert "../x" not in refusal.out
    assert cli.main(["changes", "../demo", "--json"]) == exits.USAGE
    assert cli.main(["changes", "nope", "--json"]) == exits.USAGE
    capsys.readouterr()
