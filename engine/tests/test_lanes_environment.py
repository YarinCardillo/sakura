"""Lane commands taken from the repository configuration run with no tmux identity and a private tmux directory."""

from __future__ import annotations

import stat
from pathlib import Path

import pytest
from test_lanes_identity import make_group

from sakura.lanes import status, worktrees

OWNER_SOCKET = "/tmp/owner-socket,1,0"


def recorder(tmp_path: Path, exit_code: int = 0) -> tuple[Path, Path]:
    """A script that writes the tmux variables it sees to a file, and that file."""
    seen = tmp_path / "seen.txt"
    script = tmp_path / "record.sh"
    script.write_text(f'#!/bin/sh\nprintf \'%s\\n%s\\n%s\\n\' "${{TMUX-unset}}" "${{TMUX_PANE-unset}}" '
                      f'"${{TMUX_TMPDIR-unset}}" > {seen}\nexit {exit_code}\n')
    script.chmod(0o755)
    return script, seen


@pytest.fixture(params=["direct-cli-inside-tmux", "cockpit-child"])
def caller_environment(request, monkeypatch, tmp_path):
    """The caller's tmux variables: all three set when a lane command runs inside tmux, TMUX and TMUX_PANE
    already dropped when the cockpit spawned it (it keeps TMUX_TMPDIR so the lane command can reach the person's server)."""
    owner_tmpdir = tmp_path / "owner-tmux"
    owner_tmpdir.mkdir()
    monkeypatch.setenv("TMUX_TMPDIR", str(owner_tmpdir))
    if request.param == "direct-cli-inside-tmux":
        monkeypatch.setenv("TMUX", OWNER_SOCKET)
        monkeypatch.setenv("TMUX_PANE", "%3")
    else:
        monkeypatch.delenv("TMUX", raising=False)
        monkeypatch.delenv("TMUX_PANE", raising=False)
    return owner_tmpdir


def assert_private_tmux_view(seen: Path, owner_tmpdir: Path) -> None:
    tmux, pane, tmpdir = seen.read_text().splitlines()
    assert (tmux, pane) == ("unset", "unset")
    assert tmpdir != str(owner_tmpdir) and tmpdir != "unset"
    assert stat.S_IMODE(Path(tmpdir).stat().st_mode) == 0o700


def test_setup_and_down_commands_see_no_tmux_identity_and_a_private_tmux_dir(tmp_path, caller_environment):
    script, seen = recorder(tmp_path)
    worktrees.run_command(str(script), tmp_path, "setup")
    assert_private_tmux_view(seen, caller_environment)


def test_status_command_sees_no_tmux_identity_and_a_private_tmux_dir(tmp_path, caller_environment):
    script, seen = recorder(tmp_path)
    group = make_group(tmp_path, status=str(script))
    assert status._stack_state(group, tmp_path) == "up"
    assert_private_tmux_view(seen, caller_environment)
