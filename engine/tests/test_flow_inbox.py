import json
import os
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from sakura import actors, install
from sakura.flow import brakes, cli, exits, inbox, store
from flow_support import make_context

LIB = str(Path(__file__).resolve().parents[1] / "lib")
ALICE = "cli:alice"
LOCK_HOLDER = """import pathlib, sys
from sakura.flow import store
with store.TaskStore(pathlib.Path(sys.argv[1]), "demo").lock():
    print("locked", flush=True)
    sys.stdin.readline()
"""
AGENT = """import os, pathlib
pathlib.Path(os.environ["SAKURA_FLOW_EXCHANGE"], "spec.md").write_text("# S\\n\\n## Journeys\\n1. go\\n")
"""


@pytest.fixture
def demo(repo, monkeypatch):
    """A running task `demo` and a working directory inside its repository."""
    install.install(repo)
    monkeypatch.chdir(repo)
    return make_context(repo)


@pytest.fixture
def project(repo, tmp_path, monkeypatch):
    (repo / "specs").mkdir()
    (repo / "specs/add-greeting.md").write_text("add a greeting\n")
    agent = tmp_path / "agent.py"
    agent.write_text(AGENT)
    install.install(repo)
    monkeypatch.chdir(repo)
    return repo, f"exec:{sys.executable} {agent}"


def _start(project, *extra):
    repo, command = project
    return cli.main(["start", "specs/add-greeting.md", "--role", f"planner={command}", "--role", "builder=exec:true", *extra])


def _commands(task_store):
    return [command for _, command in inbox.pending(task_store)]


def test_enqueue_returns_while_task_lock_held(demo, repo):
    holder = subprocess.Popen([sys.executable, "-c", LOCK_HOLDER, str(repo)], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, text=True, env={**os.environ, "PYTHONPATH": LIB})
    try:
        assert holder.stdout.readline().strip() == "locked"
        with pytest.raises(store.EngineBusy), demo.store.lock():
            pass
        with ThreadPoolExecutor(max_workers=1) as worker:
            result = worker.submit(cli.main, ["dial", "demo", "save"])
            try:
                assert result.result(timeout=10) == exits.OK
                assert holder.poll() is None
                assert [command["value"] for command in _commands(demo.store)] == ["save"]
            finally:
                holder.stdin.close()
    finally:
        holder.stdin.close()
        holder.wait(timeout=10)
    assert [command["value"] for command in _commands(demo.store)] == ["save"]


def test_concurrent_steer_loses_nothing(demo):
    threads = [threading.Thread(target=inbox.enqueue, args=(demo.store, "steer", f"note {number}"))
               for number in range(20)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert len(list(demo.store.inbox_dir.glob("*.json"))) == 20
    assert {command["value"] for command in _commands(demo.store)} == {f"note {n}" for n in range(20)}


def test_enqueue_writes_one_complete_command_file(demo):
    path = inbox.enqueue(demo.store, "role", "claude:opus", role="builder", actor=ALICE)
    command = json.loads(path.read_text())
    assert path.parent == demo.store.inbox_dir and path.name.endswith(".json")
    assert command["kind"] == "role" and command["value"] == "claude:opus" and command["role"] == "builder"
    assert command["actor"] == ALICE and command["id"] and command["at"]
    assert not list(demo.store.inbox_dir.glob("*.tmp"))


def test_pending_orders_by_enqueue_time(demo):
    for value in ("first", "second", "third"):
        inbox.enqueue(demo.store, "steer", value)
    assert [command["value"] for command in _commands(demo.store)] == ["first", "second", "third"]


def test_pending_yields_unreadable_file_as_empty_command(demo):
    demo.store.inbox_dir.mkdir()
    (demo.store.inbox_dir / "00000000000000000001-bad.json").write_text("{not json")
    assert _commands(demo.store) == [{}]


def test_dial_queues_without_touching_state(demo, capsys):
    before = demo.store.read_state()
    assert cli.main(["dial", "demo", "save", "--actor", ALICE]) == exits.OK
    assert "queued: dial for demo; applies at the next safe boundary" in capsys.readouterr().out
    assert demo.store.read_state() == before
    assert _commands(demo.store)[0]["actor"] == ALICE


def test_dial_rejects_unknown_step(demo, capsys):
    assert cli.main(["dial", "demo", "turbo"]) == exits.USAGE
    assert "turbo" in capsys.readouterr().out
    assert _commands(demo.store) == []


def test_role_codex_for_a_writer_needs_the_same_hook_trust_as_a_reviewer(demo, capsys):
    assert cli.main(["role", "demo", "builder", "codex:gpt-5"]) == exits.USAGE
    assert "Codex builder needs recorded hook trust" in capsys.readouterr().out
    assert _commands(demo.store) == []


def test_role_rejects_codex_reviewer_without_trust(demo, capsys):
    assert cli.main(["role", "demo", "plan_reviewer", "codex"]) == exits.USAGE
    assert "Codex plan_reviewer needs recorded hook trust" in capsys.readouterr().out
    assert _commands(demo.store) == []


@pytest.mark.parametrize("arguments, message", [
    (["evaluator", "codex:gpt-5"], "Codex evaluator needs recorded hook trust"),
    (["janitor", "claude:opus"], "unknown role janitor"),
    (["builder", "gemini:pro"], "unknown role provider"),
    (["plan_reviewer", "none"], "removes a gate"),
])
def test_role_rejects_invalid_values(demo, capsys, arguments, message):
    assert cli.main(["role", "demo", *arguments]) == exits.USAGE
    assert message in capsys.readouterr().out
    assert _commands(demo.store) == []


def test_role_queues_a_valid_value(demo):
    assert cli.main(["role", "demo", "builder", "claude:opus"]) == exits.OK
    command = _commands(demo.store)[0]
    assert (command["kind"], command["role"], command["value"]) == ("role", "builder", "claude:opus")


def test_steer_rejects_blank_text(demo):
    assert cli.main(["steer", "demo", "  "]) == exits.USAGE
    assert _commands(demo.store) == []


def test_commands_refuse_cleaned_task(demo, capsys):
    demo.store.update_state(lambda state: state.update(status="cleaned"))
    for arguments in (["dial", "demo", "save"], ["role", "demo", "builder", "claude:opus"], ["steer", "demo", "hi"]):
        assert cli.main(arguments) == exits.USAGE
        assert "cleaned" in capsys.readouterr().out
    assert _commands(demo.store) == []


@pytest.mark.parametrize("role", ["builder", "evaluator"])
def test_operator_commands_refused_inside_engine_role(demo, repo, monkeypatch, capsys, role):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", role)
    for arguments in (["dial", "demo", "save"], ["role", "demo", "builder", "claude:opus"], ["steer", "demo", "hi"],
                      ["approve", "demo", "spec"], ["stop", "demo"], ["start", "specs/other.md"],
                      ["resume", "demo"], ["recontract", "demo"], ["cleanup", "demo"]):
        assert cli.main(arguments) == exits.USAGE
        assert "operator commands cannot run inside an engine role" in capsys.readouterr().out
    assert _commands(demo.store) == [] and not demo.store.stop_file.exists()
    assert "requested" not in demo.store.ledger.read_text() and not store.TaskStore(repo, "other").exists()


def _ledger(repo, task="add-greeting"):
    return store.TaskStore(repo, task).ledger.read_text()


def _approved(project):
    _start(project)
    cli.main(["approve", "add-greeting", "spec"])


def _blocked_plan(project):
    from sakura.flow import contract

    repo, _ = project
    _approved(project)
    task_store = store.TaskStore(repo, "add-greeting")
    contract.save(task_store, [contract.Row("R1", "J1", "new", "cli", None, "true", False, False)])
    task_store.update_state(lambda state: state.update(phase="plan", status="blocked", reason="x",
                                                       completed={"intake": "h", "contract": "c"}))


@pytest.mark.parametrize("prepare, arguments", [
    (lambda project: None, ["start-new"]),
    (_approved, ["resume"]),
    (_approved, ["approve", "spec"]),
    (_approved, ["stop"]),
    (_blocked_plan, ["recontract"]),
    (_approved, ["cleanup"]),
])
def test_every_flow_write_ledgers_actor(project, monkeypatch, prepare, arguments):
    repo, _ = project
    prepare(project)
    monkeypatch.setattr(cli, "_advance", lambda *_, **__: exits.OK)
    if arguments == ["start-new"]:
        assert _start(project, "--actor", ALICE) == exits.OK
    else:
        if arguments == ["cleanup"]:
            store.TaskStore(repo, "add-greeting").update_state(lambda state: state.update(phase="cleanup"))
        verb, *rest = arguments
        assert cli.main([verb, "add-greeting", *rest, "--actor", ALICE]) == exits.OK
    assert any("actor=cli:alice" in line for line in _ledger(repo).splitlines())


def test_actor_defaults_to_environment_then_cli_user(project, monkeypatch):
    repo, _ = project
    monkeypatch.setenv("SAKURA_ACTOR", "cli:from-env")
    _start(project)
    assert "actor=cli:from-env" in _ledger(repo)
    monkeypatch.delenv("SAKURA_ACTOR")
    cli.main(["stop", "add-greeting"])
    assert "stop requested actor=cli:" in _ledger(repo).splitlines()[-1]


def test_queued_commands_carry_the_environment_actor(demo, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR", "overlay:bob@example.com:node1")
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # a child of the cockpit, the only process that may carry that identity
    cli.main(["steer", "demo", "hi"])
    assert _commands(demo.store)[0]["actor"] == "overlay:bob@example.com:node1"


@pytest.mark.parametrize("value", [
    "cli:alice\n", "cli:ali\nce", "cli:ali\x1bce", "cli:ali\x00ce", "cli:" + "a" * 65, "cli:alice:" + "n" * 65,
    "alice", "CLI:alice", "cli:", "cli:alice:node:extra", "cli:ali ce", "",
])
def test_actor_rejects_newline_control_and_oversize(value):
    with pytest.raises(actors.ActorError):
        actors.parse(value)


def test_actor_parse_and_format_round_trip():
    actor = actors.parse("overlay:bob@example.com:node-1.a")
    assert (actor.source, actor.login, actor.node_id) == ("overlay", "bob@example.com", "node-1.a")
    assert actors.format(actor) == "overlay:bob@example.com:node-1.a"
    assert actors.format(actors.parse("cli:alice")) == "cli:alice"


def test_cli_rejects_an_invalid_actor_before_any_effect(demo, capsys):
    assert cli.main(["dial", "demo", "save", "--actor", "bad actor"]) == exits.USAGE
    assert "actor" in capsys.readouterr().out
    assert _commands(demo.store) == []


ENGINE_ENV = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "builder", "SAKURA_FLOW_PHASE": "build"}


def _inbox_payloads(demo):
    target = demo.store.inbox_dir / "00000000000000000001-x.json"
    return [
        {"session_id": "s", "tool_name": "Write", "tool_input": {"file_path": str(target), "content": "{}"}},
        {"session_id": "s", "tool_name": "Bash", "tool_input": {"command": f"echo '{{}}' > {target}"}},
    ]


@pytest.mark.parametrize("role", ["builder", "evaluator", "planner"])
def test_brake_denies_inbox_write_from_engine_session(demo, repo, role):
    for payload in _inbox_payloads(demo):
        result = brakes.decision(repo, "PreToolUse", payload, {**ENGINE_ENV, "SAKURA_FLOW_ROLE": role})
        assert result is not None and result[0] == "deny" and "inbox" in result[1]
