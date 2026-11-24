import hashlib
import json
import os
import threading
import time

import pytest

from sakura.flow import config, contract, engine, exits, process, roles
from sakura.flow.phases import evaluate, integrate
from conftest import git
from flow_support import make_context


@pytest.fixture(autouse=True)
def isolated_registry(monkeypatch):
    monkeypatch.setattr(engine, "_HANDLERS", {})


def _history(ctx):
    return [(entry["phase"], entry["status"]) for entry in ctx.store.read_state()["history"]]


def test_phase_order_is_fixed():
    assert engine.PHASE_ORDER == ["intake", "contract", "plan", "build", "evaluate", "pr", "report", "cleanup"]


def test_advance_records_history_and_stops_at_await(repo):
    ctx = make_context(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("next"))
    engine.register("contract", lambda c: engine.PhaseOutcome("await", gate="contract"))
    assert engine.advance(ctx) == exits.OK
    state = ctx.store.read_state()
    assert state["phase"] == "contract"
    assert state["status"] == "awaiting_approval"
    assert "sakura flow approve demo contract" in state["reason"]
    assert _history(ctx) == [("intake", "done"), ("contract", "awaiting_approval")]
    assert "notify awaiting_approval" in ctx.store.ledger.read_text()


def test_advance_skip_moves_on_and_records_skipped(repo):
    ctx = make_context(repo, state={"phase": "plan"})
    engine.register("plan", lambda c: engine.PhaseOutcome("skip", reason="vanilla"))
    engine.register("build", lambda c: engine.PhaseOutcome("failed", reason="boom"))
    assert engine.advance(ctx) == exits.FAILED
    assert _history(ctx) == [("plan", "skipped"), ("build", "failed")]
    state = ctx.store.read_state()
    assert state["status"] == "failed" and state["reason"] == "boom" and state["phase"] == "build"


def test_unregistered_phase_blocks(repo):
    ctx = make_context(repo)
    assert engine.advance(ctx) == exits.BLOCKED
    state = ctx.store.read_state()
    assert state["status"] == "blocked" and state["reason"] == "phase intake not available"


def test_blocked_and_stopped_outcomes_map_to_exit_codes(repo):
    ctx = make_context(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", reason="why"))
    assert engine.advance(ctx) == exits.BLOCKED
    engine.register("intake", lambda c: engine.PhaseOutcome("stopped", reason="asked"))
    assert engine.advance(ctx) == exits.STOPPED
    assert ctx.store.read_state()["status"] == "stopped"


def test_refused_role_blocks_with_reason(repo):
    ctx = make_context(repo)

    def refuse(_):
        raise roles.RoleRefused("Codex builder needs recorded hook trust: run sakura trust-codex --record")

    engine.register("intake", refuse)
    assert engine.advance(ctx) == exits.BLOCKED
    assert "trust-codex --record" in ctx.store.read_state()["reason"]


def test_last_phase_completes_task(repo):
    ctx = make_context(repo, state={"phase": "cleanup"})
    engine.register("cleanup", lambda c: engine.PhaseOutcome("next"))
    assert engine.advance(ctx) == exits.OK
    assert ctx.store.read_state()["status"] == "cleaned"
    assert engine.advance(ctx) == exits.OK


def test_resume_kills_external_child_from_separate_registry(repo):
    ctx = make_context(repo)
    result = []
    worker = threading.Thread(target=lambda: result.append(process.external_tracked_run(
        ctx.store, ["sh", "-c", "sleep 30"], cwd=repo, env=dict(os.environ),
        log_path=ctx.store.operations_dir / "external-stale.log", budget_seconds=60,
        idle_seconds=60, stop_file=ctx.store.stop_file)))
    worker.start()
    deadline = time.monotonic() + 5
    runs = []
    while time.monotonic() < deadline:
        runs = ctx.store.external_runs()
        if runs:
            break
        time.sleep(0.05)

    assert len(runs) == 1
    pid = runs[0]["pid"]
    engine.register("intake", lambda _ctx: engine.PhaseOutcome("await", gate="spec"))
    assert engine.advance(ctx) == exits.OK
    worker.join(timeout=5)

    assert not worker.is_alive()
    assert len(result) == 1
    assert not process.is_alive(pid)
    assert ctx.store.external_runs() == []
    assert ctx.store.read_state().get("active_runs", []) == []


def test_resume_retains_unhandled_external_child_identity(repo, monkeypatch):
    ctx = make_context(repo)
    entry = {"pid": 24680, "command": "expected command"}
    ctx.store.register_external_run(entry["pid"], entry["command"])
    monkeypatch.setattr(engine.process, "kill_tree", lambda _pid, _command: False)
    monkeypatch.setattr(engine.process, "is_alive", lambda _pid: True)

    engine.kill_stale_run(ctx)

    assert ctx.store.external_runs() == [entry]


def test_resume_clears_blocked_status_before_running(repo):
    ctx = make_context(repo, state={"status": "blocked", "reason": "old"})
    seen = []
    engine.register("intake", lambda c: seen.append(c.store.read_state()["status"]) or engine.PhaseOutcome("await", gate="spec"))
    engine.advance(ctx)
    assert seen == ["running"]


def test_tampered_state_blocks_and_discards_forgery(repo):
    ctx = make_context(repo)

    def forge(c):
        state = c.store.read_state()
        state["approvals"]["spec"] = {"sha256": "forged"}
        c.store.state_file.write_text(json.dumps(state))
        return engine.PhaseOutcome("next")

    engine.register("intake", forge)
    assert engine.advance(ctx) == exits.BLOCKED
    state = ctx.store.read_state()
    assert state["status"] == "blocked" and state["reason"] == "state.json changed outside the engine"
    assert state["approvals"] == {}


def test_completed_phase_is_skipped_until_inputs_change(repo):
    ctx = make_context(repo)
    calls, inputs = [], ["a"]
    engine.register("intake", lambda c: calls.append(1) or engine.PhaseOutcome("next"), lambda c: inputs[0])
    engine.register("contract", lambda c: engine.PhaseOutcome("await", gate="contract"))
    engine.advance(ctx)
    assert ctx.store.read_state()["completed"] == {"intake": "a"}
    ctx.store.update_state(lambda s: s.update(phase="intake"))
    engine.advance(ctx)
    assert calls == [1]
    ctx.store.update_state(lambda s: s.update(phase="intake"))
    inputs[0] = "b"
    engine.advance(ctx)
    assert calls == [1, 1] and ctx.store.read_state()["completed"]["intake"] == "b"


def test_spec_edit_after_approval_returns_to_gate(repo):
    ctx = make_context(repo, state={"phase": "contract"})
    ctx.store.spec.write_text("## Journeys\n1. a\n")
    digest = hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    ctx.store.update_state(lambda s: s["approvals"].update(spec={"at": "t", "sha256": digest}))
    engine.register("contract", lambda c: engine.PhaseOutcome("blocked", reason="reached"))
    assert engine.advance(ctx) == exits.BLOCKED
    ctx.store.spec.write_text("## Journeys\n1. a\n2. b\n")
    assert engine.advance(ctx) == exits.OK
    state = ctx.store.read_state()
    assert state["phase"] == "intake" and state["status"] == "awaiting_approval"
    assert "approve demo spec" in state["reason"] and "spec" not in state["approvals"]
    assert "spec changed after approval" in ctx.store.ledger.read_text()


def test_repeated_gate_does_not_duplicate_history_or_ledger(repo):
    ctx = make_context(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("await", gate="spec"))
    engine.advance(ctx)
    engine.advance(ctx)
    assert len(ctx.store.read_state()["history"]) == 1
    assert ctx.store.ledger.read_text().count("notify awaiting_approval") == 1


def test_blocked_and_failed_repeats_are_always_recorded_and_notified(repo):
    ctx = make_context(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", reason="same"))
    engine.advance(ctx)
    engine.advance(ctx)
    engine.register("intake", lambda c: engine.PhaseOutcome("failed", reason="same"))
    engine.advance(ctx)
    engine.advance(ctx)
    state = ctx.store.read_state()
    assert [entry["status"] for entry in state["history"]] == ["blocked", "blocked", "failed", "failed"]
    ledger = ctx.store.ledger.read_text()
    assert ledger.count("notify escalation") == 2 and ledger.count("notify failed") == 2


class _Recorder:
    def __init__(self):
        self.events = []

    def notify(self, event, task, message):
        self.events.append((event, message))


def _recording(repo):
    ctx = make_context(repo)
    recorder = _Recorder()
    return engine.FlowContext(ctx.repo, ctx.store, ctx.config, ctx.runner, recorder, ctx.steps), recorder


def test_blocked_outcome_notifies_escalation(repo):
    ctx, recorder = _recording(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", reason="agent exited 7"))
    engine.advance(ctx)
    assert recorder.events == [("escalation", "agent exited 7")]


def test_explicit_event_wins_over_default_escalation(repo):
    ctx, recorder = _recording(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", reason="x", event="gate_red"))
    engine.advance(ctx)
    assert recorder.events[0][0] == "gate_red"


def test_unavailable_phase_notifies_escalation(repo):
    ctx, recorder = _recording(repo)
    ctx.store.update_state(lambda state: state.update(phase="evaluate"))
    engine.advance(ctx)
    assert recorder.events == [("escalation", "phase evaluate not available")]


def test_tamper_block_notifies_escalation(repo):
    ctx, recorder = _recording(repo)

    def forge(c):
        state = c.store.read_state()
        state["approvals"]["spec"] = {"sha256": "forged"}
        c.store.state_file.write_text(json.dumps(state))
        return engine.PhaseOutcome("next")

    engine.register("intake", forge)
    assert engine.advance(ctx) == exits.BLOCKED
    assert recorder.events == [("escalation", engine.TAMPERED_REASON)]


def test_plan_review_escalation_is_posted_once(repo):
    from sakura.flow.phases import plan as plan_phase
    from test_flow_plan import REVISE, _ctx

    plan_ctx, _ = _ctx(repo, [REVISE] * 5)
    recorder = _Recorder()
    ctx = engine.FlowContext(plan_ctx.repo, plan_ctx.store, plan_ctx.config, plan_ctx.runner, recorder, plan_ctx.steps)
    ctx.store.update_state(lambda state: state.update(phase="plan"))
    engine.register("plan", plan_phase.run)
    engine.advance(ctx)
    assert [event for event, _ in recorder.events] == ["escalation"]


def test_resume_from_pr_reopens_evaluation_when_inputs_changed(repo):
    ctx = make_context(repo, state={"phase": "pr", "workspaces": {"0": {"path": str(repo)}},
                                    "completed": {"evaluate": "old-inputs"}})
    digest = contract.save(ctx.store, [contract.Row("R1", "j", "s", "api", None, "true", False, True)])
    ctx.store.update_state(lambda state: state.update(contract_sha256=digest))
    object.__setattr__(ctx, "steps", config.PresetSteps(True, True, True))
    calls = []
    engine.register("evaluate", lambda _ctx: calls.append("evaluate") or engine.PhaseOutcome("next"),
                    evaluate.inputs_hash)
    engine.register("pr", lambda _ctx: calls.append("pr") or engine.PhaseOutcome("await", gate="spec"))

    engine.advance(ctx)

    assert calls == ["evaluate", "pr"]


def test_resume_from_pr_reopens_full_tests_when_configuration_changes(repo):
    head = git(repo, "rev-parse", "HEAD").strip()
    ctx = make_context(repo, state={"phase": "pr", "full_green_head": head})
    initial_hash = integrate.full_tests_inputs_hash(ctx)
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256=initial_hash))
    ctx.config.values["tests"]["full"] = ["true"]
    calls = []
    engine.register("build", lambda _ctx: calls.append("build") or engine.PhaseOutcome("next"))
    engine.register("evaluate", lambda _ctx: calls.append("evaluate") or engine.PhaseOutcome("skip"))
    engine.register("pr", lambda _ctx: calls.append("pr") or engine.PhaseOutcome("await", gate="spec"))

    engine.advance(ctx)

    assert calls == ["build", "evaluate", "pr"]


def test_resume_from_pr_reopens_full_tests_when_workspace_head_changes(repo):
    old_head = git(repo, "rev-parse", "HEAD").strip()
    ctx = make_context(repo, state={"phase": "pr", "full_green_head": old_head,
                                    "workspaces": {"0": {"path": str(repo)}}})
    object.__setattr__(ctx, "steps", config.PresetSteps(True, True, False))
    input_hash = integrate.full_tests_inputs_hash(ctx)
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256=input_hash))
    (repo / "after-tests.txt").write_text("new code\n")
    git(repo, "add", "after-tests.txt")
    git(repo, "commit", "-qm", "change code after full tests")
    calls = []
    engine.register("build", lambda _ctx: calls.append("build") or engine.PhaseOutcome("next"))
    engine.register("evaluate", lambda _ctx: calls.append("evaluate") or engine.PhaseOutcome("skip"))
    engine.register("pr", lambda _ctx: calls.append("pr") or engine.PhaseOutcome("await", gate="spec"))

    engine.advance(ctx)

    assert calls == ["build", "evaluate", "pr"]


def test_stale_run_is_cleared_before_the_spec_changed_gate_returns(repo, monkeypatch):
    ctx = make_context(repo, state={"phase": "contract"})
    ctx.store.spec.write_text("## Journeys\n1. a\n")
    digest = hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    ctx.store.update_state(lambda s: s["approvals"].update(spec={"at": "t", "sha256": digest}))
    ctx.store.spec.write_text("## Journeys\n1. a\n2. b\n")
    ctx.store.register_external_run(24680, "expected command")
    killed = []
    monkeypatch.setattr(engine.process, "kill_tree", lambda pid, command: killed.append(pid) or True)
    monkeypatch.setattr(engine.process, "is_alive", lambda pid: False)

    assert engine.advance(ctx) == exits.OK

    assert "spec changed after approval" in ctx.store.ledger.read_text()
    assert killed == [24680] and ctx.store.external_runs() == []
