"""Detach, attach and open: the tmux commands of `sakura flow`, against private tmux servers only."""

from __future__ import annotations

import hashlib
import os
import re
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import SimpleNamespace

import pytest
from flow_support import make_context
from lock_support import observe_contention
from tmux_support import needs_tmux, private_tmux, wait_until

from sakura import engine_home, interpreter
from sakura import install, repository
from sakura.flow import cli, exits, render, store, surface, tasks


SESSION_ID = "9b5c773d-e4a1-4311-bca3-43830f5ff541"
CODEX_ID = "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"
NEEDS_TMUX = "needs tmux (surface.tmux = true and tmux on PATH)"
ACTOR = "cli:tester"


@pytest.fixture
def records(tmp_path):
    path = tmp_path / "records"
    path.mkdir()
    return path


def write_shim(path: Path, records: Path, name: str | None = None) -> Path:
    """An executable that records its argv, environment and directory, then holds (the ledger follower always, others if `hold` exists) or exits.

    The kit shim (`name` None) is named by its first two arguments, e.g. flow-resume; an agent shim by `name`.
    """
    stem = '"$1-$2"' if name is None else name
    path.write_text(f"#!/bin/sh\nstem={stem}\nd={records}\nprintf '%s\\n' \"$@\" > \"$d/$stem.argv.tmp\"\n"
                    f"mv \"$d/$stem.argv.tmp\" \"$d/$stem.argv\"\n"
                    f"env > \"$d/$stem.env.tmp\"\nmv \"$d/$stem.env.tmp\" \"$d/$stem.env\"\n"
                    f"pwd > \"$d/$stem.cwd.tmp\"\nmv \"$d/$stem.cwd.tmp\" \"$d/$stem.cwd\"\n"
                    f"[ -e \"$d/hold\" ] || [ \"$2\" = events ] && exec sleep 600\nexit 0\n")
    path.chmod(0o755)
    return path


@pytest.fixture
def kit_shim(tmp_path, records, monkeypatch):
    """Stand in for bin/sakura: the engine and events windows run it, and it only records what it got."""
    shim = write_shim(tmp_path / "kit", records)
    monkeypatch.setattr(surface, "KIT_BIN", shim)
    return shim


@pytest.fixture
def project(repo, monkeypatch):
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SAKURA_ACTOR", raising=False)
    return make_context(repo).store


@pytest.fixture
def private_cli(monkeypatch):
    """The cli talks to a private tmux server; `entered` collects what attach would have run."""
    entered = []
    with private_tmux() as client:
        monkeypatch.setattr(cli, "_new_tmux_client", lambda: client)
        monkeypatch.setattr(cli, "_enter_session", lambda argv: entered.append(argv) or exits.OK)
        yield client, entered


def personal_config(tmp_path, text):
    path = engine_home.config_file()
    path.write_text(text)


def session_of(output: str) -> str:
    match = re.search(r"detached: (\S+);", output)
    assert match, output
    return match.group(1)


def lines_of(path: Path) -> list[str]:
    return path.read_text().splitlines()


def add_run(task_store, name="build-1", **changes):
    record = {"name": name, "role": "builder", "phase": "build", "provider": "claude", "model": "sonnet",
              "effort": "low", "host": "local", "identity": None, "cwd": str(task_store.repo),
              "env": {"SAKURA_FLOW_TASK": task_store.task_id, "SAKURA_FLOW_ROLE": "builder"},
              "transcript_root": str(Path.home() / ".claude"), "state": "finished", "started_at": "2026-10-26T10:00:00+00:00",
              "ended_at": None, "exit_code": 0, "session_id": SESSION_ID}
    record.update(changes)
    task_store.update_state(lambda state: state.setdefault("runs", {}).update({name: record}))
    return record


def transcript_path(root: Path, cwd: str, session_id: str = SESSION_ID) -> Path:
    return root / "projects" / re.sub(r"[^A-Za-z0-9]", "-", cwd) / f"{session_id}.jsonl"


def window_names(client, session):
    return sorted(window.name for window in client.windows(session))


def ledger(task_store):
    return task_store.ledger.read_text()


# -- --detach -----------------------------------------------------------------


@needs_tmux
def test_detach_opens_engine_window(project, private_cli, kit_shim, records, capsys):
    client, _ = private_cli

    assert cli.main(["resume", "demo", "--detach", f"--actor={ACTOR}"]) == exits.OK

    session = session_of(capsys.readouterr().out)
    assert client.has_session(session)
    assert window_names(client, session) == ["engine", "events"]
    assert wait_until(lambda: (records / "flow-resume.argv").exists())
    assert lines_of(records / "flow-resume.argv") == ["flow", "resume", f"--actor={ACTOR}", "--", "demo"]
    assert wait_until(lambda: (records / "flow-events.argv").exists())
    assert "--wait" in lines_of(records / "flow-events.argv")
    engine = next(window for window in client.windows(session) if window.name == "engine")
    assert engine.options["@sakura-engine"] == "demo"


@needs_tmux
def test_detach_windows_get_the_pinned_interpreter_even_when_the_callers_environment_names_another(
        project, private_cli, kit_shim, records, monkeypatch, capsys):
    monkeypatch.setenv(interpreter.ENV_NAME, "/old/python3")
    assert cli.main(["resume", "demo", "--detach", f"--actor={ACTOR}"]) == exits.OK
    pinned = f"{interpreter.ENV_NAME}={interpreter.pinned()}"
    assert wait_until(lambda: (records / "flow-resume.env").exists() and (records / "flow-events.env").exists())
    assert pinned in lines_of(records / "flow-resume.env")
    assert pinned in lines_of(records / "flow-events.env")


@needs_tmux
def test_detach_prints_the_attach_command(project, private_cli, kit_shim, capsys):
    assert cli.main(["resume", "demo", "--detach"]) == exits.OK
    out = capsys.readouterr().out
    assert out == f"detached: {session_of(out)}; sakura flow attach demo\n"


def _task_file(project, name="specs/add-greeting.md", text="# Add a greeting\n\n## Journeys\n1. greet\n"):
    path = project.repo / name
    path.parent.mkdir(exist_ok=True)
    path.write_text(text)
    return path


@needs_tmux
def test_detach_start_creates_the_task_and_its_window_resumes_that_task(project, private_cli, kit_shim, records, capsys):
    _task_file(project)
    argv = ["start", "specs/add-greeting.md", "--new", "--detach", "--preset", "standard", "--dial", "lean",
            "--role", "builder=exec:true", "--set", "timeouts.build=60", "--relax", "preset",
            "--familiarity", "expert", f"--actor={ACTOR}"]
    assert cli.main(argv) == exits.OK
    session = session_of(capsys.readouterr().out)
    assert session.startswith("flow-") and "add-greeting" in session
    assert wait_until(lambda: (records / "flow-resume.argv").exists())
    assert lines_of(records / "flow-resume.argv") == ["flow", "resume", f"--actor={ACTOR}", "--continues-start", "--",
                                                    "add-greeting"]
    created = store.TaskStore(project.repo, "add-greeting")
    state = created.read_state()
    assert state["ref"] == "specs/add-greeting.md" and state["dial"] == "lean" and state["familiarity"] == "expert"
    assert state["overrides"]["timeouts"]["build"] == 60 and state["preset"] == "standard"
    assert "start requested" in ledger(created)
    assert state[tasks.PENDING_START_KEY] is True


def _mark_pending_start(task_store):
    task_store.update_state(lambda state: state.update({tasks.PENDING_START_KEY: True}))


def test_ResumeContinuingAStart_WritesNoResumeLine(project, monkeypatch):
    task = store.TaskStore(project.repo, "demo")
    _mark_pending_start(task)
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: exits.OK)
    before = ledger(task)

    assert cli.main(["resume", "demo", "--continues-start", f"--actor={ACTOR}"]) == exits.OK

    assert "resume requested" not in ledger(task)[len(before):]


def test_ResumeContinuingAStart_UsedTwice_RefusesTheSecondInPlainWords(project, monkeypatch, capsys):
    task = store.TaskStore(project.repo, "demo")
    monkeypatch.setattr(cli.engine, "advance", lambda ctx: exits.OK)
    _mark_pending_start(task)
    assert cli.main(["resume", "demo", "--continues-start", f"--actor={ACTOR}"]) == exits.OK
    capsys.readouterr()
    before = ledger(task)

    assert cli.main(["resume", "demo", "--continues-start", f"--actor={ACTOR}"]) == exits.USAGE

    assert "no start of demo is waiting to continue" in capsys.readouterr().out
    assert ledger(task) == before


def test_ResumeContinuingAStart_ForATaskWithNoPendingStart_RefusesAndRecordsNothing(project, capsys):
    task = store.TaskStore(project.repo, "demo")
    before = ledger(task)

    assert cli.main(["resume", "demo", "--continues-start", f"--actor={ACTOR}"]) == exits.USAGE

    assert "no start of demo is waiting to continue" in capsys.readouterr().out
    assert ledger(task) == before


def test_ResumeContinuingAStart_AfterAnotherDriverRan_RefusesAsStale(project, monkeypatch, capsys):
    task = store.TaskStore(project.repo, "demo")
    _mark_pending_start(task)
    monkeypatch.setattr(cli.engine, "advance", lambda ctx: exits.OK)
    assert cli.main(["resume", "demo", f"--actor={ACTOR}"]) == exits.OK
    capsys.readouterr()
    before = ledger(task)

    assert cli.main(["resume", "demo", "--continues-start", f"--actor={ACTOR}"]) == exits.USAGE

    assert "no start of demo is waiting to continue" in capsys.readouterr().out
    assert ledger(task) == before


def test_ResumeContinuingAStart_WithAStoppedRevision_IsRefused(project, capsys):
    task = store.TaskStore(project.repo, "demo")
    _mark_pending_start(task)

    code = cli.main(["resume", "demo", "--continues-start", "--stopped-revision=abc", f"--actor={ACTOR}"])

    assert code == exits.USAGE
    assert task.read_state().get(tasks.PENDING_START_KEY) is True


def test_ResumeByThePerson_StillWritesItsLine(project, monkeypatch):
    task = store.TaskStore(project.repo, "demo")
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: exits.OK)

    assert cli.main(["resume", "demo", f"--actor={ACTOR}"]) == exits.OK

    assert f"resume requested actor={ACTOR}" in ledger(task)


@needs_tmux
def test_detach_resume_by_the_person_does_not_claim_to_continue_a_start(project, private_cli, kit_shim, records, capsys):
    assert cli.main(["resume", "demo", "--detach", f"--actor={ACTOR}"]) == exits.OK
    assert wait_until(lambda: (records / "flow-resume.argv").exists())
    assert "--continues-start" not in lines_of(records / "flow-resume.argv")


@needs_tmux
def test_detach_start_keeps_the_task_file_it_captured_when_the_file_is_replaced_afterwards(
        project, private_cli, kit_shim, records, capsys):
    path = _task_file(project, text="# Original task\n")
    assert cli.main(["start", "specs/add-greeting.md", "--detach", f"--actor={ACTOR}"]) == exits.OK
    capsys.readouterr()
    path.write_text("# A different task swapped in by a workspace process\n")
    assert wait_until(lambda: (records / "flow-resume.argv").exists())
    created = store.TaskStore(project.repo, "add-greeting")
    assert created.input.read_text() == "# Original task\n"
    assert created.read_state()[tasks.INPUT_DIGEST_KEY] == hashlib.sha256(b"# Original task\n").hexdigest()
    assert "specs/add-greeting.md" not in " ".join(lines_of(records / "flow-resume.argv"))


@needs_tmux
def test_detach_start_when_cheap_is_stored_in_the_task(project, private_cli, kit_shim, capsys):
    _task_file(project)
    assert cli.main(["start", "specs/add-greeting.md", "--when-cheap", "--detach"]) == exits.OK
    assert store.TaskStore(project.repo, "add-greeting").read_state()["urgent"] is False


@needs_tmux
def test_detach_start_of_task_text_stores_the_text_and_resumes_by_id(project, private_cli, kit_shim, records, capsys):
    assert cli.main(["start", "-m", "Add a greeting to the CLI", "--detach", f"--actor={ACTOR}"]) == exits.OK
    session_of(capsys.readouterr().out)
    assert wait_until(lambda: (records / "flow-resume.argv").exists())
    task_id = lines_of(records / "flow-resume.argv")[-1]
    assert store.TaskStore(project.repo, task_id).input.read_text() == "Add a greeting to the CLI\n"


def test_rebuilt_commands_parse_back_to_the_same_arguments():
    approve = cli._parser().parse_args(["approve", "--continue", f"--actor={ACTOR}", "--", "demo", "spec"])
    assert (approve.task, approve.gate, approve.continue_after, approve.detach) == ("demo", "spec", True, False)
    resume = cli._parser().parse_args(["resume", f"--actor={ACTOR}", "--", "demo"])
    assert (resume.task, resume.actor, resume.detach) == ("demo", ACTOR, False)


@needs_tmux
def test_detach_start_of_a_known_ref_uses_the_existing_task(project, private_cli, kit_shim, capsys):
    assert cli.main(["start", "specs/demo.md", "--detach"]) == exits.OK
    assert "sakura flow attach demo" in capsys.readouterr().out


@needs_tmux
def test_detach_approve_continue_rebuilds_the_command(project, private_cli, kit_shim, records, capsys):
    project.spec.write_text("# S\n\n## Journeys\n1. go\n")
    # The task waits at the spec gate, as the engine records it: an approval is accepted only then.
    project.update_state(lambda state: state.update(
        status="awaiting_approval", reason="awaiting approval: sakura flow approve demo spec"))
    assert cli.main(["approve", "demo", "spec", "--continue", "--detach", f"--actor={ACTOR}"]) == exits.OK
    capsys.readouterr()
    assert wait_until(lambda: (records / "flow-approve.argv").exists())
    assert lines_of(records / "flow-approve.argv") == ["flow", "approve", "--continue", f"--actor={ACTOR}", "--", "demo", "spec"]
    assert "spec approved" not in ledger(project), "the engine window records the approval, not detach"


def test_detach_approve_needs_continue(project, capsys):
    assert cli.main(["approve", "demo", "spec", "--detach"]) == exits.USAGE
    assert "--detach needs --continue" in capsys.readouterr().out


@needs_tmux
def test_detach_approve_refuses_a_missing_spec_before_any_window(project, private_cli, kit_shim, capsys):
    client, _ = private_cli
    assert cli.main(["approve", "demo", "spec", "--continue", "--detach"]) == exits.USAGE
    assert "cannot approve demo" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))


@pytest.mark.parametrize("command", [["resume", "demo", "--detach"], ["start", "specs/new.md", "--detach"],
                                     ["approve", "demo", "spec", "--continue", "--detach"], ["attach", "demo"],
                                     ["open", "demo", "build-1"]])
@pytest.mark.parametrize("reason", ["disabled", "absent"])
def test_detach_without_tmux_usage(project, tmp_path, monkeypatch, capsys, command, reason):
    if reason == "disabled":
        personal_config(tmp_path, "[surface]\ntmux = false\n")
    else:
        monkeypatch.setenv("PATH", str(tmp_path))
    before = sorted(path.name for path in store.flow_root(project.repo).iterdir())

    assert cli.main(command) == exits.USAGE

    assert NEEDS_TMUX in capsys.readouterr().out
    assert sorted(path.name for path in store.flow_root(project.repo).iterdir()) == before


@needs_tmux
def test_second_detached_driver_busy(project, private_cli, kit_shim, records, capsys):
    client, _ = private_cli
    (records / "hold").write_text("")
    assert cli.main(["resume", "demo", "--detach"]) == exits.OK
    session = session_of(capsys.readouterr().out)

    assert cli.main(["resume", "demo", "--detach"]) == exits.BUSY

    assert "already running" in capsys.readouterr().out
    assert window_names(client, session).count("engine") == 1


@needs_tmux
def test_detach_while_a_foreground_driver_holds_the_lock_is_busy(project, private_cli, kit_shim, capsys):
    client, _ = private_cli
    with project.lock():
        assert cli.main(["resume", "demo", "--detach"]) == exits.BUSY
    assert "engine already running" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))


@needs_tmux
def test_detach_replaces_a_driver_window_that_has_ended(project, private_cli, kit_shim, records, capsys):
    client, _ = private_cli
    assert cli.main(["resume", "demo", "--detach"]) == exits.OK
    session = session_of(capsys.readouterr().out)
    assert wait_until(lambda: next(w for w in client.windows(session) if w.name == "engine").dead)
    first = next(w for w in client.windows(session) if w.name == "engine").id

    assert cli.main(["resume", "demo", "--detach"]) == exits.OK

    engines = [w for w in client.windows(session) if w.name == "engine"]
    assert len(engines) == 1 and engines[0].id != first


@needs_tmux
def test_detach_refuses_a_foreign_session_with_the_task_name(project, private_cli, kit_shim, capsys):
    client, _ = private_cli
    name = surface.session_name("flow-", project.repo, "demo")
    client.ensure_session(name, project.repo, "x", ["sleep", "600"], {}, ("@sakura-task", "someone-else"))

    assert cli.main(["resume", "demo", "--detach"]) == exits.FAILED

    assert "belongs to something else" in capsys.readouterr().out
    assert window_names(client, name) == ["x"]


def test_engine_environment_drops_tmux_variables_and_unsafe_names():
    environment = {"TMUX": "/s,1,0", "TMUX_PANE": "%1", "HOME": "/h", "PATH": "/bin", "SAKURA_MARK": "x",
                   "BASH_FUNC_f%%": "() { :; }", "1BAD": "no"}
    assert surface.driver_environment(environment) == {"HOME": "/h", "PATH": "/bin", "SAKURA_MARK": "x"}


# -- events --wait ------------------------------------------------------------


def test_await_task_returns_once_the_task_is_created(repo, monkeypatch):
    monkeypatch.setattr(cli, "EVENTS_POLL_SECONDS", 0.01)
    created = store.TaskStore(repo, "later")
    threading.Timer(0.1, lambda: created.create({"task": "later", "status": "running"})).start()
    assert cli._await_task(repo, "later").task_id == "later"


def test_events_wait_gives_up_on_a_task_that_never_appears(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "EVENTS_WAIT_SECONDS", 0.05)
    monkeypatch.setattr(cli, "EVENTS_POLL_SECONDS", 0.01)
    assert cli.main(["events", "never", "--wait"]) == exits.USAGE
    assert "unknown task never" in capsys.readouterr().out


def test_events_wait_on_an_existing_task_follows_it(project, monkeypatch, capsys):
    project.append_ledger("engine", "hello")

    def interrupt(_seconds):
        raise KeyboardInterrupt

    monkeypatch.setattr(render.time, "sleep", interrupt)
    assert cli.main(["events", "demo", "--wait"]) == exits.OK
    assert "- [engine] hello" in capsys.readouterr().out


# -- attach -------------------------------------------------------------------


@needs_tmux
def test_attach_rebuilds_missing_windows(project, private_cli, kit_shim):
    client, entered = private_cli
    add_run(project, "plan-1", role="planner", state="finished")
    add_run(project, "build-1", state="running")
    add_run(project, "build-2", state="starting")
    session = surface.session_name("flow-", project.repo, "demo")

    assert cli.main(["attach", "demo"]) == exits.OK

    assert window_names(client, session) == ["build-1-builder", "build-2-builder", "events"]
    assert entered == [client.switch_or_attach(session)]

    events = next(window for window in client.windows(session) if window.name == "events")
    client.kill_window(events.id)
    watch = next(window for window in client.windows(session) if window.name == "build-1-builder")
    client.kill_window(watch.id)
    assert cli.main(["attach", "demo"]) == exits.OK
    assert window_names(client, session) == ["build-1-builder", "build-2-builder", "events"]
    assert len(entered) == 2


@needs_tmux
def test_attach_does_not_duplicate_windows_that_exist(project, private_cli, kit_shim):
    client, _ = private_cli
    add_run(project, "build-1", state="running")
    session = surface.session_name("flow-", project.repo, "demo")
    for _ in range(2):
        assert cli.main(["attach", "demo"]) == exits.OK
    assert window_names(client, session) == ["build-1-builder", "events"]


@needs_tmux
def test_attach_refuses_a_foreign_session_and_unknown_task(project, private_cli, kit_shim, capsys):
    client, entered = private_cli
    name = surface.session_name("flow-", project.repo, "demo")
    client.ensure_session(name, project.repo, "x", ["sleep", "600"], {}, ("@sakura-task", "someone-else"))
    assert cli.main(["attach", "demo"]) == exits.FAILED
    assert cli.main(["attach", "nope"]) == exits.USAGE
    assert entered == [] and window_names(client, name) == ["x"]


def test_attach_reports_the_exit_of_the_terminal_command(project, monkeypatch):
    monkeypatch.setattr(cli, "_new_tmux_client", lambda: _NoServerClient())
    monkeypatch.setattr(cli, "_enter_session", lambda argv: exits.FAILED)
    assert cli.main(["attach", "demo"]) == exits.FAILED


class _NoServerClient:
    """Enough of a TmuxClient for attach to reach the terminal step without a server."""

    def available(self):
        return True

    def socket_path_problem(self):
        return ""

    def has_session(self, name):
        return True

    def option(self, target, key):
        return f"{key}"

    def ensure_session(self, *args):
        return True

    def windows(self, session):
        return []

    def new_window(self, *args, **kwargs):
        return "@1"

    def set_option(self, *args):
        return None

    def switch_or_attach(self, session):
        return ["tmux", "attach-session", "-t", session]


# -- open ---------------------------------------------------------------------


@pytest.fixture
def agent_shims(tmp_path, records, monkeypatch):
    """`claude` and `codex` on PATH that record how they were started."""
    directory = tmp_path / "agents"
    directory.mkdir()
    for name in ("claude", "codex"):
        write_shim(directory / name, records, name)
    monkeypatch.setenv("PATH", f"{directory}:{os.environ['PATH']}")
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    (records / "hold").write_text("")


@needs_tmux
def test_open_refuses_running(project, private_cli, capsys):
    client, _ = private_cli
    add_run(project, "build-1", state="running")
    before = ledger(project)

    assert cli.main(["open", "demo", "build-1"]) == exits.USAGE

    assert "stop it first" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))
    assert ledger(project) == before


@needs_tmux
def test_open_refuses_an_unknown_run_and_hostile_names(project, private_cli, capsys):
    add_run(project, "build-1")
    assert cli.main(["open", "demo", "build-9"]) == exits.USAGE
    assert cli.main(["open", "demo", "../x\x1b]2;owned\x07"]) == exits.USAGE
    out = capsys.readouterr().out
    assert "build-9" in out and "\x1b" not in out and "\x07" not in out


@needs_tmux
def test_open_uses_record_cwd_env_and_safe_argv(project, private_cli, agent_shims, records, tmp_path, capsys,
                                                monkeypatch):
    client, _ = private_cli
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    project.update_state(lambda state: state.update({"workspaces": {"0": {"path": str(workspace)}}}))
    config_dir = tmp_path / "claude-config"
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(config_dir))
    record = add_run(project, "build-1", cwd=str(workspace.resolve()), transcript_root=str(config_dir.resolve()))
    transcript = transcript_path(config_dir.resolve(), record["cwd"])
    transcript.parent.mkdir(parents=True)
    transcript.write_text("{}\n")

    assert cli.main(["open", "demo", "build-1"]) == exits.OK

    session = surface.session_name("flow-", project.repo, "demo")
    assert "build-1-open" in window_names(client, session)
    assert wait_until(lambda: (records / "claude.cwd").exists())
    assert lines_of(records / "claude.argv") == ["--resume", SESSION_ID, "--model", "sonnet", "--add-dir",
                                                 str(repository.common_dir(project.repo))]
    assert not any("dangerously" in line for line in lines_of(records / "claude.argv"))
    assert Path(lines_of(records / "claude.cwd")[0]).resolve() == workspace.resolve()
    environment = lines_of(records / "claude.env")
    assert "SAKURA_FLOW_TASK=demo" in environment and "SAKURA_FLOW_ROLE=builder" in environment
    assert f"CLAUDE_CONFIG_DIR={config_dir.resolve()}" in environment
    assert f"owner opened build-1 (claude {SESSION_ID})" in ledger(project)


@needs_tmux
def test_open_default_transcript_root_sets_no_config_variable(project, private_cli, agent_shims, records):
    record = add_run(project, "build-1")
    transcript = transcript_path(Path.home() / ".claude", record["cwd"])
    transcript.parent.mkdir(parents=True)
    transcript.write_text("{}\n")

    assert cli.main(["open", "demo", "build-1"]) == exits.OK

    assert wait_until(lambda: (records / "claude.env").exists())
    assert not any(line.startswith("CLAUDE_CONFIG_DIR=") for line in lines_of(records / "claude.env"))


@needs_tmux
def test_open_codex_uses_codex_home_and_sandbox(project, private_cli, agent_shims, records, tmp_path, monkeypatch):
    codex_home = tmp_path / "codex-home"
    monkeypatch.setenv("CODEX_HOME", str(codex_home))
    add_run(project, "build-1", provider="codex", model="gpt", session_id=CODEX_ID,
            transcript_root=str(codex_home.resolve()))

    assert cli.main(["open", "demo", "build-1"]) == exits.OK

    assert wait_until(lambda: (records / "codex.env").exists())
    assert lines_of(records / "codex.argv")[:4] == ["resume", CODEX_ID, "--sandbox", "workspace-write"]
    assert f"CODEX_HOME={codex_home.resolve()}" in lines_of(records / "codex.env")
    assert f"owner opened build-1 (codex {CODEX_ID})" in ledger(project)


@needs_tmux
def test_open_refuses_missing_transcript(project, private_cli, agent_shims, capsys):
    client, _ = private_cli
    add_run(project, "build-1")
    before = ledger(project)

    assert cli.main(["open", "demo", "build-1"]) == exits.USAGE

    assert "transcript" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))
    assert ledger(project) == before


@needs_tmux
def test_open_twice_keeps_one_window(project, private_cli, agent_shims):
    client, _ = private_cli
    record = add_run(project, "build-1")
    transcript = transcript_path(Path.home() / ".claude", record["cwd"])
    transcript.parent.mkdir(parents=True)
    transcript.write_text("{}\n")
    for _ in range(2):
        assert cli.main(["open", "demo", "build-1"]) == exits.OK
    session = surface.session_name("flow-", project.repo, "demo")
    assert window_names(client, session).count("build-1-open") == 1
    assert ledger(project).count("owner opened build-1") == 1


@needs_tmux
def test_open_refuses_a_working_directory_that_is_gone(project, private_cli, agent_shims, tmp_path, capsys):
    gone = tmp_path / "gone"
    add_run(project, "build-1", provider="codex", session_id=CODEX_ID, cwd=str(gone))
    assert cli.main(["open", "demo", "build-1"]) == exits.USAGE
    assert "no longer exists" in capsys.readouterr().out


@needs_tmux
def test_open_refuses_a_stored_config_root_the_operator_did_not_choose(project, private_cli, agent_shims, tmp_path, capsys):
    client, _ = private_cli
    add_run(project, "build-1", provider="codex", session_id=CODEX_ID, transcript_root=str(tmp_path / "planted"))
    assert cli.main(["open", "demo", "build-1"]) == exits.USAGE
    assert "another config root" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))


@needs_tmux
def test_open_refuses_a_stored_directory_outside_the_repository(project, private_cli, agent_shims, tmp_path, capsys):
    elsewhere = tmp_path / "hooked-project"
    elsewhere.mkdir()
    add_run(project, "build-1", provider="codex", session_id=CODEX_ID, cwd=str(elsewhere),
            transcript_root=str(Path.home() / ".codex"))
    assert cli.main(["open", "demo", "build-1"]) == exits.USAGE
    assert "not the repository or a recorded workspace" in capsys.readouterr().out


@pytest.mark.parametrize("command", [["attach", "{task}"], ["open", "{task}", "build-1"]])
def test_unknown_task_message_strips_controls_from_the_argument_and_the_listing(project, capsys, command):
    (store.flow_root(project.repo) / "listed\x1b]2;owned\x07").mkdir()
    hostile = "ghost\x1b]2;owned\x07"

    assert cli.main([part.format(task=hostile) for part in command]) == exits.USAGE

    out = capsys.readouterr().out
    assert "unknown task ghost" in out and "listed" in out
    assert "\x1b" not in out and "\x07" not in out


@needs_tmux
@pytest.mark.parametrize("arguments", [["open", "demo", "build-1"], ["attach", "demo"]])
def test_open_and_attach_are_refused_inside_an_engine_role(project, private_cli, kit_shim, monkeypatch, capsys, arguments):
    client, entered = private_cli
    add_run(project, "build-1")
    before = ledger(project)
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "planner")

    assert cli.main(arguments) == exits.USAGE

    assert "operator commands cannot run inside an engine role" in capsys.readouterr().out
    assert not client.has_session(surface.session_name("flow-", project.repo, "demo"))
    assert entered == [] and ledger(project) == before


# -- races between separate operator processes ----------------------------------

class WindowClient:
    def __init__(self):
        self.created = []
        self.creating = threading.Event()
        self.release = threading.Event()

    def available(self):
        return True

    def socket_path_problem(self):
        return ""

    def ensure_session(self, _session, *_args):
        return True

    def windows(self, _session):
        return self.created[:]

    def new_window(self, _session, name, *_args, **_kwargs):
        self.creating.set()
        assert self.release.wait(15)
        window = SimpleNamespace(name=name, options={}, dead=False)
        self.created.append(window)
        return window

    def set_option(self, window, key, value):
        window.options[key] = value


def race_windows(monkeypatch, arguments):
    client = WindowClient()
    monkeypatch.setattr(cli, "_new_tmux_client", lambda: client)
    blocked = observe_contention(monkeypatch, surface, {"contender"})
    codes = []
    contender = threading.Thread(name="contender", target=lambda: codes.append(cli.main(arguments)))
    with ThreadPoolExecutor(max_workers=1) as worker:
        first = worker.submit(cli.main, arguments)
        try:
            assert client.creating.wait(10)
            contender.start()
            assert blocked.acquire(timeout=10)
        finally:
            client.release.set()
            if contender.ident is not None:
                contender.join(10)
        assert not contender.is_alive()
        codes.append(first.result(timeout=10))
    return client, codes


def test_concurrent_detached_drivers_leave_one_engine_window_and_one_busy(project, monkeypatch, capsys):
    client, codes = race_windows(monkeypatch, ["resume", "demo", "--detach"])
    assert sorted(codes) == [exits.OK, exits.BUSY]
    session = surface.session_name("flow-", project.repo, "demo")
    assert window_names(client, session).count("engine") == 1
    assert capsys.readouterr().out.count("already running") == 1


def test_concurrent_open_leaves_one_window_and_one_ledger_line(project, agent_shims, monkeypatch, capsys):
    record = add_run(project, "build-1")
    transcript = transcript_path(Path.home() / ".claude", record["cwd"])
    transcript.parent.mkdir(parents=True)
    transcript.write_text("{}\n")

    client, codes = race_windows(monkeypatch, ["open", "demo", "build-1"])
    assert codes == [exits.OK, exits.OK]
    session = surface.session_name("flow-", project.repo, "demo")
    assert window_names(client, session).count("build-1-open") == 1
    assert ledger(project).count("owner opened build-1") == 1
    out = capsys.readouterr().out
    assert out.count("is already open") == 1 and out.count("opened build-1-open") == 1


@needs_tmux
def test_detach_start_with_a_socket_path_over_the_limit_fails_first_naming_the_path_the_limit_and_the_way_out(
        project, private_cli, kit_shim, tmp_path, monkeypatch, capsys):
    from sakura.flow import tmux

    deep = tmp_path / ("very-long-folder-name-" * 6)
    deep.mkdir()
    monkeypatch.setenv("TMUX_TMPDIR", str(deep))
    _task_file(project)
    code = cli.main(["start", "specs/add-greeting.md", "--detach", f"--actor={ACTOR}"])
    out = capsys.readouterr()
    text = out.out + out.err
    client, _entered = private_cli
    expected = os.path.join(os.path.realpath(deep), f"tmux-{os.getuid()}", client.socket)
    assert code != exits.OK
    assert expected in text and str(tmux.SOCKET_PATH_LIMIT) in text and "TMUX_TMPDIR" in text
    assert not store.TaskStore(project.repo, "add-greeting").exists()
