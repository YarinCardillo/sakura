"""The engine's own object store: what it proves and delivers comes from a bare repository the engine filled itself,
every object hashed on the way in, never from the object database the agents can write."""

import hashlib
import os
import re
import struct
import subprocess
import zlib
from pathlib import Path

import pytest

from conftest import git
from flow_support import engine_head, make_context, task_workspace
from sakura import engine_home
from sakura.flow import completion, final_head, sealed
from sakura.flow.phases import pull_request

GOOD = b"good\n"
EVIL = b"evil\n"
OBJECT_TYPES = {"commit": 1, "tree": 2, "blob": 3}


def _commit_good(repo):
    (repo / "a.txt").write_bytes(GOOD)
    git(repo, "add", "a.txt")
    git(repo, "commit", "-qm", "the task's change")
    return git(repo, "rev-parse", "HEAD"), git(repo, "rev-parse", "HEAD:a.txt")


def _plant_pack(repo, claimed, content):
    """A pack whose index files `content` under the ID `claimed`: Git reads packs before loose objects and does not
    hash an object it reads for a checkout or a push, so the recorded blob now shows other bytes there."""
    entry = _pack_header(OBJECT_TYPES["blob"], len(content)) + zlib.compress(content)
    body = b"PACK" + struct.pack(">II", 2, 1)
    offset = len(body)
    body += entry
    checksum = hashlib.sha1(body).digest()
    raw = bytes.fromhex(claimed)
    fanout = b"".join(struct.pack(">I", 1 if index >= raw[0] else 0) for index in range(256))
    index = b"\377tOc" + struct.pack(">I", 2) + fanout + raw + struct.pack(">I", zlib.crc32(entry))
    index += struct.pack(">I", offset) + checksum
    folder = _objects(repo) / "pack"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"pack-{checksum.hex()}.pack").write_bytes(body + checksum)
    (folder / f"pack-{checksum.hex()}.idx").write_bytes(index + hashlib.sha1(index).digest())


def _plant_loose(repo, claimed, content):
    """The loose object file of `claimed` rewritten with `content`."""
    path = _objects(repo) / claimed[:2] / claimed[2:]
    path.chmod(0o644)
    path.write_bytes(zlib.compress(b"blob %d\0" % len(content) + content))


def _objects(repo):
    """The object folder of `repo`'s common Git folder (a task repository's, for a workspace of one)."""
    return Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir")) / "objects"


def _pack_header(kind, size):
    header = bytearray()
    byte = (kind << 4) | (size & 0x0F)
    size >>= 4
    while size:
        header.append(byte | 0x80)
        byte, size = size & 0x7F, size >> 7
    header.append(byte)
    return bytes(header)


def _context(repo, overrides=None):
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [41990, 41999]}},
             "branch": "feat/demo", "base_commit": git(repo, "rev-parse", "HEAD"), "contract_sha256": ""}
    return make_context(repo, state=state, overrides=overrides)


def _bare(path):
    subprocess.run(["git", "init", "-q", "--bare", str(path)], check=True, capture_output=True)
    return path


def test_Substitution_ThePlantedObjectReallyShowsOtherBytesInTheWorkspace(repo):
    """The attack the store answers, shown on Git itself: a checkout from the workspace's database gets other bytes."""
    head, blob = _commit_good(repo)
    _plant_pack(repo, blob, EVIL)
    assert git(repo, "cat-file", "blob", blob) == EVIL.decode().strip()


def _in_its_own_repository(repo):
    """A task with its own repository whose workspace 0 holds the good commit; returns the context, workspace 0,
    the commit and its blob."""
    ctx = make_context(repo, own_repository=True, state={"branch": "feat/demo", "contract_sha256": ""})
    zero = task_workspace(ctx)
    head, blob = _commit_good(zero)
    return ctx, zero, head, blob


@pytest.mark.parametrize("plant", [_plant_pack, _plant_loose], ids=["pack", "loose"])
def test_Integration_AnObjectSubstitutedInTheTaskRepository_IsNeverIntegratedCheckedOutOrPushed(repo, tmp_path, plant):
    ctx, zero, head, blob = _in_its_own_repository(repo)
    plant(zero, blob, EVIL)
    remote = _bare(tmp_path / "remote.git")
    with pytest.raises(sealed.SealRefused, match="object store"):
        final_head.merge(ctx, "T1", "the task's change", head)
    state = ctx.store.read_state()
    assert final_head.recorded(state) == state["base_commit"] and not _store_has(repo, head)
    with completion.opened(ctx) as checkout:
        assert checkout.revision == state["base_commit"] and not (checkout.source / "a.txt").exists()
    outcome = pull_request._push(ctx, sealed.path(repo), head, "feat/demo", str(remote))
    assert outcome is not None and outcome.kind == "blocked"
    assert subprocess.run(["git", "-C", str(remote), "rev-parse", "--verify", "refs/heads/feat/demo"],
                          capture_output=True).returncode != 0


@pytest.mark.parametrize("plant", [_plant_pack, _plant_loose], ids=["pack", "loose"])
def test_CompletionCheckout_ASubstitutionPlantedAfterTheSeal_NeverReachesALaterCheckout(repo, plant):
    head, blob = _commit_good(repo)
    ctx = _context(repo)
    engine_head(ctx)
    with completion.opened(ctx):
        pass
    plant(repo, blob, EVIL)
    with completion.opened(ctx) as checkout:
        assert (checkout.source / "a.txt").read_bytes() == GOOD
        assert checkout.revision == head


def test_CompletionCheckout_ItsGitFolderIsTheEnginesStore_WithNoAlternates(repo):
    _commit_good(repo)
    ctx = _context(repo)
    engine_head(ctx)
    store = sealed.path(repo)
    with completion.opened(ctx) as checkout:
        pointer = (checkout.source / ".git").read_text().removeprefix("gitdir:").strip()
        assert Path(pointer).resolve().parent.parent == store.resolve()
    assert engine_home.root().resolve() in store.resolve().parents
    assert not (store / "objects" / "info" / "alternates").exists()
    assert git(store, "rev-parse", "--is-bare-repository") == "true"


@pytest.mark.parametrize("plant", [_plant_pack, _plant_loose], ids=["pack", "loose"])
def test_Delivery_ASubstitutionPlantedAfterTheSeal_DoesNotChangeWhatIsPushed(repo, tmp_path, plant):
    head, blob = _commit_good(repo)
    git(repo, "branch", "feat/demo")
    remote = _bare(tmp_path / "remote.git")
    ctx = _context(repo)
    store = sealed.seal(repo, head)
    plant(repo, blob, EVIL)
    assert pull_request._push(ctx, store, head, "feat/demo", str(remote)) is None
    assert git(remote, "rev-parse", "refs/heads/feat/demo") == head
    assert git(remote, "cat-file", "blob", f"{head}:a.txt") == GOOD.decode().strip()


def test_Seal_ARevisionTheWorkspaceDoesNotHave_IsRefused(repo):
    _context(repo)  # a seal verifies its source against the task's repository, which every engine process trusts
    with pytest.raises(sealed.SealRefused, match="object store"):
        sealed.seal(repo, "0123456789abcdef0123456789abcdef01234567")


def test_Seal_SomethingThatIsNotAFullObjectId_IsRefusedWithoutRunningGit(repo):
    with pytest.raises(sealed.SealRefused, match="full object ID"):
        sealed.seal(repo, "HEAD")


def test_Seal_TheStoreHoldsTheRevisionUnderItsOwnRef_AndTheWorkspaceIsUntouched(repo):
    head, _ = _commit_good(repo)
    _context(repo)
    refs_before = git(repo, "for-each-ref")
    store = sealed.seal(repo, head)
    assert git(store, "rev-parse", f"{sealed.REF_PREFIX}{head}") == head
    assert git(repo, "for-each-ref") == refs_before
    assert os.stat(store).st_mode & 0o777 == 0o700


# -- review 7: the source of a seal cannot borrow objects from another repository ------------------------------------

PRIVATE = b"private-commit-canary-62d0\n"


def _private_repository(tmp_path):
    """Another repository of the same user, with a commit the task must never see."""
    other = tmp_path / "private"
    git(tmp_path, "init", "-q", "-b", "main", str(other))
    git(other, "config", "user.name", "Other")
    git(other, "config", "user.email", "other@example.invalid")
    (other / "secret.txt").write_bytes(PRIVATE)
    git(other, "add", "secret.txt")
    git(other, "commit", "-qm", "private work")
    git(other, "repack", "-adq")  # in a pack, so a linked pack folder alone exposes it
    return other, git(other, "rev-parse", "HEAD")


def _borrow_by_alternates(repo, other, name="alternates"):
    (_objects(repo) / "info").mkdir(exist_ok=True)
    (_objects(repo) / "info" / name).write_text(f"{other / '.git' / 'objects'}\n")


def _borrow_by_link(repo, other, folder):
    """`objects`, `objects/info` or `objects/pack` of the source replaced by a link into the other repository: the
    source keeps its own objects too, copied into the other one first."""
    own = repo / ".git" / folder
    target = other / ".git" / folder
    if folder == "objects":
        subprocess.run(["cp", "-R", f"{own}/.", str(target)], check=True)
    os.rename(own, own.with_name(own.name + "-moved"))
    own.symlink_to(target)


def _store_has(repo, revision):
    store = sealed.path(repo)
    return store.is_dir() and subprocess.run(["git", "-C", str(store), "cat-file", "-e", revision],
                                             capture_output=True).returncode == 0


def _point_branch_at(repo, revision, branch="main"):
    (repo / ".git" / "refs" / "heads" / branch).write_text(revision + "\n")


@pytest.mark.parametrize("name", ["alternates", "http-alternates"])
def test_Seal_ASourceWithAnAlternatesFile_IsRefused_AndThePrivateCommitNeverReachesTheStore(repo, tmp_path, name):
    other, private = _private_repository(tmp_path)
    _context(repo)
    _borrow_by_alternates(repo, other, name)
    if name == "alternates":
        _point_branch_at(repo, private)
    with pytest.raises(sealed.SealRefused, match=name):
        sealed.seal(repo, private)
    assert not _store_has(repo, private)


@pytest.mark.parametrize("folder", ["objects", "objects/info", "objects/pack"])
def test_Seal_ASourceWhoseObjectFoldersAreLinks_IsRefused_AndThePrivateCommitNeverReachesTheStore(repo, tmp_path,
                                                                                                     folder):
    other, private = _private_repository(tmp_path)
    _context(repo)
    _borrow_by_link(repo, other, folder)
    _point_branch_at(repo, private)
    with pytest.raises(sealed.SealRefused, match="plain folder"):
        sealed.seal(repo, private)
    assert not _store_has(repo, private)


@pytest.mark.parametrize("planted", ["commondir", ".git"])
def test_Seal_AGitFolderPointerInTheSourcesCommonFolder_IsRefused(repo, tmp_path, planted):
    other, private = _private_repository(tmp_path)
    _context(repo)
    text = str(other / ".git") if planted == "commondir" else f"gitdir: {other / '.git'}"
    (repo / ".git" / planted).write_text(text + "\n")
    with pytest.raises(sealed.SealRefused, match=re.escape(planted)):
        sealed.seal(repo, private)
    assert not _store_has(repo, private)


def test_Seal_AWorkspaceWhoseGitFileNamesAnotherRepository_IsRefused(repo, tmp_path):
    other, private = _private_repository(tmp_path)
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    _context(repo)
    (lane / ".git").write_text(f"gitdir: {other / '.git'}\n")
    with pytest.raises(sealed.SealRefused):
        sealed.seal(lane, private)
    assert not _store_has(repo, private)


@pytest.mark.parametrize("borrow", ["alternates", "commondir", "objects/pack"])
def test_Seal_TheSourceSideRunsInAProfileThatReadsOnlyTheRepository_EvenPastTheChecks(repo, tmp_path, monkeypatch,
                                                                                    borrow):
    """The checks before the fetch name the indirection; the read-only profile the source side runs in is what
    stops it, also when the checks are skipped (a link made between the check and the fetch)."""
    other, private = _private_repository(tmp_path)
    _context(repo)
    if borrow == "alternates":
        _borrow_by_alternates(repo, other)
    elif borrow == "commondir":
        (repo / ".git" / "commondir").write_text(f"{other / '.git'}\n")
    else:
        _borrow_by_link(repo, other, borrow)
    _point_branch_at(repo, private)
    monkeypatch.setattr(sealed, "_require_plain_source", lambda common: None)
    with pytest.raises(sealed.SealRefused, match="object store"):
        sealed.seal(repo, private)
    assert not _store_has(repo, private)


def test_Seal_TheSourceSideProfile_ReadsTheCommonFolderAndTheSystemOnly(repo):
    head, _ = _commit_good(repo)
    _context(repo)
    sealed.seal(repo, head)
    profile = (engine_home.project_dir(repo) / sealed.SOURCE_PROFILE).read_text()
    assert "(deny file-read*)" in profile and "(deny network*)" in profile
    assert f'(subpath "{os.path.realpath(repo / ".git")}")' in profile
    assert f'(subpath "{os.path.realpath(repo)}")' not in profile
    assert str(engine_home.root()) not in profile and os.path.realpath(engine_home.root()) not in profile


def test_Integration_ACommitBorrowedFromAnotherRepository_IsNeverIntegratedOrPushed(repo, tmp_path):
    other, private = _private_repository(tmp_path)
    ctx, zero, _head, _blob = _in_its_own_repository(repo)
    remote = _bare(tmp_path / "remote.git")
    final_head.proved(ctx)  # the base is in the store before any task is integrated
    _borrow_by_alternates(zero, other)
    with pytest.raises(sealed.SealRefused, match="alternates"):
        final_head.merge(ctx, "T1", "private work", private)
    outcome = pull_request._push(ctx, sealed.path(repo), private, "feat/demo", str(remote))
    assert outcome is not None and outcome.kind == "blocked"
    assert subprocess.run(["git", "-C", str(remote), "cat-file", "-e", private], capture_output=True).returncode != 0
    assert not _store_has(repo, private)
