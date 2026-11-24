from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura.accounts import providers, registry
from sakura.accounts.registry import AccountsConfigError


def write_registry(home: Path, text: str) -> Path:
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def load(home: Path) -> registry.Registry:
    return registry.load(env={"HOME": str(home)})


def test_load_reads_two_providers_with_the_same_id(tmp_path):
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "work"
config_dir = "~/.work-claude"
reserve_week_pct = 12.5

[[accounts]]
provider = "codex"
id = "work"
enabled = false
""")
    loaded = load(tmp_path)
    claude, codex = loaded.accounts
    assert loaded.implicit is False
    assert (claude.provider, claude.id, claude.config_dir) == ("claude", "work", tmp_path / ".work-claude")
    assert claude.enabled is True and claude.reserve_week_pct == 12.5
    assert (codex.provider, codex.id, codex.config_dir) == ("codex", "work", tmp_path / ".codex")
    assert codex.enabled is False and codex.reserve_week_pct is None


def test_missing_file_is_an_implicit_empty_registry(tmp_path):
    assert load(tmp_path) == registry.Registry(accounts=(), implicit=True)


@pytest.mark.parametrize("bad", ["Work", "-work", "work_1", "work space", "", "a" * 41, "../x"])
def test_id_must_be_a_slug(tmp_path, bad):
    write_registry(tmp_path, f'[[accounts]]\nprovider = "claude"\nid = "{bad}"\n')
    with pytest.raises(AccountsConfigError, match="slug"):
        load(tmp_path)


def test_duplicate_provider_and_id_is_refused(tmp_path):
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "work"
config_dir = "~/.a"

[[accounts]]
provider = "claude"
id = "work"
config_dir = "~/.b"
""")
    with pytest.raises(AccountsConfigError, match="duplicate.*claude.*work"):
        load(tmp_path)


def test_two_accounts_on_one_config_dir_are_refused(tmp_path):
    (tmp_path / "real").mkdir()
    (tmp_path / "link").symlink_to(tmp_path / "real")
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "one"
config_dir = "~/real"

[[accounts]]
provider = "claude"
id = "two"
config_dir = "~/link"
""")
    with pytest.raises(AccountsConfigError, match="one.*two|two.*one"):
        load(tmp_path)


def test_same_config_dir_under_two_providers_is_allowed(tmp_path):
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "one"
config_dir = "~/shared"

[[accounts]]
provider = "codex"
id = "one"
config_dir = "~/shared"
""")
    assert len(load(tmp_path).accounts) == 2


def test_unknown_provider_is_refused(tmp_path):
    write_registry(tmp_path, '[[accounts]]\nprovider = "gemini"\nid = "work"\n')
    with pytest.raises(AccountsConfigError, match="provider.*gemini"):
        load(tmp_path)


def test_malformed_toml_names_the_file(tmp_path):
    path = write_registry(tmp_path, "[[accounts]\n")
    with pytest.raises(AccountsConfigError, match="accounts.toml"):
        registry.load(path, env={"HOME": str(tmp_path)})


def test_label_defaults_to_id(tmp_path):
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "work"

[[accounts]]
provider = "codex"
id = "work"
label = "Work (Codex)"
""")
    claude, codex = load(tmp_path).accounts
    assert (claude.label, codex.label) == ("work", "Work (Codex)")


def test_plans_are_sorted_and_scoped_to_their_account(tmp_path):
    write_registry(tmp_path, """
[[accounts]]
provider = "claude"
id = "work"

[[accounts.plans]]
plan = "max20"
since = 2026-03-01

[[accounts.plans]]
plan = "pro"
since = 2026-01-05
estimated = true

[[accounts.extra_usage]]
on = 2026-02-10
amount = 25
currency = "EUR"
note = "top up"

[[accounts]]
provider = "claude"
id = "other"
config_dir = "~/.other"
""")
    work, other = load(tmp_path).accounts
    assert [(plan.account, plan.plan, plan.estimated) for plan in work.plans] == [
        ("work", "pro", True), ("work", "max20", False)]
    assert work.plans[0].since == datetime(2026, 1, 5, tzinfo=timezone.utc)
    assert [(item.account, item.amount, item.currency, item.note) for item in work.extra_usage] == [
        ("work", 25.0, "EUR", "top up")]
    assert other.plans == () and other.extra_usage == ()


def test_adapters_name_the_provider_conventions():
    claude, codex = providers.ADAPTERS["claude"], providers.ADAPTERS["codex"]
    assert (claude.env_var, claude.default_dir, claude.transcript_subdir) == ("CLAUDE_CONFIG_DIR", "~/.claude", "projects")
    assert (codex.env_var, codex.default_dir, codex.transcript_subdir) == ("CODEX_HOME", "~/.codex", "sessions")
    assert claude.snapshot_key("work") == "work"
    assert codex.snapshot_key("work") == "codex:work"


def test_plan_and_extra_usage_stay_importable_from_usage_settings():
    from sakura.usage.archive import settings

    assert settings.Plan is registry.Plan and settings.ExtraUsage is registry.ExtraUsage
    assert settings.CODEX_ACCOUNT_PREFIX == providers.CODEX_ACCOUNT_PREFIX


def test_non_utf8_accounts_toml_is_a_config_error(tmp_path):
    write_registry(tmp_path, "").write_bytes(b'[[accounts]]\nlabel = "caf\xe9"\n')
    with pytest.raises(AccountsConfigError, match="not valid UTF-8"):
        load(tmp_path)


def test_non_utf8_usage_toml_is_a_config_error(tmp_path):
    usage = tmp_path / ".config" / "sakura" / "usage.toml"
    usage.parent.mkdir(parents=True)
    usage.write_bytes(b'pr_repos = ["caf\xe9"]\n')
    with pytest.raises(AccountsConfigError, match="not valid UTF-8"):
        load(tmp_path)
