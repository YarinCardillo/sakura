"""Fault injection through the cockpit, detached launcher and locked driver."""

import contextlib
import subprocess
import sys
from dataclasses import replace
from types import SimpleNamespace

import pytest

from sakura.cockpit import actions, api, items, proofs
from sakura.cockpit.controller import Controller
from sakura.flow import ask_words, cli, engine, exits, owner_marks, phases, ports, process, runner, store, surface, waits
from sakura.flow.tmux import TmuxError
from flow_support import MonitorLauncher, make_context, task_workspace
from test_cockpit_controller import SECRET, form
from test_cockpit_actions import OWNER
from test_cockpit_snapshot import create_task, make_snapshot


@pytest.fixture
def stopped(repo, monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_remember_repository", lambda args: None)
    task = create_task(repo, "t1", status="stopped")
    task.stop_file.write_text("operator stop")
    return task


def test_cockpit_binds_resume_to_the_stopped_revision(stopped):
    item = items.flow_item(stopped, lambda key: None)
    invocation = actions.ACTIONS["R"].build(item, {})
    assert any(word.startswith("--stopped-revision=") for word in invocation.argv)


@pytest.mark.parametrize(("path", "note"), [
    pytest.param("cli", "Prove that --format preserves empty rows.", id="cli"),
    pytest.param("detached", "Prove that --format preserves empty rows.", id="detached"),
    pytest.param("cockpit", "Prove that --format preserves empty rows.", id="cockpit"),
    pytest.param("cockpit", "x" * 280, id="cockpit-280-characters"),
    pytest.param("cockpit", "Check empty rows.\nCompare\tthe output.", id="cockpit-two-lines"),
    pytest.param("cli", "", id="no-note"),
])
def test_resume_passes_optional_note_to_the_next_planner_prompt(repo, monkeypatch, path, note):
    monkeypatch.setattr(ports, "is_free", lambda port: True)
    monkeypatch.setattr(process, "hold_awake", lambda pid: contextlib.nullcontext())
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable}"}},
                       state={"phase": "contract", "status": "blocked", "reason": ask_words.NOTHING_NEW_WORDS})
    workspace = task_workspace(ctx)
    prompts = []

    def monitor(argv, *, stdin_text, **kwargs):
        prompts.append(stdin_text)
        return process.ProcessOutcome(0, False, False, False, 0)

    def advance(current):
        agent = runner.AgentRunner(current.store, current.config, launcher=MonitorLauncher(monitor))
        agent.run(runner.RunRequest("planner", "contract", "Draft the contract.", {}, workspace))
        return exits.OK

    monkeypatch.chdir(repo)
    monkeypatch.setattr(engine, "open_context", lambda repo, task: replace(ctx, store=task))
    monkeypatch.setattr(engine, "advance", advance)
    monkeypatch.setattr(cli.tasks, "check_codex_trust", lambda *args: None)
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: SimpleNamespace(start_engine=launch))

    def launch(task, argv, env):
        assert cli.main(argv[2:]) == exits.OK
        return "session"

    if path == "cockpit":
        lane = items.flow_item(ctx.store, lambda key: None)
        (wait,) = waits.collect([repo])
        snapshot = make_snapshot(items=[lane], waits=[wait])
        invocation = actions.plan("R", snapshot.wait_target(wait), {"note": note}, OWNER)
        assert api._wait(wait, snapshot)["wants_note"] is True
        argv = invocation.argv[2:]
    else:
        options = [f"--note={note}"] if note else []
        argv = ["resume", *options, *(["--detach"] if path == "detached" else []), "demo"]
    assert cli.main(argv) == exits.OK
    steer = f"\n\nOperator steer:\n{note}\n" if note else ""
    assert prompts == [f"Draft the contract.{steer}"]
    resume_lines = [line for line in ctx.store.ledger.read_text().splitlines() if "resume requested" in line]
    assert len(resume_lines) == 1
    escaped = note.replace("\n", r"\n").replace("\t", r"\t")
    assert (f"note={escaped}" in resume_lines[0]) == bool(note)


@pytest.mark.parametrize("path", ["cockpit", "cli"])
@pytest.mark.parametrize(("note", "reason"), [
    pytest.param("x" * (owner_marks.MAX_NOTE + 1), str(owner_marks.MAX_NOTE), id="over-limit"),
    pytest.param("Check this.\r", "control character", id="carriage-return"),
    pytest.param("Check\x1b this.", "control character", id="escape"),
])
def test_resume_refuses_invalid_notes_before_launch(stopped, monkeypatch, capsys, path, note, reason):
    item = items.flow_item(stopped, lambda key: None)
    if path == "cockpit":
        with pytest.raises(actions.ActionRefused, match=f"invalid note: .*{reason}"):
            actions.plan("R", item, {"note": note}, OWNER)
    else:
        monkeypatch.setattr(cli, "_detach_stopped", lambda *args: pytest.fail("invalid note reached launch"))
        argv = ["resume", "--detach", f"--stopped-revision={item.resume_revision}", f"--note={note}", "t1"]
        assert cli.main(argv) == exits.USAGE
        assert "invalid note:" in (output := capsys.readouterr().out)
        assert reason in output


@pytest.mark.parametrize("fresh_after", [False, True])
@pytest.mark.parametrize("after", ["failed", "awaiting_approval", "blocked", "report_ready", "cleaned", "stopped"])
def test_two_independent_confirmations_admit_only_one_driver(stopped, monkeypatch, tmp_path, after, fresh_after):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child: the only process that may carry a viewer's identity
    item = items.flow_item(stopped, lambda key: None)
    snapshot = make_snapshot(items=[item])
    driven = []
    if after != "failed":
        monkeypatch.setattr(engine, "open_context", lambda repo, task: SimpleNamespace(config=None, store=task))
    monkeypatch.setattr(cli.tasks, "check_codex_trust", lambda *args: None)

    def advance(ctx):
        driven.append(ctx)
        ctx.store.update_state(lambda state: state.update(status=after))
        return exits.FAILED

    if after == "failed":
        def fail_phase(ctx):
            driven.append(ctx)
            return engine.PhaseOutcome("failed", "injected phase failure")
        monkeypatch.setitem(engine._HANDLERS, "build", fail_phase)
    else:
        monkeypatch.setattr(engine, "advance", advance)
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: SimpleNamespace(start_engine=launch))
    monkeypatch.setattr(cli, "_task_config", lambda *args: None)

    def launch(task, argv, env):
        cli.main(argv[2:])
        return "session"

    def run(argv, **kwargs):
        return subprocess.CompletedProcess(argv, cli.main(argv[2:]), "", "")

    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=run,
                            log_path=tmp_path / "actions.jsonl", store=proofs.ProofStore(tmp_path / "proofs"))
    fields = form(item, "R")
    first = controller.perform(fields, OWNER, proof_field="proof", proof_source=proofs.OVERLAY_SOURCE)
    second = controller.perform(fields, OWNER, proof_field="proof", proof_source=proofs.OVERLAY_SOURCE)
    controller.perform({**fields, "proof": first.token}, OWNER, proof_field="proof", proof_source=proofs.OVERLAY_SOURCE)
    if fresh_after:
        snapshot.items[:] = [items.flow_item(store.TaskStore(stopped.repo, stopped.task_id), lambda key: None)]
    controller.perform({**fields, "proof": second.token}, OWNER, proof_field="proof", proof_source=proofs.OVERLAY_SOURCE)
    assert len(driven) == 1


@pytest.mark.parametrize("fault", ["lock", "context"])
def test_child_startup_failure_is_proven_refusal_and_keeps_stop(stopped, monkeypatch, fault):
    item = items.flow_item(stopped, lambda key: None)
    monkeypatch.setattr(cli, "_task_config", lambda *args: None)

    def reject(*args):
        raise store.StateError("cannot load context")

    monkeypatch.setattr(engine, "open_context", reject)

    def launch(task, argv, env):
        if fault == "lock":
            with task.lock():
                cli.main(argv[2:])
        else:
            cli.main(argv[2:])
        return "session"

    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: SimpleNamespace(start_engine=launch))
    invocation = actions.ACTIONS["R"].build(item, {})
    assert cli.main(invocation.argv[2:]) == exits.USAGE
    assert stopped.stop_file.exists()
    assert stopped.read_state()["status"] == "stopped"


def test_failure_after_new_window_is_uncertain_and_unmarked_driver_is_not_relaunched(stopped, monkeypatch):
    launched = []
    client = SimpleNamespace(new_window=lambda *args: launched.append(True) or "@1",
                             set_option=lambda *args: (_ for _ in ()).throw(TmuxError("lost connection")))
    shown = surface.TmuxSurface(client, "test-")
    monkeypatch.setattr(shown, "_ensure_session", lambda task: "session")
    monkeypatch.setattr(shown, "_retire_ended_engine", lambda session: None)
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: shown)
    monkeypatch.setattr(cli, "_task_config", lambda *args: None)
    invocation = actions.ACTIONS["R"].build(items.flow_item(stopped, lambda key: None), {})
    code = cli.main(invocation.argv[2:])
    assert launched == [True]
    assert code != 0
    assert actions.refusal("R", actions.ActionResult(invocation, code, "lost connection")) is None


def test_timeout_after_launch_is_uncertain(stopped, tmp_path):
    item = items.flow_item(stopped, lambda key: None)
    launched = []

    def timeout(argv, **kwargs):
        launched.append(True)
        raise subprocess.TimeoutExpired(argv, 120)

    result = actions.run("R", item, {}, actions.actors.Actor("cli", "owner"), run=timeout,
                         log_path=tmp_path / "actions.jsonl")
    assert launched == [True]
    assert result.exit_code != 0
    assert actions.refusal("R", result) is None


def test_unmarked_engine_window_still_prevents_a_duplicate_launch():
    window = SimpleNamespace(name=surface.ENGINE_WINDOW, options={}, dead=False)
    shown = surface.TmuxSurface(SimpleNamespace(windows=lambda session: [window]), "test-")
    with pytest.raises(surface.DriverRunning):
        shown._retire_ended_engine("session")


def test_resume_note_is_queued_once_after_an_interrupted_admission(stopped, monkeypatch):
    from sakura.flow import inbox

    revision = items.flow_item(stopped, lambda key: None).resume_revision
    monkeypatch.setattr(engine, "open_context", lambda repo, task: SimpleNamespace(config=None))
    monkeypatch.setattr(cli.tasks, "check_codex_trust", lambda *args: None)
    monkeypatch.setattr(engine, "advance", lambda ctx: exits.OK)
    monkeypatch.setattr(process, "hold_awake", lambda pid: contextlib.nullcontext())
    argv = ["resume", f"--stopped-revision={revision}", "--note=Check empty rows.", "t1"]

    def interrupt(*args):
        raise OSError("interrupted before persisting admission")

    with monkeypatch.context() as patch:
        patch.setattr(store.TaskStore, "update_state", interrupt)
        with pytest.raises(OSError, match="interrupted"):
            cli.main(argv)
    assert cli.main(argv) == exits.OK
    current = store.TaskStore(stopped.repo, stopped.task_id)
    notes = [entry["text"] for entry in current.read_state().get("steer_pending", [])]
    notes.extend(command["value"] for _, command in inbox.pending(stopped))
    assert notes == ["Check empty rows."]


def test_cockpit_parent_does_not_record_its_own_resume_decision(stopped, tmp_path):
    from datetime import UTC, datetime

    from sakura.cockpit import decided

    stopped.update_state(lambda state: state.update(status="blocked", reason="interrupted"))
    item = items.flow_item(stopped, lambda key: None)
    log = tmp_path / "parent" / "actions.jsonl"
    result = actions.run("R", item, {}, OWNER,
                         run=lambda argv, **kwargs: subprocess.CompletedProcess(argv, 0, "", ""), log_path=log)
    assert result.exit_code == exits.OK
    assert decided.recent(log.parent, now=datetime.now(UTC)) == []
