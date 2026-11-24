"""Plans and extra usage per provider: a Codex account keeps its own history under its snapshot key."""

from __future__ import annotations

import os
import stat
import tomllib
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import cli
from sakura.accounts import registry
from sakura.accounts.registry import Plan
from sakura.budget import predict, preflight, weekly, weekly_window
from sakura.usage.archive import calibration, command, settings, store

NOW = datetime(2026, 10, 29, 12, 0, tzinfo=timezone.utc)
CODEX_SINCE = datetime(2026, 10, 21, tzinfo=timezone.utc)
LEGACY_PLANS = (("alpha", "pro", "2026-01-01"), ("alpha", "max5", "2026-02-01"), ("beta", "pro", "2026-01-15"),
                ("alpha", "max20", "2026-03-01"), ("beta", "max5", "2026-05-01"))
LEGACY_USAGE = """
[plan_weights]
pro = 1
max5 = 5
max20 = 20

[[sources]]
provider = "claude"
account = "alpha"
root = "~/.claude/projects"

[[sources]]
provider = "claude"
account = "beta"
root = "~/.altclaude/projects"
""" + "".join(f'\n[[plans]]\naccount = "{who}"\nplan = "{plan}"\nsince = {since}\n' for who, plan, since in LEGACY_PLANS)


@pytest.fixture
def home(tmp_path, monkeypatch) -> Path:
    path = tmp_path / "home"
    (path / ".config" / "sakura").mkdir(parents=True)
    monkeypatch.setenv("HOME", str(path))
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    return path


def accounts_file(home: Path) -> Path:
    return home / ".config" / "sakura" / "accounts.toml"


def entry(provider: str, account_id: str, home: Path, body: str = "") -> str:
    return (f'[[accounts]]\nprovider = "{provider}"\nid = "{account_id}"\nconfig_dir = "{home / f".{provider}-{account_id}"}"\n'
            + body)


def write_accounts(home: Path, text: str) -> None:
    accounts_file(home).write_text(text, encoding="utf-8")


def plan_body(plan: str, since: str) -> str:
    return f'[[accounts.plans]]\nplan = "{plan}"\nsince = "{since}"\n'


EXTRA_BODY = '[[accounts.extra_usage]]\non = "2026-10-25"\namount = 40\ncurrency = "EUR"\nnote = "top up"\n'


def test_codex_plans_load_and_key_by_snapshot_key(home):
    write_accounts(home, entry("codex", "work", home, plan_body("plus", "2026-09-01") + plan_body("prolite", "2026-10-21")
                               + EXTRA_BODY))

    loaded = registry.load(env={"HOME": str(home)}).accounts[0]
    assert [(p.account, p.plan) for p in loaded.plans] == [("codex:work", "plus"), ("codex:work", "prolite")]
    assert [x.account for x in loaded.extra_usage] == ["codex:work"]

    usage = settings.load(env={"HOME": str(home)})
    assert [(p.account, p.plan) for p in usage.plans] == [("codex:work", "plus"), ("codex:work", "prolite")]
    assert [(x.account, x.amount) for x in usage.extra_usage] == [("codex:work", 40.0)]
    assert settings.plan_at(usage.plans, "codex:work", NOW) == "prolite"
    assert settings.plan_at(usage.plans, "work", NOW) is None


def test_claude_plan_lookups_unchanged(home):
    usage_path = home / ".config" / "sakura" / "usage.toml"
    usage_path.write_text(LEGACY_USAGE, encoding="utf-8")
    raw = tomllib.loads(LEGACY_USAGE)["plans"]
    probes = [datetime(2026, 1, 1, tzinfo=timezone.utc) + timedelta(days=day * 5) for day in range(60)]

    def expected(who: str, at: datetime) -> str | None:
        started = [(since, plan) for acct, plan, since in LEGACY_PLANS
                   if acct == who and datetime.fromisoformat(since).replace(tzinfo=timezone.utc) <= at]
        return max(started)[1] if started else None

    assert len(raw) == len(LEGACY_PLANS)
    legacy = settings.load(env={"HOME": str(home)})
    cli.main(["accounts", "migrate", "--write"])
    migrated = settings.load(env={"HOME": str(home)})
    for loaded in (legacy, migrated):
        assert {p.account for p in loaded.plans} == {"alpha", "beta"}
        for who in ("alpha", "beta"):
            assert [settings.plan_at(loaded.plans, who, at) for at in probes] == [expected(who, at) for at in probes]


def test_same_id_claude_and_codex_plans_do_not_mix(home):
    write_accounts(home, entry("claude", "work", home, plan_body("max5x", "2026-01-01") + EXTRA_BODY)
                   + entry("codex", "work", home, plan_body("prolite", "2026-10-21")))

    usage = settings.load(env={"HOME": str(home)})

    assert settings.plan_at(usage.plans, "work", NOW) == "max5x"
    assert settings.plan_at(usage.plans, "codex:work", NOW) == "prolite"
    assert [x.account for x in usage.extra_usage] == ["work"]


def test_accounts_plan_and_extra_write_atomically(home):
    write_accounts(home, entry("claude", "work", home) + entry("codex", "work", home))
    path = accounts_file(home)

    assert cli.main(["accounts", "plan", "codex", "work", "prolite", "2026-10-21", "--estimated"]) == 0
    assert cli.main(["accounts", "plan", "codex", "work", "plus", "2026-09-01T08:30:00"]) == 0
    assert cli.main(["accounts", "extra", "codex", "work", "2026-10-25", "40", "EUR", "--note", "top up"]) == 0

    claude, codex = registry.load(path).accounts
    assert claude.plans == () and claude.extra_usage == ()
    assert [(p.plan, p.since, p.estimated) for p in codex.plans] == [
        ("plus", datetime(2026, 9, 1, 8, 30, tzinfo=timezone.utc), False), ("prolite", CODEX_SINCE, True)]
    assert [(x.on, x.amount, x.currency, x.note) for x in codex.extra_usage] == [
        (date(2026, 10, 25), 40.0, "EUR", "top up")]
    assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
    assert sorted(p.name for p in path.parent.iterdir()) == ["accounts.toml", "accounts.toml.lock"]


def test_a_duplicate_plan_or_a_missing_account_is_refused(home, capsys):
    write_accounts(home, entry("codex", "work", home))
    assert cli.main(["accounts", "plan", "codex", "work", "plus", "2026-09-01"]) == 0
    before = accounts_file(home).read_bytes()

    for argv in (["plan", "codex", "work", "plus", "2026-09-01"], ["plan", "claude", "work", "plus", "2026-09-02"],
                 ["plan", "codex", "work", "plus", "yesterday"], ["extra", "codex", "work", "2026-10-25", "-4", "EUR"],
                 ["extra", "codex", "work", "2026-10-25", "many", "EUR"]):
        with pytest.raises(SystemExit) as stopped:
            cli.main(["accounts", *argv])
        assert stopped.value.code == 2
    assert "duplicate plan" in capsys.readouterr().err
    assert accounts_file(home).read_bytes() == before


def test_plan_write_refused_on_a_legacy_host(home, capsys):
    usage_path = home / ".config" / "sakura" / "usage.toml"
    usage_path.write_text(LEGACY_USAGE, encoding="utf-8")

    for argv in (["plan", "claude", "alpha", "max5", "2026-10-01"], ["extra", "claude", "alpha", "2026-10-25", "5", "EUR"]):
        with pytest.raises(SystemExit) as stopped:
            cli.main(["accounts", *argv])
        assert stopped.value.code == 2
        assert "accounts migrate" in capsys.readouterr().err
    assert usage_path.read_text(encoding="utf-8") == LEGACY_USAGE
    assert not accounts_file(home).exists()


def ratio(account: str, plan: str, per_pct: float) -> calibration.Ratio:
    return calibration.Ratio(account, plan, "opus", per_pct, 3, 3.0)


def codex_plans() -> tuple[Plan, ...]:
    return (Plan("codex:work", "plus", datetime(2026, 9, 1, tzinfo=timezone.utc)), Plan("codex:work", "prolite", CODEX_SINCE))


def test_plan_without_weight_says_no_calibration(tmp_path, capsys):
    plans, mix = codex_plans(), {"opus": 1000.0}
    plus_only = [ratio("codex:work", "plus", 100.0)]

    assert calibration.estimate(plus_only, plans, {"plus": 1.0}, "codex:work", NOW, mix) is None
    assert calibration.estimate(plus_only, plans, {"plus": 1.0, "prolite": 5.0}, "codex:work", NOW, mix).scaled

    usage = settings.Settings(db=None, sources=(), pr_repos=(), pr_days=7, plans=plans, plan_weights={"plus": 1.0})
    tokens = {"opus": predict.Band(1000.0, 1000.0, 1000.0)}
    assert preflight.percent_band(tokens, plus_only, usage, "codex:work", NOW) == (None, "no calibration for prolite")

    config = settings.Settings(db=tmp_path / "state" / "usage.db", sources=(), pr_repos=(), pr_days=7, plans=plans,
                               plan_weights={"plus": 1.0})
    assert command.estimate(config, "codex:work", mix, False, NOW) == 1
    assert "no calibration for prolite" in capsys.readouterr().err


def test_codex_calibration_uses_only_its_own_plan_intervals(tmp_path):
    plans = (*codex_plans(), Plan("work", "prolite", CODEX_SINCE))
    mix = {"opus": 1000.0}
    ratios = [ratio("codex:work", "plus", 500.0), ratio("codex:work", "prolite", 100.0), ratio("work", "prolite", 10.0)]

    own = calibration.estimate(ratios, plans, {"plus": 1.0, "prolite": 5.0}, "codex:work", NOW, mix)

    assert (own.pct, own.scaled, own.basis) == (10.0, False, "prolite calibration")
    unweighted = calibration.estimate(ratios[:1] + ratios[2:], plans, {}, "codex:work", NOW, mix)
    assert unweighted is None, "another account's intervals of the same plan name never fill in"


def switched_usage(account: str, tmp_path: Path) -> settings.Settings:
    plans = (Plan(account, "max5", datetime(2026, 9, 1, tzinfo=timezone.utc)), Plan(account, "max20", CODEX_SINCE))
    return settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7, plans=plans)


def every_basis(usage: settings.Settings, account: str, conn, monkeypatch) -> dict[str, str]:
    """The basis each no-calibration path reports for `account`: figure, preflight, cheaper and outside."""
    monkeypatch.setattr(calibration, "outside_share", lambda conn, plans, days: [
        calibration.OutsideShare(account, NOW.date(), 5.0, None, None)])
    convert = weekly._Converter([], usage, account, NOW)
    tokens = {"opus": predict.Band(1000.0, 1000.0, 1000.0)}
    window = weekly_window.Window(NOW - timedelta(days=1), NOW)
    return {"figure": convert.figure({"opus": 1000.0}).basis,
            "empty": convert.figure({calibration.OTHER: 1.0}).basis,
            "preflight": preflight.percent_band(tokens, [], usage, account, NOW)[1],
            "cheaper": weekly._cheaper(convert, {"opus": 1000.0}).basis,
            "outside": weekly._outside(conn, usage, account, window).basis}


def test_claude_no_calibration_stays_plain_after_a_plan_switch(tmp_path, monkeypatch):
    usage = switched_usage("work", tmp_path)
    bases = every_basis(usage, "work", store.open_db(usage.db), monkeypatch)

    assert set(bases.values()) == {"no calibration"}, bases


def test_codex_no_calibration_names_the_plan_on_every_path(tmp_path, monkeypatch):
    usage = switched_usage("codex:work", tmp_path)
    bases = every_basis(usage, "codex:work", store.open_db(usage.db), monkeypatch)

    assert set(bases.values()) == {"no calibration for max20"}, bases
