"""The `sakura` command, its `agentkit` alias and the `box` dispatcher."""

import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest

from sakura import box_dispatch

KIT = Path(__file__).resolve().parents[1]
SAKURA = KIT / "bin" / "sakura"
ALIAS = KIT / "bin" / "agentkit"


def _stub_box(directory: Path, name: str = "box") -> Path:
    """A fake service command that prints its argv, one item per line, and exits 3."""
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.write_text('#!/bin/sh\nfor item in "$0" "$@"; do printf "%s\\n" "$item"; done\nexit 3\n')
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def _run(argv: list[str], env: dict[str, str]) -> subprocess.CompletedProcess:
    return subprocess.run(argv, capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL, timeout=60)


def test_alias_agentkit_prints_one_deprecation_line_and_forwards_argv(tmp_path):
    env = {**os.environ, "SAKURA_BOX_BIN": str(_stub_box(tmp_path / "bin"))}
    through_alias = _run([str(ALIAS), "box", "status", "--json"], env)
    direct = _run([str(SAKURA), "box", "status", "--json"], env)
    assert through_alias.returncode == direct.returncode == 3
    assert through_alias.stdout == direct.stdout
    assert through_alias.stderr.count("\n") == 1
    assert "agentkit" in through_alias.stderr and "sakura" in through_alias.stderr
    assert direct.stderr == ""


def test_box_forwards_argv_unchanged_to_the_command_named_by_the_environment(tmp_path):
    stub = _stub_box(tmp_path / "service bin")
    env = {**os.environ, "SAKURA_BOX_BIN": str(stub)}
    done = _run([str(SAKURA), "box", "new", "a task", "--", "-x", ""], env)
    assert done.returncode == 3
    assert done.stdout.splitlines() == [str(stub), "new", "a task", "--", "-x", ""]


def test_box_falls_back_to_box_on_path(tmp_path):
    stub = _stub_box(tmp_path / "path")
    env = {key: value for key, value in os.environ.items() if key != "SAKURA_BOX_BIN"}
    env["PATH"] = f"{stub.parent}{os.pathsep}{env.get('PATH', '')}"
    done = _run([str(SAKURA), "box", "--help"], env)
    assert done.returncode == 3
    assert done.stdout.splitlines()[1:] == ["--help"]


def test_box_without_the_service_command_fails_with_a_clear_message(tmp_path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "python3").symlink_to(sys.executable)
    (tools / "dirname").symlink_to("/usr/bin/dirname")
    env = {"HOME": str(tmp_path), "PATH": str(tools)}
    done = _run([str(SAKURA), "box", "--help"], env)
    assert done.returncode == box_dispatch.NOT_FOUND
    assert "SAKURA_BOX_BIN" in done.stderr and "PATH" in done.stderr


def test_resolve_rejects_an_environment_path_that_is_not_executable(tmp_path):
    plain = tmp_path / "box"
    plain.write_text("not a program\n")
    with pytest.raises(box_dispatch.BoxNotFound, match="SAKURA_BOX_BIN"):
        box_dispatch.resolve({"SAKURA_BOX_BIN": str(plain), "PATH": ""})


def test_resolve_prefers_the_environment_over_path(tmp_path):
    named = _stub_box(tmp_path / "named")
    on_path = _stub_box(tmp_path / "path")
    found = box_dispatch.resolve({"SAKURA_BOX_BIN": str(named), "PATH": str(on_path.parent)})
    assert found == named


def test_help_lists_the_engine_commands_with_one_line_each():
    done = _run([str(SAKURA), "--help"], dict(os.environ))
    assert done.returncode == 0
    lines = {line.split()[0]: line for line in done.stdout.splitlines() if line.startswith("  ") and line.split()}
    for command in ("flow", "preset", "stack", "box"):
        assert command in lines and len(lines[command].split()) > 3, command


def test_Dispatch_Lane_SaysShelved(tmp_path):
    done = _run([str(SAKURA), "lane", "status", "--group", "work"], dict(os.environ))
    assert done.returncode == 2
    assert done.stdout.count("\n") == 1 and "shelved" in done.stdout and "sakura flow start" in done.stdout
    assert "lane" not in {line.split()[0] for line in _run([str(SAKURA), "--help"], dict(os.environ)).stdout.splitlines()
                          if line.startswith("  ") and line.split()}
