"""Lane writers refuse engine role sessions; `lane status` stays readable."""

from __future__ import annotations

import pytest

from sakura.flow import exits
from sakura.lanes import cli

REFUSAL = "operator commands cannot run inside an engine role"
WRITERS = [["open", "alpha"], ["done", "alpha", "--remove"], ["say", "alpha", "hi"], ["model", "alpha", "opus"]]


class UntouchableClient:
    """A tmux client that fails the test on any use."""

    def __getattr__(self, name):
        raise AssertionError(f"tmux client used: {name}")


@pytest.mark.parametrize("role", ["builder", "evaluator", "planner"])
@pytest.mark.parametrize("arguments", WRITERS)
def test_lane_writer_in_role_session_is_refused_before_any_tmux_use(arguments, role, tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", role)
    log = tmp_path / "events.jsonl"

    code = cli.main(arguments, client=UntouchableClient(), config_path=tmp_path / "lanes.toml", events_path=log)

    assert code == exits.USAGE
    assert REFUSAL in capsys.readouterr().err


def test_lane_status_in_role_session_is_not_refused(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")

    cli.main(["status"], client=UntouchableClient(), config_path=tmp_path / "lanes.toml")

    assert REFUSAL not in capsys.readouterr().err
