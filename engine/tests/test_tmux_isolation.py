"""The test suite must never reach the person's tmux server, even when it runs inside a tmux pane."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path
from types import SimpleNamespace

import tmux_support
from tmux_support import TMUX_PROGRAM, needs_tmux

ROOT = Path(__file__).resolve().parents[1]


def test_tests_run_without_the_owners_tmux_variables():
    assert "TMUX" not in os.environ and "TMUX_PANE" not in os.environ
    assert os.environ["TMUX_TMPDIR"].startswith(os.environ["SAKURA_TEST_TMUX_ROOT"])


def test_isolation_tests_never_run_a_bare_tmux():
    bare_call = re.compile(r"""\[\s*["']tmux["']\s*,\s*["'](?!-[SL]["'])""")
    for relative in ("tests/test_tmux_isolation.py",):
        assert not bare_call.search((ROOT / relative).read_text()), f"{relative} runs a tmux without -S or -L"


@needs_tmux
def test_default_socket_of_the_private_tmpdir_has_no_server(tmp_path, monkeypatch):
    fake = tmp_path / "tmux"
    fake.write_text("#!/bin/sh\nexit 0\n")
    fake.chmod(0o700)
    monkeypatch.setenv("PATH", str(tmp_path))
    default_socket = Path(os.environ["TMUX_TMPDIR"]) / f"tmux-{os.getuid()}" / "default"
    done = subprocess.run([TMUX_PROGRAM, "-S", str(default_socket), "list-sessions"], capture_output=True, text=True)
    assert done.returncode != 0
    assert "no server running" in done.stderr or os.environ["SAKURA_TEST_TMUX_ROOT"] in done.stderr


def test_tmux_client_uses_the_private_socket_and_pinned_binary(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(tmux_support, "TMUX_PROGRAM", str(tmp_path / "pinned-tmux"))
    monkeypatch.setattr(tmux_support, "subprocess", SimpleNamespace(
        run=lambda argv, **kwargs: calls.append((argv, kwargs["env"])),
    ))
    monkeypatch.setenv("TMUX", "/tmp/concurrent-test/default,1,0")
    monkeypatch.setenv("TMUX_PANE", "%1")

    tmux_support._isolated_run(["tmux", "-L", "private", "list-sessions"])

    (argv, env), = calls
    assert argv == [str(tmp_path / "pinned-tmux"), "-L", "private", "-f", "/dev/null", "list-sessions"]
    assert "TMUX" not in env and "TMUX_PANE" not in env


def _demo(tmp_path, monkeypatch):
    monkeypatch.syspath_prepend(str(ROOT / "acceptance"))
    import harness

    monkeypatch.setattr(harness, "NtfyCapture", lambda: SimpleNamespace(url="http://127.0.0.1:42009", close=lambda: None))
    monkeypatch.setattr(harness.Demo, "_kill_leftover_apps", lambda _demo: None)
    return harness.Demo(tmp_path)
