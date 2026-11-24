"""Files in folders caged code can write: the engine reads and writes them without following a link."""

import os
import signal

import pytest

from conftest import git
from sakura import beneath
from sakura.flow import flowfiles
from sakura.flow.phases import evaluate


@pytest.fixture
def outside(tmp_path):
    folder = tmp_path / "outside"
    folder.mkdir()
    (folder / "secret.txt").write_text("person's secret\n")
    return folder


def test_Beneath_ALinkedFileOrFolder_IsNeverFollowed(tmp_path, outside):
    root = tmp_path / "workspace"
    root.mkdir()
    (root / "file.txt").symlink_to(outside / "secret.txt")
    (root / "tests").symlink_to(outside)
    for relative in ("file.txt", "tests/secret.txt"):
        with pytest.raises(OSError):
            beneath.read_bytes(root, relative)
    with pytest.raises(OSError):
        beneath.write_bytes(root, "tests/secret.txt", b"overwritten\n", 0o644)
    beneath.write_bytes(root, "file.txt", b"replaced\n", 0o644)  # the link itself is replaced, never written through
    assert not (root / "file.txt").is_symlink() and (root / "file.txt").read_bytes() == b"replaced\n"
    assert (outside / "secret.txt").read_text() == "person's secret\n"


def test_Beneath_AFifo_IsRefusedWithoutBlocking(tmp_path):
    os.mkfifo(tmp_path / "pipe")
    previous = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(AssertionError("the read blocked")))
    signal.alarm(5)
    try:
        with pytest.raises(beneath.NotRegular):
            beneath.read_bytes(tmp_path, "pipe")
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)


def test_Beneath_WriteCreatesMissingFoldersAndSetsTheMode(tmp_path):
    beneath.write_bytes(tmp_path, "a/b/run.sh", b"echo\n", 0o755)
    assert (tmp_path / "a" / "b" / "run.sh").read_bytes() == b"echo\n" and beneath.is_executable(tmp_path, "a/b/run.sh")
    with pytest.raises(ValueError):
        beneath.read_bytes(tmp_path, "../x")


def test_WorkspaceSnapshot_ALink_IsRecordedByItsTargetNotFollowed(repo, outside):
    (repo / "linked.txt").symlink_to(outside / "secret.txt")
    (repo / "plain.txt").write_text("plain\n")
    snapshot = flowfiles.workspace_snapshot(repo)
    assert snapshot["linked.txt"] == f"symlink:{outside / 'secret.txt'}" and len(snapshot["plain.txt"]) == 64


def test_CalibrationSync_APlantedLinkInTheBaseWorkspace_IsNotWrittenThrough(repo, tmp_path, outside):
    from flow_support import engine_head, make_context

    base = tmp_path / "base"
    git(repo, "worktree", "add", "-q", "--detach", str(base))
    (repo / "tests").mkdir()
    (repo / "tests" / "test_new.py").write_text("def test_new():\n    pass\n")
    git(repo, "add", "tests")
    git(repo, "commit", "-qm", "test")
    (base / "tests").symlink_to(outside)
    ctx = make_context(repo, state={"base_commit": git(base, "rev-parse", "HEAD"),
                                    "workspaces": {"0": {"path": str(repo)}}})
    engine_head(ctx)  # the flows are copied from the engine's head, read from its store
    with pytest.raises(OSError):
        evaluate._sync_contract_flows(ctx, base)
    assert sorted(path.name for path in outside.iterdir()) == ["secret.txt"]


def test_Beneath_WriteOverAHardlinkToAnOutsideFile_LeavesTheOutsideFileUntouched(tmp_path, outside):
    root = tmp_path / "workspace"
    root.mkdir()
    os.link(outside / "secret.txt", root / "test_x.py")
    mode = (outside / "secret.txt").stat().st_mode
    beneath.write_bytes(root, "test_x.py", b"new\n", 0o755)
    assert (outside / "secret.txt").read_text() == "person's secret\n"
    assert (outside / "secret.txt").stat().st_mode == mode
    assert (root / "test_x.py").read_bytes() == b"new\n" and (root / "test_x.py").stat().st_nlink == 1
    assert [path.name for path in root.iterdir()] == ["test_x.py"], "no temporary file is left"


def test_Beneath_ReadOfAFileWithASecondName_IsRefused(tmp_path, outside):
    os.link(outside / "secret.txt", tmp_path / "second.txt")
    with pytest.raises(beneath.NotRegular):
        beneath.read_bytes(tmp_path, "second.txt")


# -- a capped, exact-size, identity-checked read (review 6, finding 5) -------------------------------------------------


def _during_the_read(monkeypatch, action):
    """Run `action` once, right before the first read of the file under test."""
    real = os.read
    done = []

    def read(descriptor, size):
        if not done:
            done.append(True)
            action()
        return real(descriptor, size)

    monkeypatch.setattr(os, "read", read)


def test_ReadCapped_AFileWithinTheLimit_IsReturnedWhole(tmp_path):
    (tmp_path / "task.md").write_bytes(b"x" * 100)
    assert beneath.read_capped(tmp_path, "task.md", 100) == b"x" * 100


def test_ReadCapped_AFileOverTheLimit_IsRefusedBeforeAnyRead(tmp_path, monkeypatch):
    (tmp_path / "task.md").write_bytes(b"x" * 101)
    monkeypatch.setattr(os, "read", lambda *_: (_ for _ in ()).throw(AssertionError("read an oversized file")))
    with pytest.raises(beneath.FileTooLarge, match="101 bytes"):
        beneath.read_capped(tmp_path, "task.md", 100)


def test_ReadCapped_ASparseFileOfManyGigabytes_IsRefusedByItsSize(tmp_path):
    with open(tmp_path / "task.md", "wb") as sparse:
        sparse.truncate(50 << 30)
    with pytest.raises(beneath.FileTooLarge):
        beneath.read_capped(tmp_path, "task.md", 1 << 20)


def test_ReadCapped_AFileThatGrowsDuringTheRead_IsRefused(tmp_path, monkeypatch):
    path = tmp_path / "task.md"
    path.write_bytes(b"x" * 10)
    _during_the_read(monkeypatch, lambda: path.open("ab").write(b"more"))
    with pytest.raises(beneath.FileChanged, match="changed while it was read"):
        beneath.read_capped(tmp_path, "task.md", 100)


def test_ReadCapped_AFileRewrittenInPlaceDuringTheRead_IsRefused(tmp_path, monkeypatch):
    path = tmp_path / "task.md"
    path.write_bytes(b"x" * 10)
    _during_the_read(monkeypatch, lambda: path.open("r+b").write(b"y" * 10))
    with pytest.raises(beneath.FileChanged):
        beneath.read_capped(tmp_path, "task.md", 100)


def test_ReadCapped_AFileSwappedForAnotherDuringTheRead_IsRefused(tmp_path, monkeypatch):
    path = tmp_path / "task.md"
    path.write_bytes(b"x" * 10)
    (tmp_path / "other").write_bytes(b"y" * 10)
    _during_the_read(monkeypatch, lambda: os.rename(tmp_path / "other", path))
    with pytest.raises(beneath.FileChanged):
        beneath.read_capped(tmp_path, "task.md", 100)


def test_ReadCapped_AFileTruncatedDuringTheRead_IsRefused(tmp_path, monkeypatch):
    path = tmp_path / "task.md"
    path.write_bytes(b"x" * 10)
    _during_the_read(monkeypatch, lambda: os.truncate(path, 3))
    with pytest.raises(beneath.FileChanged):
        beneath.read_capped(tmp_path, "task.md", 100)


def test_ReadCapped_KeepsTheLinkAndSingleNameRefusals(tmp_path, outside):
    (tmp_path / "link").symlink_to(outside / "secret.txt")
    with pytest.raises(OSError):
        beneath.read_capped(tmp_path, "link", 100)
    os.link(outside / "secret.txt", tmp_path / "second")
    with pytest.raises(beneath.NotRegular):
        beneath.read_capped(tmp_path, "second", 100)
