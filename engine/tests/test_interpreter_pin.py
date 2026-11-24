"""Engine re-entries run under the interpreter the engine itself runs under, never a `python3` found on PATH."""

from __future__ import annotations

import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest
from tmux_support import needs_tmux, private_tmux, wait_until

from sakura import interpreter
from sakura.flow import surface
from sakura.flow.store import TaskStore

KIT = Path(__file__).resolve().parents[1]
SAKURA = KIT / "bin" / "sakura"


def _stub_python(directory: Path) -> Path:
    """A `python3` that claims to be too old: it leaves a marker file and exits 97."""
    directory.mkdir(parents=True, exist_ok=True)
    stub = directory / "python3"
    stub.write_text(f"#!/bin/sh\ntouch {directory}/stub-was-run\nexit 97\n")
    stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
    return stub


def _run(argv, env):
    return subprocess.run(argv, capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL, timeout=60)


def test_pinned_is_the_absolute_resolved_interpreter_of_this_process():
    assert interpreter.pinned() == os.path.realpath(sys.executable)
    assert os.path.isabs(interpreter.pinned())


def test_pinned_refuses_an_interpreter_older_than_the_minimum(monkeypatch):
    monkeypatch.setattr(interpreter, "MINIMUM_VERSION", (99, 0))
    with pytest.raises(interpreter.InterpreterError, match="needs Python 99.0 or later"):
        interpreter.pinned()


def test_window_environment_carries_the_pinned_interpreter_whatever_the_caller_has():
    shown = surface.TmuxSurface(client=None, prefix="flow-", environ={"HOME": "/h", "PATH": "/usr/bin",
                                                                   interpreter.ENV_NAME: "/old/python3"})
    assert shown._window_env()[interpreter.ENV_NAME] == interpreter.pinned()


def test_the_kit_command_ignores_a_too_old_python3_first_on_path(tmp_path):
    stub = _stub_python(tmp_path / "stubbin")
    env = {"HOME": str(tmp_path), "PATH": f"{stub.parent}:{os.environ['PATH']}", **interpreter.environment()}
    done = _run([str(SAKURA), "flow", "--help"], env)
    assert done.returncode == 0, done.stderr
    assert not (stub.parent / "stub-was-run").exists()


@needs_tmux
def test_detached_window_runs_the_kit_with_the_pinned_interpreter_not_the_one_on_path(tmp_path, repo):
    stub = _stub_python(tmp_path / "stubbin")
    probe = tmp_path / "probe"
    probe.write_text(f'#!/bin/sh\nprintf "%s" "${interpreter.ENV_NAME}" > {tmp_path}/seen\nexit 0\n')
    probe.chmod(0o755)
    with private_tmux() as client:
        shown = surface.TmuxSurface(client, "flow-", environ={"HOME": str(tmp_path), "PATH": f"{stub.parent}:/usr/bin"})
        task = TaskStore(repo, "demo")
        shown.start_engine(task, [str(probe)], {"PATH": f"{stub.parent}:/usr/bin", **interpreter.environment()})
        assert wait_until(lambda: (tmp_path / "seen").exists())
    assert (tmp_path / "seen").read_text() == interpreter.pinned()


def test_the_kit_command_names_the_found_version_and_the_needed_one_when_python3_is_too_old(tmp_path):
    fake = tmp_path / "python3"
    fake.write_text(
        f"#!/bin/sh\nexec '{sys.executable}' \"$1\" \"$2\" \"$3\" "
        "'import sys; sys.version_info = (3, 9); exec(sys.argv.pop(1))' \"$4\" \"$5\" \"$6\" \"$7\"\n"
    )
    fake.chmod(0o700)
    (tmp_path / "dirname").symlink_to("/usr/bin/dirname")
    env = {"HOME": str(tmp_path), "PATH": str(tmp_path)}
    done = _run([str(SAKURA), "flow", "--help"], env)
    assert done.returncode != 0
    assert done.stdout == ""
    assert done.stderr.count("\n") == 1
    assert "3.9" in done.stderr and "3.11" in done.stderr and "python3" in done.stderr


def test_a_roles_environment_carries_the_pinned_interpreter_for_the_hooks_and_commands_it_runs(repo):
    from flow_support import make_context
    from sakura.flow import roles, runner

    ctx = make_context(repo)
    agent = runner.AgentRunner(ctx.store, ctx.config)
    request = runner.RunRequest("builder", "build", "x", {}, repo, {})
    spec = roles.parse_role(ctx.config.values["roles"]["builder"])
    env = agent._environment(request, "build-1", ctx.config, spec)
    assert env[interpreter.ENV_NAME] == interpreter.pinned()


# -- the test interpreter, when the process runs under it, reaches the engine's own children too

FIXTURE = KIT / "tests" / "fixtures" / "trust_root_python"


def test_a_test_interpreter_the_engine_was_started_with_is_handed_to_its_children(monkeypatch, tmp_path):
    monkeypatch.setenv(interpreter.ENV_NAME, str(FIXTURE))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))
    monkeypatch.setenv("SAKURA_TEST_LIB", str(KIT / "lib"))

    handed = interpreter.environment()

    assert handed == {interpreter.ENV_NAME: str(FIXTURE), "SAKURA_TEST_TRUST_ROOT": str(tmp_path / "trust"),
                      "SAKURA_TEST_LIB": str(KIT / "lib")}


def test_without_the_test_trust_root_the_engine_still_pins_its_own_interpreter(monkeypatch):
    monkeypatch.setenv(interpreter.ENV_NAME, str(FIXTURE))
    monkeypatch.delenv("SAKURA_TEST_TRUST_ROOT", raising=False)

    assert interpreter.environment() == {interpreter.ENV_NAME: interpreter.pinned()}


def test_a_roles_and_a_workers_environment_carry_the_test_interpreter_and_its_trust_root(repo, monkeypatch, tmp_path):
    from flow_support import make_context
    from sakura.flow import roles, runner

    monkeypatch.setenv(interpreter.ENV_NAME, str(FIXTURE))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))
    monkeypatch.setenv("SAKURA_TEST_LIB", str(KIT / "lib"))
    ctx = make_context(repo)
    agent = runner.AgentRunner(ctx.store, ctx.config)
    spec = roles.parse_role(ctx.config.values["roles"]["builder"])

    role_env = agent._environment(runner.RunRequest("builder", "build", "x", {}, repo, {}), "build-1", ctx.config, spec)
    worker_env = {**surface.driver_environment(os.environ), **interpreter.environment()}

    for env in (role_env, worker_env):
        assert env[interpreter.ENV_NAME] == str(FIXTURE) and env["SAKURA_TEST_TRUST_ROOT"] == str(tmp_path / "trust")


def test_the_test_interpreter_keeps_trust_records_of_a_grandchild_in_the_run_trust_root(monkeypatch, tmp_path):
    monkeypatch.setenv(interpreter.ENV_NAME, str(FIXTURE))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))
    monkeypatch.setenv("SAKURA_TEST_LIB", str(KIT / "lib"))
    first = {**os.environ, **interpreter.environment()}
    probe = ("import json, os; from sakura import engine_home, interpreter; "
             "print(json.dumps({'trust': str(engine_home.root_for_trust()), 'child': interpreter.environment()}))")

    done = subprocess.run([first[interpreter.ENV_NAME], "-c", probe], env=first, capture_output=True, text=True, check=True)

    import json
    seen = json.loads(done.stdout)
    assert seen["trust"] == str(tmp_path / "trust")
    assert seen["child"][interpreter.ENV_NAME] == str(FIXTURE), "and it hands the same interpreter on again"


def test_only_the_test_interpreter_is_handed_on_whatever_the_variable_names(monkeypatch, tmp_path):
    other = tmp_path / "some-other-python"
    other.write_text("#!/bin/sh\n")
    monkeypatch.setenv(interpreter.ENV_NAME, str(other))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))

    assert interpreter.environment() == {interpreter.ENV_NAME: interpreter.pinned()}
