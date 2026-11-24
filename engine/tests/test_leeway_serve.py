"""The keeper keeps one `leeway serve` of the located helper running, with a fake helper and no real data folder."""

import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

from lock_support import observe_contention

import pytest

from sakura import engine_home, keeper, leeway_serve, programs
from sakura.usage import leeway

from leeway_support import calls, make_helper

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)


def _command(helper):
    return lambda: leeway_serve.Command((str(helper), "serve"), programs.environment("system", str(helper)))


def _serves(helper):
    return [call for call in calls(helper) if call == ["serve"]]


def _pid():
    return int((engine_home.root() / leeway_serve.PID_FILE).read_text())


def _alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


@pytest.fixture
def kept(tmp_path):
    """The serves started by a test, stopped at its end so a failure leaves no helper running."""
    started = []

    def make(helper):
        serve = leeway_serve.LeewayServe(_command(helper))
        started.append(serve)
        return serve

    yield make
    for serve in started:
        serve.stop()


def test_Keep_StartsServeOnceAndRecordsItsPid(tmp_path, kept):
    helper = make_helper(tmp_path / "helpers")
    serve = kept(helper)
    serve.keep()
    serve.keep()
    assert wait_for(lambda: len(_serves(helper)) == 1) and _alive(_pid())
    assert len(_serves(helper)) == 1


def test_Keep_StartsItDetachedInItsOwnProcessGroupWithNoStdin(tmp_path, kept):
    serve = kept(make_helper(tmp_path / "helpers"))
    serve.keep()
    pid = _pid()
    assert os.getpgid(pid) == pid != os.getpgrp()
    assert (engine_home.root() / leeway_serve.PID_FILE).stat().st_mode & 0o777 == 0o600


def test_Keep_ServeEnds_StartsItAgain(tmp_path, kept):
    helper = make_helper(tmp_path / "helpers", serve_exits=True)
    serve = kept(helper)
    serve.keep()
    assert wait_for(lambda: len(_serves(helper)) == 1)
    assert wait_for(lambda: (serve.keep(), len(_serves(helper)) >= 2)[1])


@pytest.fixture
def launches(monkeypatch):
    launched = []
    start = leeway_serve._start

    def record(command):
        child = start(command)
        launched.append((child, command.argv))
        return child

    def is_running(pid, argv):
        return any(child.pid == pid and child.poll() is None and command == argv for child, command in launched)

    monkeypatch.setattr(leeway_serve, "_start", record)
    monkeypatch.setattr(leeway_serve, "_is_running", is_running)
    return launched


def test_Keep_TwoKeepers_StartOneServe(tmp_path, kept, launches):
    helper = make_helper(tmp_path / "helpers")
    first, second = kept(helper), kept(helper)
    first.keep()
    second.keep()
    assert [(child.pid, argv) for child, argv in launches] == [(_pid(), (str(helper), "serve"))]
    assert wait_for(lambda: len(_serves(helper)) == 1)


def test_Keep_TwoKeepersAtOnce_StartOneServe(tmp_path, kept, launches, monkeypatch):
    helper = make_helper(tmp_path / "helpers")
    first, second = kept(helper), kept(helper)
    starting = threading.Event()
    release = threading.Event()
    start = leeway_serve._start
    blocked = observe_contention(monkeypatch, leeway_serve, {"second-keeper"})

    def hold_first(command):
        if not starting.is_set():
            starting.set()
            assert release.wait(15)
        return start(command)

    monkeypatch.setattr(leeway_serve, "_start", hold_first)
    with ThreadPoolExecutor(max_workers=1) as worker:
        started = worker.submit(first.keep)
        contender = threading.Thread(name="second-keeper", target=second.keep)
        try:
            assert starting.wait(10)
            contender.start()
            assert blocked.acquire(timeout=10)
        finally:
            release.set()
            if contender.ident is not None:
                contender.join(10)
        assert not contender.is_alive()
        started.result(timeout=10)
    assert [(child.pid, argv) for child, argv in launches] == [(_pid(), (str(helper), "serve"))]
    assert wait_for(lambda: len(_serves(helper)) == 1)


def test_Keep_ARestartedKeeper_AdoptsTheServeItLeftRunning(tmp_path, kept):
    helper = make_helper(tmp_path / "helpers")
    before = kept(helper)
    before.keep()
    pid = _pid()
    after = kept(helper)
    after.keep()
    assert _pid() == pid and wait_for(lambda: len(_serves(helper)) == 1)
    after.stop()
    assert _alive(pid)


def test_Keep_NoHelperFound_StartsNothing(tmp_path):
    serve = leeway_serve.LeewayServe(lambda: None)
    serve.keep()
    assert not (engine_home.root() / leeway_serve.PID_FILE).exists()


def test_Stop_StopsOnlyTheServeItStartedByPid(tmp_path, kept):
    serve = kept(make_helper(tmp_path / "helpers"))
    serve.keep()
    pid = _pid()
    serve.stop()
    assert wait_for(lambda: not _alive(pid)) and not (engine_home.root() / leeway_serve.PID_FILE).exists()


def test_Stop_APidFileOfAProcessThatIsNotServe_IsNeverSignalled(tmp_path):
    (engine_home.root() / leeway_serve.PID_FILE).write_text(str(os.getpid()))
    serve = leeway_serve.LeewayServe(_command(make_helper(tmp_path / "helpers")))
    serve.stop()
    serve.keep()
    assert _alive(os.getpid()) and _pid() != os.getpid()
    serve.stop()


def test_Serve_RunsTheServeForAsLongAsTheKeeperRunsAndStopsItWithIt(tmp_path, kept):
    helper = make_helper(tmp_path / "helpers")
    serve = kept(helper)
    ports = keeper.Ports(repos=lambda: [], is_alive=lambda entry: False, resume=lambda *_: None, lanes=lambda: 3,
                         leeway_serve=serve)
    seen = []
    rounds = iter([False, True])

    def wait(seconds):
        seen.append(_pid())
        return next(rounds)

    keeper.serve(ports, now=lambda: NOW, phone_page=None, wait=wait)
    assert len(seen) == 2 and seen[0] == seen[1]
    assert wait_for(lambda: not _alive(seen[0]))


def test_Serve_AServeThatCannotBeStarted_IsReportedAndTheKeeperGoesOn(tmp_path, capsys):
    broken = leeway_serve.LeewayServe(lambda: leeway_serve.Command((str(tmp_path / "missing"), "serve"), {}))
    ports = keeper.Ports(repos=lambda: [], is_alive=lambda entry: False, resume=lambda *_: None, lanes=lambda: 3,
                         leeway_serve=broken)
    keeper.serve(ports, now=lambda: NOW, phone_page=None, wait=lambda seconds: True)
    assert "leeway serve" in capsys.readouterr().err


def test_HelperCommand_NoHelperFound_IsNone():
    assert leeway_serve.helper_command() is None


def test_HelperCommand_RunsTheLocatedHelperWithItsServeVerb(tmp_path, monkeypatch):
    helper = make_helper(tmp_path / "helpers")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    leeway.forget()
    command = leeway_serve.helper_command()
    assert command.argv == (str(helper), "serve") and "HOME" in command.env


def wait_for(condition, seconds=5.0):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(0.02)
    return False
