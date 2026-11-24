from pathlib import Path

from sakura.accounts.registry import Account, Registry
from sakura.quota import targets


def account(provider, account_id, directory, enabled=True):
    return Account(provider, account_id, account_id.title(), directory, enabled, None, (), ())


def orca_profile(home: Path, name: str, marker=True) -> Path:
    profile = home / "Library/Application Support/orca/claude-profiles" / name / "home"
    profile.mkdir(parents=True)
    if marker:
        (profile / ".claude.json").write_text("{}")
    return profile


def test_Discover_RegistryAccounts_AreRegisteredTargetsWithTheirKeys(tmp_path):
    claude, codex = tmp_path / "c", tmp_path / "x"
    claude.mkdir(), codex.mkdir()
    registry = Registry((account("claude", "work", claude), account("codex", "work", codex)), False)
    found = targets.discover(registry, tmp_path)
    assert [(t.key, t.registered) for t in found] == [("work", True), ("codex:work", True)]


def test_Discover_DisabledOrMissingDirectory_IsSkipped(tmp_path):
    present = tmp_path / "p"
    present.mkdir()
    registry = Registry((account("claude", "off", present, enabled=False),
                         account("claude", "gone", tmp_path / "nope")), False)
    assert targets.discover(registry, tmp_path) == []


def test_Discover_OrcaProfile_IsAnUnregisteredClaudeTarget(tmp_path):
    profile = orca_profile(tmp_path, "1ace90f2-4d86")
    found = targets.discover(Registry((), True), tmp_path)
    assert [(t.id, t.registered, t.config_dir) for t in found] == [("orca-1ace90f2", False, profile)]


def test_Discover_OrcaProfileWithoutClaudeMarker_IsIgnored(tmp_path):
    orca_profile(tmp_path, "abc", marker=False)
    assert targets.discover(Registry((), True), tmp_path) == []


def test_Discover_OrcaProfileThatIsARegisteredDirectory_IsNotListedTwice(tmp_path):
    profile = orca_profile(tmp_path, "abc")
    link = tmp_path / ".claude-work"
    link.symlink_to(profile)
    found = targets.discover(Registry((account("claude", "work", link),), False), tmp_path)
    assert [t.id for t in found] == ["work"]


def test_Discover_ExtraDirectory_IsAddedOnce(tmp_path):
    extra = tmp_path / "extra"
    extra.mkdir()
    (extra / ".claude.json").write_text("{}")
    found = targets.discover(Registry((), True), tmp_path, [extra, extra])
    assert len(found) == 1 and found[0].config_dir == extra
