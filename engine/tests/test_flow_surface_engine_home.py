"""Review 17, finding 9: helper windows and the pinned tmux client keep `SAKURA_ENGINE_HOME`."""

import os
import subprocess
import sys
from pathlib import Path

from sakura import engine_home, programs
from sakura.flow import registry, surface

SAKURA = Path(__file__).resolve().parents[1] / "bin" / "sakura"


def test_WindowEnvironment_WithAnEngineHome_CarriesItToEveryHelperWindow():
    shown = surface.TmuxSurface(client=None, prefix="flow-", environ={"HOME": "/h", "PATH": "/usr/bin",
                                                                   engine_home.ENV_VARIABLE: "/elsewhere"})
    assert shown._window_env()[engine_home.ENV_VARIABLE] == "/elsewhere"


def test_WindowEnvironment_WithoutAnEngineHome_AddsNone():
    shown = surface.TmuxSurface(client=None, prefix="flow-", environ={"HOME": "/h", "PATH": "/usr/bin"})
    assert engine_home.ENV_VARIABLE not in shown._window_env()


def test_PinnedTmuxEnvironment_WithAnEngineHome_KeepsIt():
    environment = programs.environment("tmux", "/usr/bin/tmux", {engine_home.ENV_VARIABLE: "/elsewhere", "SECRET": "x"})
    assert environment[engine_home.ENV_VARIABLE] == "/elsewhere" and "SECRET" not in environment


def test_HelperWindowCommand_WithTheForwardedEngineHome_RegistersNothingInTheDefaultHome(repo, tmp_path):
    default_home = tmp_path / "default-home"
    default_home.mkdir()
    chosen = tmp_path / "chosen"
    chosen.mkdir(mode=0o700)
    shown = surface.TmuxSurface(client=None, prefix="flow-", environ={"HOME": str(default_home),
                                                                   "PATH": os.environ["PATH"],
                                                                   engine_home.ENV_VARIABLE: str(chosen)})
    env = {**shown._window_env(), "SAKURA_PYTHON": sys.executable}
    done = subprocess.run([str(SAKURA), "flow", "events", "nosuchtask"], cwd=repo, env=env, capture_output=True,
                          text=True, stdin=subprocess.DEVNULL, timeout=60)
    assert done.returncode != 0, "the task does not exist"
    assert not any(default_home.rglob("*registry*")) and not (default_home / ".local").exists()
    assert any(chosen.rglob("*")), done.stderr
