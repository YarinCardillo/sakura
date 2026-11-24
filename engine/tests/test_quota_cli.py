import json

import pytest
from quota_support import NOW, FakeRunner, clock, event

from sakura.quota import cli


@pytest.fixture
def host(tmp_path, monkeypatch):
    """A HOME with one registered Claude account whose directory exists."""
    home = tmp_path / "isolated-home"
    work = home / ".claude-work"
    (work / "projects").mkdir(parents=True)
    config = home / ".config/sakura"
    config.mkdir(parents=True)
    (config / "accounts.toml").write_text(
        f'[[accounts]]\nprovider = "claude"\nid = "work"\nlabel = "Work"\nconfig_dir = "{work}"\nenabled = true\n')
    (config / "usage.toml").write_text(f'db = "{tmp_path / "usage.db"}"\n')
    return home, str(work)


def test_Main_StatusJson_PrintsTheContract(host, capsys):
    home, work = host
    code = cli.main(["status", "--json"], FakeRunner({work: event(seven=0.94)}), clock, home)
    document = json.loads(capsys.readouterr().out)
    assert code == 0
    assert set(document) == {"generated_at", "accounts", "history"}
    assert document["accounts"][0]["seven_day"]["pct"] == 94.0
    assert set(document["accounts"][0]["forecast"]) == {
        "seven_day_pct_at_reset", "next_round_pct", "exhausts_at", "confidence"}


def test_Main_StatusText_ShowsLabelAndWeeklyPercentage(host, capsys):
    home, work = host
    cli.main(["status"], FakeRunner({work: event(seven=0.94)}), clock, home)
    out = capsys.readouterr().out
    assert "Work" in out and "94%" in out and "in 10h00m" in out


def test_Main_ProbeDown_MarksTheAccountStale(host, capsys):
    home, _ = host
    cli.main(["status"], FakeRunner(), clock, home)
    assert "Work*" in capsys.readouterr().out


def advise_args(repo, *extra):
    return ["advise", "--repo", str(repo), "--role", "builder", "--task-class", "integration", *extra]


def test_Main_Advise_PrintsTheContractFields(host, capsys):
    home, work = host
    code = cli.main(advise_args(home / "p", "--agent", "claude"), FakeRunner({work: event(seven=0.5)}), clock, home)
    answer = json.loads(capsys.readouterr().out)
    assert code == 0
    assert set(answer) >= {"verdict", "agent", "model", "effort", "config_dir", "reason", "retry_at"}
    assert (answer["verdict"], answer["model"], answer["config_dir"]) == ("go", "sonnet", work)


def test_Main_Advise_AccountOverItsCeiling_Waits(host, capsys):
    home, work = host
    cli.main(advise_args(home / "p", "--agent", "claude"), FakeRunner({work: event(seven=0.98)}), clock, home)
    answer = json.loads(capsys.readouterr().out)
    assert (answer["verdict"], answer["retry_at"]) == ("wait", "2026-11-03T07:00:00+00:00")


def test_Main_BrokenQuotaConfig_FailsWithTheMessage(host, capsys):
    home, _ = host
    (home / ".config/sakura/quota.toml").write_text("port = 0")
    assert cli.main(["status"], FakeRunner(), clock, home) == 1
    assert "port" in capsys.readouterr().err


def test_Main_ConfigOption_ReplacesTheDefaultRulesFile(host, tmp_path, capsys):
    home, work = host
    rules = tmp_path / "rules.toml"
    rules.write_text('[default]\nclaude = ["nobody"]\n')
    cli.main(["--config", str(rules), *advise_args(home / "p", "--agent", "claude")], FakeRunner({work: event()}), clock, home)
    assert json.loads(capsys.readouterr().out)["verdict"] == "deny"
