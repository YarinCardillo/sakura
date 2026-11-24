import json
from datetime import datetime, timedelta

import pytest

from sakura import engine_home
from sakura.flow import cli, config, exits, gates

DAY_WINDOW = '[gates]\nday = "{window}"\n'


def _load(repo, text=""):
    home = repo / "home"
    if text:
        path = engine_home.config_file()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return config.load(repo, home=home)


def _at(hour, minute=0, day=25):
    return datetime(2026, 10, day, hour, minute)


def test_auto_mode_window_and_wrap(repo, tmp_path):
    absent = tmp_path / "mode"
    default = _load(repo)
    assert gates.effective_mode(default, _at(7, 59), absent) == ("night", "auto")
    assert gates.effective_mode(default, _at(8, 0), absent) == ("day", "auto")
    assert gates.effective_mode(default, _at(22, 59), absent) == ("day", "auto")
    assert gates.effective_mode(default, _at(23, 0), absent) == ("night", "auto")
    wrapped = _load(repo, DAY_WINDOW.format(window="22:00-06:00"))
    assert [gates.effective_mode(wrapped, _at(h), absent)[0] for h in (23, 3, 6, 12, 22)] == [
        "day", "day", "night", "night", "day"]


def test_override_wins(repo, tmp_path):
    override = tmp_path / "mode"
    cfg = _load(repo)
    override.write_text("night\n")
    assert gates.effective_mode(cfg, _at(12), override) == ("night", "override")
    override.write_text("day\n")
    assert gates.effective_mode(cfg, _at(3), override) == ("day", "override")
    for ignored in ("auto\n", "dusk\n", ""):
        override.write_text(ignored)
        assert gates.effective_mode(cfg, _at(3), override) == ("night", "auto")


def test_config_mode_pins_the_mode(repo, tmp_path):
    pinned = _load(repo, '[gates]\nmode = "night"\n')
    assert gates.effective_mode(pinned, _at(12), tmp_path / "mode") == ("night", "config")


@pytest.mark.parametrize("text", ['[gates]\nmode = "dusk"\n', '[gates]\nday = "8-23"\n', '[gates]\nday = "08:00-25:00"\n',
                                  '[gates]\nday = "08:00-08:00"\n', '[gates]\nday = 7\n'])
def test_invalid_gates_fail_config_validation(repo, text):
    with pytest.raises(config.FlowConfigError, match="gates"):
        _load(repo, text)


@pytest.mark.parametrize("layer", [".sakura/flow.toml", ".sakura/local.toml"])
def test_only_the_personal_file_sets_gates(repo, layer):
    path = repo / layer
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('[gates]\nmode = "night"\n' if layer.endswith("flow.toml") else '[flow.gates]\nmode = "night"\n')
    with pytest.raises(config.FlowConfigError, match=r"gates\.mode.*<engine home>/config.toml"):
        config.load(repo, home=repo / "nohome")


def _defer(directory, name, at, event="escalation"):
    directory.mkdir(parents=True, exist_ok=True)
    body = {"at": at.astimezone().isoformat(timespec="seconds"), "repo": "r", "task": "t", "event": event, "message": "m"}
    (directory / name).write_text(json.dumps(body))


def test_deferred_since_last_day_lists_until_a_day_period_begins(repo, tmp_path):
    cfg = _load(repo)
    directory = tmp_path / "deferred"
    _defer(directory, "1-a.json", _at(23, 30, day=24))
    _defer(directory, "2-b.json", _at(3, 0))
    (directory / "3-broken.json").write_text("{")
    (directory / ".4-c.json.tmp").write_text("{}")
    assert [e["event"] for e in gates.deferred_since_last_day(directory, cfg, _at(4, 0))] == ["escalation", "escalation"]
    assert len(gates.deferred_since_last_day(directory, cfg, _at(7, 59))) == 2
    assert gates.deferred_since_last_day(directory, cfg, _at(8, 0)) == []
    assert len(gates.deferred_since_last_day(directory, cfg, _at(23, 5))) == 0


def test_deferred_since_last_day_without_directory_is_empty(repo, tmp_path):
    assert gates.deferred_since_last_day(tmp_path / "none", _load(repo), _at(1)) == []


@pytest.fixture
def operator(repo, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SAKURA_FLOW_ROLE", raising=False)
    return repo


def test_mode_command(operator, capsys):
    assert cli.main(["mode"]) == exits.OK
    assert capsys.readouterr().out.strip().endswith("(auto)")
    assert cli.main(["mode", "night"]) == exits.OK
    assert gates.default_mode_file().read_text().strip() == "night"
    cli.main(["mode"])
    assert capsys.readouterr().out.strip().endswith("night (override)")
    assert cli.main(["mode", "day"]) == exits.OK
    assert gates.default_mode_file().read_text().strip() == "day"
    assert cli.main(["mode", "auto"]) == exits.OK
    assert not gates.default_mode_file().exists()


def test_mode_command_refused_inside_an_engine_role(operator, monkeypatch):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["mode", "night"]) == exits.USAGE
    assert not gates.default_mode_file().exists()


def test_mode_command_rejects_unknown_mode(operator):
    with pytest.raises(SystemExit):
        cli.main(["mode", "dusk"])
