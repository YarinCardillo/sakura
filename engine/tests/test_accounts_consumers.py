"""The account registry as the one source of accounts for the archive, the budget and the flow role environments."""

from __future__ import annotations

import re
import shutil
import sqlite3
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura.accounts import detect, registry
from sakura.budget import account as budget_account
from sakura.flow import process, runner
from sakura.usage.archive import command, settings, store
from flow_support import MonitorLauncher, make_context, task_workspace

FIXTURES = Path(__file__).parent / "fixtures" / "usage" / "archive"
NOW = datetime(2026, 10, 29, 12, 0, tzinfo=timezone.utc)
LIB = Path(__file__).resolve().parents[1] / "lib"


@pytest.fixture
def home(tmp_path, monkeypatch):
    path = tmp_path / "home"
    path.mkdir()
    monkeypatch.setenv("HOME", str(path))
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    return path


def env_of(home):
    return {"HOME": str(home)}


def claude_dir(path: Path) -> Path:
    shutil.copytree(FIXTURES / "claude", path / "projects")
    return path


def codex_dir(path: Path) -> Path:
    shutil.copytree(FIXTURES / "codex", path / "sessions")
    return path


def write_accounts(home: Path, text: str) -> None:
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def entry(provider: str, account_id: str, directory: Path, enabled: bool = True) -> str:
    return (f'[[accounts]]\nprovider = "{provider}"\nid = "{account_id}"\nconfig_dir = "{directory}"\n'
            f'enabled = {"true" if enabled else "false"}\n')


class ProbeRunner:
    """A quota runner that records the CLAUDE_CONFIG_DIR of every call and returns no rate limit event."""

    def __init__(self):
        self.dirs = []

    def __call__(self, argv, env, cwd):
        self.dirs.append(env.get("CLAUDE_CONFIG_DIR", "<unset>"))
        return subprocess.CompletedProcess(argv, 0, "", "")


def run_archive(home, probe=None, capsys=None):
    loaded = settings.load(env=env_of(home))
    (loaded.db.parent).mkdir(parents=True, exist_ok=True)
    code = command.archive(loaded, lambda argv: subprocess.CompletedProcess(argv, 0, "[]", ""), NOW,
                           quota_run=probe or ProbeRunner(), waker=lambda: [])
    assert code == 0
    return loaded


def archived(loaded, sql):
    conn = sqlite3.connect(loaded.db)
    try:
        return conn.execute(sql).fetchall()
    finally:
        conn.close()


def test_zero_config_archives_the_detected_claude_dir_as_default(home, capsys):
    claude_dir(home / ".claude")

    loaded = run_archive(home)

    assert [(s.provider, s.account, s.config_dir) for s in loaded.sources] == [("claude", "default", home / ".claude")]
    assert archived(loaded, "SELECT DISTINCT provider, account FROM turns") == [("claude", "default")]
    assert "claude default: 1 files, 1 read, 2 turns" in capsys.readouterr().out


def test_zero_config_detects_a_codex_dir_with_its_config_dir(home):
    codex_dir(home / ".codex")

    (source,) = settings.load(env=env_of(home)).sources

    assert source == settings.Source("codex", "default", home / ".codex" / "sessions", home / ".codex")


def test_zero_config_leaves_the_budget_off(home):
    claude_dir(home / ".claude")
    env = env_of(home)

    assert settings.config_exists(env) is False
    assert command._alert_lines(sqlite3.connect(":memory:"), settings.load(env=env), NOW) == []


def test_the_budget_turns_on_with_accounts_toml_or_legacy_keys(home):
    usage = home / ".config" / "sakura" / "usage.toml"
    usage.parent.mkdir(parents=True)
    usage.write_text('db = "~/x.db"\n')
    assert settings.config_exists(env_of(home)) is False
    usage.write_text(f'[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "{home}/a/projects"\n')
    assert settings.config_exists(env_of(home)) is True
    usage.unlink()
    write_accounts(home, entry("claude", "a", home / "a"))
    assert settings.config_exists(env_of(home)) is True


def test_detected_implicit_accounts_are_not_registered_ones(home):
    claude_dir(home / ".claude")
    implicit = registry.load(env=env_of(home))
    assert implicit.implicit and [a.id for a in implicit.accounts] == ["default"]

    proposals = detect.propose(detect.detect(home, env_of(home)), implicit, home)

    assert [(p.proposed_id, p.registered_as) for p in proposals] == [("default", None)]


def test_disabled_account_is_not_scanned_or_probed(home, capsys):
    on, off = claude_dir(home / ".on"), claude_dir(home / ".off")
    write_accounts(home, entry("claude", "on", on) + entry("claude", "off", off, enabled=False))
    probe = ProbeRunner()

    loaded = run_archive(home, probe)

    assert [s.account for s in loaded.sources] == ["on"]
    assert archived(loaded, "SELECT DISTINCT account FROM turns") == [("on",)]
    assert set(probe.dirs) == {str(on)}
    with pytest.raises(budget_account.UnknownAccountError, match="'off'.*disabled"):
        budget_account.resolve("off", loaded.sources, {}, disabled=loaded.disabled)
    with pytest.raises(budget_account.UnknownAccountError, match="'off'.*disabled"):
        budget_account.apply({}, "off", loaded.sources, disabled=loaded.disabled)


def test_missing_dir_is_skipped_with_one_line(home, capsys):
    present = claude_dir(home / ".present")
    write_accounts(home, entry("claude", "gone", home / ".gone") + entry("claude", "present", present))
    probe = ProbeRunner()

    loaded = run_archive(home, probe)

    streams = capsys.readouterr()
    assert (streams.out + streams.err).count("claude gone: config dir missing, skipped") == 1
    assert archived(loaded, "SELECT DISTINCT account FROM turns") == [("present",)]
    assert probe.dirs and set(probe.dirs) == {str(present)}
    assert "archive done" in streams.out


def test_codex_role_gets_codex_home_of_the_paired_account(home, repo):
    work = home / ".codex-work"
    write_accounts(home, entry("claude", "work", home / ".claude-work") + entry("codex", "work", work)
                   + entry("codex", "other", home / ".codex-other"))
    seen = _launch(repo, "codex", "work")
    assert seen["CODEX_HOME"] == str(work)


def test_codex_role_gets_the_only_codex_account_when_no_id_matches(home, repo):
    only = home / ".codex-only"
    write_accounts(home, entry("claude", "work", home / ".claude-work") + entry("codex", "solo", only))
    assert _launch(repo, "codex", "work")["CODEX_HOME"] == str(only)


def test_codex_role_keeps_its_environment_without_a_pairing(home, repo, monkeypatch):
    monkeypatch.setenv("CODEX_HOME", "/inherited")
    write_accounts(home, entry("claude", "work", home / ".claude-work"))
    assert _launch(repo, "codex", "work")["CODEX_HOME"] == "/inherited"


def _launch(repo, provider, account):
    """Run a role of `provider` with `budget.account` set; return the environment it was launched with."""
    # Codex only reviews, so a Codex run is the code reviewer.
    role, phase = ("code_reviewer", "review") if provider == "codex" else ("builder", "build")
    ctx = make_context(repo, overrides={"budget": {"account": account}, "roles": {role: provider}})
    workspace = task_workspace(ctx)
    if provider == "codex":
        from sakura import codex_trust, install
        install.install_files(workspace)
        codex_trust.record(workspace)
    seen = {}

    def monitor(argv, **kwargs):
        seen.update(kwargs["env"])
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
        agent.run(runner.RunRequest(role, phase, "x", {}, workspace))
    return seen


def test_unowned_claude_config_dir_fails_resolve(home):
    claude_dir(home / ".claude")
    sources = settings.load(env=env_of(home)).sources
    assert budget_account.resolve("", sources, {}) == "default"

    with pytest.raises(budget_account.UnknownAccountError, match="elsewhere"):
        budget_account.resolve("", sources, {"CLAUDE_CONFIG_DIR": str(home / "elsewhere")})


def test_default_dir_account_leaves_claude_config_dir_unset_in_the_role_env(home, repo, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/inherited")
    claude_dir(home / ".claude")
    assert "CLAUDE_CONFIG_DIR" not in _launch(repo, "claude:sonnet", "default")


def test_no_account_literals_outside_accounts_package():
    """Account ids come from the registry. The quoted id `personal` is banned only where account ids live."""
    banned = [(re.compile(r"\.claude-work"), None),
              (re.compile(r"""["']personal["']"""), ("usage", "budget"))]
    offenders = []
    for path in sorted(LIB.rglob("*.py")):
        parts = path.relative_to(LIB / "sakura").parts
        if parts[0] == "accounts":
            continue
        text = path.read_text(encoding="utf-8")
        for pattern, folders in banned:
            if (folders is None or parts[0] in folders) and pattern.search(text):
                offenders.append(f"{path.relative_to(LIB)}: {pattern.pattern}")
    assert offenders == []


def test_zero_config_accounts_ignore_dot_directories(home):
    claude_dir(home / ".claude")
    claude_dir(home / ".conductor")
    codex_dir(home / ".factory")
    codex_dir(home / ".codex")

    implicit = registry.load(env=env_of(home))

    assert [(a.provider, a.id, a.config_dir) for a in implicit.accounts] == [
        ("claude", "default", (home / ".claude").resolve()), ("codex", "default", (home / ".codex").resolve())]
