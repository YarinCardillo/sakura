from sakura import testing
from sakura.flow import process
from flow_support import make_context
import pytest


def test_focused_test_command_timeout_is_bounded_and_clears_active_child(repo, monkeypatch):
    ctx = make_context(repo, own_repository=True, overrides={"tests": {"focused": ["sleep 30"]}})
    monkeypatch.setattr(testing, "_commands", lambda _repo, _suite: ["sleep 30"])

    with pytest.raises(process.TestCommandInterrupted) as raised:
        testing.run(repo, "focused", command_runner=lambda command, cwd:
                     process.run_test_command(ctx, command, cwd, budget_seconds=0.2, idle_seconds=1))

    assert raised.value.outcome.timed_out
    state = ctx.store.read_state()
    assert state["active_runs"] == []
    assert state["active_run"] is None
