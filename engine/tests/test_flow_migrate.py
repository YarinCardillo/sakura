"""`sakura flow migrate-state`: agentkit-era reports and logs copied into an archive; nothing that carries authority.

The old `.git` folder was writable by agents, so task state, contracts, calibration, trust records and the old host
folders are never migrated. Symlinks and non-regular files are refused, and a source changed after the plan (size or
digest, the same inode included) is not copied.
"""

import json
import os
import signal
from pathlib import Path

from sakura import engine_home, repository
from sakura.flow import cli, exits, migrate, store, tasks

TASK_LOGS = ("demo/ledger.md", "demo/report.md", "demo/runs/build-1.log", "demo/runs/operations/full-tests.log")
FORGED_STATE = {"task": "demo", "phase": "pr", "status": "running", "approvals": {"checks": {"commands": ["rm -rf /"]},
                                                                                   "spec": {"sha256": "x"}},
                "receipts": {"R1/api": {"exit": 0}}, "evaluator_receipts": {"R1/api": {"exit": 0}},
                "owner_marks": {"R2/api": {"verdict": "ok"}}, "full_green_head": "f" * 40, "contract_sha256": "x"}
FORGED_CONTRACT = {"rows": [{"id": "R1", "passes": True}]}


def _old_layout(repo: Path, home: Path) -> Path:
    common = repository.common_dir(repo)
    old = common / "flow"
    for relative in TASK_LOGS:
        (old / relative).parent.mkdir(parents=True, exist_ok=True)
        (old / relative).write_text(f"{relative}\n")
    (old / "demo" / "state.json").write_text(json.dumps(FORGED_STATE))
    (old / "demo" / "contract.json").write_text(json.dumps(FORGED_CONTRACT))
    (old / "demo" / "rulings.json").write_text("[]\n")
    (old / "demo" / "evidence" / "checks").mkdir(parents=True)
    (old / "demo" / "evidence" / "checks" / "R1-api.log").write_text("exit 0\n")
    (old / "calibration.json").write_text('{"claude:opus": {"task": "demo"}}\n')
    for relative in ("stamps/tree.fast.x", "fingerprints/s1.json", "trusted-hooks.json", "codex-trust.json"):
        path = common / "agentkit" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("{}\n")
    (home / ".config" / "agentkit").mkdir(parents=True)
    (home / ".config" / "agentkit" / "flow.toml").write_text('[checks]\napproved = ["sh"]\n')
    (home / ".local" / "state" / "agentkit" / "flow-digests").mkdir(parents=True)
    return common


def _archive(repo):
    return engine_home.project_dir(repo) / migrate.ARCHIVE_DIR


def test_Migrate_CraftedOldStateWithForgedPasses_ImportsNothingAuthoritative(repo):
    _old_layout(repo, Path.home())
    migrate.apply(migrate.plan(repo, Path.home()))
    project = engine_home.project_dir(repo)
    copied = sorted(str(path.relative_to(project)) for path in project.rglob("*") if path.is_file())
    assert copied == sorted(f"{migrate.ARCHIVE_DIR}/{relative}" for relative in TASK_LOGS)
    assert not store.TaskStore(repo, "demo").exists() and not tasks.known_tasks(repo)
    assert not engine_home.config_file().exists() and not engine_home.kit_dir(repo).exists()
    assert not (Path.home() / ".config" / "sakura").exists() and not (Path.home() / ".local" / "state" / "sakura").exists()


def test_Migrate_EveryEntryThatCouldCarryAuthority_IsListedAsRefused(repo):
    common = _old_layout(repo, Path.home())
    refused = {path: reason for path, reason in migrate.plan(repo, Path.home()).refused}
    for name in ("state.json", "contract.json", "rulings.json", "evidence"):
        assert refused[common / "flow" / "demo" / name] == migrate.AUTHORITY
    assert refused[common / "flow" / "calibration.json"] == migrate.AUTHORITY
    assert {"stamps", "fingerprints", "trusted-hooks.json", "codex-trust.json"} <= {
        path.name for path, reason in refused.items() if reason == migrate.TRUST_RECORD}
    assert refused[Path.home() / ".config" / "agentkit"] == migrate.HOST_FOLDER
    assert refused[Path.home() / ".local" / "state" / "agentkit"] == migrate.HOST_FOLDER


def test_Migrate_ReportsAndLogs_LandInTheArchiveWithTheirContent(repo):
    _old_layout(repo, Path.home())
    found = migrate.plan(repo, Path.home())
    assert len(found.copies) == len(TASK_LOGS)
    assert all(copy.size == len(copy.source.read_bytes()) and copy.sha256 for copy in found.copies)
    assert migrate.apply(found) == []
    for relative in TASK_LOGS:
        assert (_archive(repo) / relative).read_text() == f"{relative}\n"
    assert (repository.common_dir(repo) / "flow" / "demo" / "state.json").is_file(), "nothing old is deleted"


def test_Migrate_SymlinksSpecialAndBinaryLogs_AreRefused(repo, tmp_path):
    common = _old_layout(repo, Path.home())
    outside = tmp_path / "outside.log"
    outside.write_text("forged\n")
    runs = common / "flow" / "demo" / "runs"
    (runs / "linked.log").symlink_to(outside)
    (common / "flow" / "linked-task").symlink_to(tmp_path)
    os.mkfifo(runs / "pipe.log")
    (runs / "binary.log").write_bytes(b"\x89PNG\r\n\x1a\n\0\0")
    os.link(runs / "build-1.log", tmp_path / "second-name.log")
    refused = {path.name: reason for path, reason in migrate.plan(repo, Path.home()).refused}
    assert "symlink" in refused["linked.log"] and "symlink" in refused["linked-task"]
    assert "regular" in refused["pipe.log"] and "regular" in refused["build-1.log"]
    assert "UTF-8" in refused["binary.log"]


def test_Migrate_ASecondRun_CopiesNothingAndKeepsWhatIsThere(repo):
    _old_layout(repo, Path.home())
    migrate.apply(migrate.plan(repo, Path.home()))
    (_archive(repo) / "demo" / "ledger.md").write_text("newer\n")
    again = migrate.plan(repo, Path.home())
    assert again.copies == [] and len(again.kept) == len(TASK_LOGS)
    migrate.apply(again)
    assert (_archive(repo) / "demo" / "ledger.md").read_text() == "newer\n"


def test_Migrate_NothingOld_PlansNothing(repo):
    found = migrate.plan(repo, Path.home())
    assert found.copies == [] and found.kept == [] and found.refused == []


def test_MigrateState_PrintsTheWholePlanBeforeCopying(repo, capsys, monkeypatch):
    _old_layout(repo, Path.home())
    monkeypatch.chdir(repo)
    assert cli.main(["migrate-state"]) == exits.OK
    lines = capsys.readouterr().out.splitlines()
    planned = [index for index, line in enumerate(lines) if line.startswith("will copy ")]
    refused = [index for index, line in enumerate(lines) if line.startswith("refused ")]
    done = [index for index, line in enumerate(lines) if line.startswith("copied ")]
    assert len(planned) == len(TASK_LOGS) and refused and len(done) == 1
    assert max(planned + refused) < done[0]
    assert cli.main(["migrate-state", "--repo", str(repo)]) == exits.OK
    assert capsys.readouterr().out.count("kept ") == len(TASK_LOGS)


def test_MigrateState_InsideAnEngineRole_IsRefused(repo, monkeypatch):
    _old_layout(repo, Path.home())
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["migrate-state"]) == exits.USAGE
    assert not _archive(repo).exists()


# -- a source changed between the plan and the copy ---------------------------------------------------------------

def _planned_source(repo):
    """The plan for the old layout, the source of its ledger and where that file would land."""
    found = migrate.plan(repo, Path.home())
    copy = next(copy for copy in found.copies if copy.source.name == "ledger.md")
    return found, copy.source, copy.target


def _assert_refused_and_not_copied(refused, source, target):
    assert [path for path, _ in refused] == [source] and not target.exists()
    assert not target.parent.exists() or list(target.parent.glob("*.part")) == []


def test_Migrate_ASourceSwappedForASymlinkAfterPlanning_IsNotCopied(repo, tmp_path):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    outside = tmp_path / "outside.md"
    outside.write_text("forged\n")
    source.unlink()
    source.symlink_to(outside)
    _assert_refused_and_not_copied(migrate.apply(found), source, target)


def test_Migrate_ASourceGivenASecondNameAfterPlanning_IsNotCopied(repo, tmp_path):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    os.link(source, tmp_path / "second-name.md")
    _assert_refused_and_not_copied(migrate.apply(found), source, target)


def test_Migrate_ASourceSwappedForAFifoAfterPlanning_IsRefusedWithoutBlocking(repo):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    source.unlink()
    os.mkfifo(source)
    previous = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(AssertionError("the copy blocked")))
    signal.alarm(5)
    try:
        refused = migrate.apply(found)
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
    _assert_refused_and_not_copied(refused, source, target)


def test_Migrate_ASourceReplacedByANewInodeAfterPlanning_IsNotCopied(repo):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    source.unlink()
    source.write_text("demo/ledger.md\n")
    _assert_refused_and_not_copied(migrate.apply(found), source, target)


def test_Migrate_ASourceExtendedInPlaceAfterPlanning_IsNotCopied(repo):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    with source.open("a") as stream:
        stream.write("appended by a writer that kept the file open\n")
    _assert_refused_and_not_copied(migrate.apply(found), source, target)


def test_Migrate_ASourceRewrittenInPlaceWithTheSameSize_IsNotCopied(repo):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    inode = source.stat().st_ino
    with source.open("r+") as stream:
        stream.write("X")
    assert source.stat().st_ino == inode and source.stat().st_size == len("demo/ledger.md\n")
    _assert_refused_and_not_copied(migrate.apply(found), source, target)


def test_Migrate_AnUntouchedSource_IsCopiedThroughATemporaryFilePrivateToTheUser(repo):
    _old_layout(repo, Path.home())
    found, source, target = _planned_source(repo)
    source.chmod(0o644)
    assert migrate.apply(found) == []
    assert target.read_text() == "demo/ledger.md\n" and (target.stat().st_mode & 0o777) == 0o600
    assert list(target.parent.glob("*.part")) == []
