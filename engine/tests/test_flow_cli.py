import json
import os
import sys

import pytest

from sakura import codex_trust, engine_home, install
from sakura.flow import cli, contract, exits, flags, process, run_records, store
from conftest import git

AGENT = """import os, pathlib
pathlib.Path(os.environ["SAKURA_FLOW_EXCHANGE"], "spec.md").write_text("# S\\n\\n## Journeys\\n1. go\\n")
"""


@pytest.fixture
def project(repo, tmp_path, monkeypatch):
    (repo / "specs").mkdir()
    (repo / "specs/add-greeting.md").write_text("add a greeting\n")
    agent = tmp_path / "agent.py"
    agent.write_text(AGENT)
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    return repo, f"exec:{sys.executable} {agent}"


def _start(project, *extra):
    repo, command = project
    roles = ["--role", f"planner={command}", "--role", "builder=exec:true"]
    return cli.main(["start", "specs/add-greeting.md", *roles, *extra])


def _awaiting(repo, gate, task="add-greeting"):
    """The task waits at `gate` for the person's approval, as the engine records it (an approval is accepted only then)."""
    store.TaskStore(repo, task).update_state(lambda state: state.update(
        status="awaiting_approval", reason=f"awaiting approval: sakura flow approve {task} {gate}"))


def _state(repo, task="add-greeting"):
    return store.TaskStore(repo, task).read_state()


def test_build_overrides_maps_every_flag():
    overrides = flags.build_overrides("standard", "lean", None, ["code_reviewer=codex:x"],
                                      ["timeouts.build=60", "spec.source=notion", "app.ready_seconds=3"], ["preset", "preset2"])
    assert overrides == {"preset": "standard", "dial": "lean", "roles": {"code_reviewer": "codex:x"},
                         "timeouts": {"build": 60}, "spec": {"source": "notion"}, "app": {"ready_seconds": 3},
                         "relax": ["preset", "preset2"]}
    assert flags.build_overrides(None, None, None, [], ["approve_spec=false"], []) == {"approve_spec": False}
    with pytest.raises(flags.FlagError):
        flags.build_overrides(None, None, None, ["nonsense"], [], [])


def test_start_pins_base_and_stores_overrides(project, capsys):
    repo, _ = project
    assert _start(project, "--dial", "lean") == exits.OK
    assert "sakura flow approve add-greeting spec" in capsys.readouterr().out
    state = _state(repo)
    assert state["base_branch"] == "main" and state["base_commit"] == git(repo, "rev-parse", "main")
    assert state["branch"] == "feat/flow-add-greeting" and state["dial"] == "lean"
    assert state["overrides"]["dial"] == "lean" and state["status"] == "awaiting_approval"
    assert (store.TaskStore(repo, "add-greeting").spec).exists()


def test_approve_records_hash_and_resume_reaches_contract_phase(project, capsys):
    repo, _ = project
    _start(project)
    assert cli.main(["approve", "add-greeting", "spec"]) == exits.OK
    assert "sakura flow resume add-greeting" in capsys.readouterr().out
    assert len(_state(repo)["approvals"]["spec"]["sha256"]) == 64
    assert cli.main(["resume", "add-greeting"]) == exits.BLOCKED
    assert "The planner ended without writing the contract" in capsys.readouterr().out  # plain words (round 6, N13)
    assert _state(repo)["phase"] == "contract"


def test_resume_uses_stored_overrides(project):
    repo, _ = project
    _start(project, "--dial", "power")
    cli.main(["approve", "add-greeting", "spec"])
    cli.main(["resume", "add-greeting"])
    assert _state(repo)["dial"] == "power"


def test_resume_refused_while_engine_holds_lock(project, capsys):
    repo, _ = project
    _start(project)
    with store.TaskStore(repo, "add-greeting").lock():
        assert cli.main(["resume", "add-greeting"]) == exits.BUSY
    assert "engine already running (pid " in capsys.readouterr().out


def _stopped_task_with_busy_engine(project):
    """A started task with an operator STOP file; returns its store and the ledger text before the command."""
    repo, _ = project
    _start(project)
    task_store = store.TaskStore(repo, "add-greeting")
    task_store.stop_file.write_text("now operator\n")
    return task_store, task_store.ledger.read_text()


def test_busy_resume_leaves_stop_in_place_and_ledgers_nothing(project, capsys):
    task_store, ledger_before = _stopped_task_with_busy_engine(project)
    with task_store.lock():
        assert cli.main(["resume", "add-greeting"]) == exits.BUSY
    assert task_store.stop_file.exists()
    assert task_store.ledger.read_text() == ledger_before


def test_busy_start_of_an_existing_task_leaves_stop_in_place_and_ledgers_nothing(project):
    task_store, ledger_before = _stopped_task_with_busy_engine(project)
    with task_store.lock():
        assert _start(project) == exits.BUSY
    assert task_store.stop_file.exists()
    assert task_store.ledger.read_text() == ledger_before


def test_busy_raced_start_leaves_stop_in_place_and_ledgers_nothing(project, monkeypatch):
    task_store, ledger_before = _stopped_task_with_busy_engine(project)
    monkeypatch.setattr(cli.tasks, "known_tasks", lambda repo: {})
    with task_store.lock():
        assert _start(project) == exits.BUSY
    assert task_store.stop_file.exists()
    assert task_store.ledger.read_text() == ledger_before


def test_busy_cleanup_leaves_stop_in_place_and_ledgers_nothing(project):
    task_store, _ = _stopped_task_with_busy_engine(project)
    task_store.update_state(lambda state: state.update(phase="cleanup"))
    ledger_before = task_store.ledger.read_text()
    with task_store.lock():
        assert cli.main(["cleanup", "add-greeting"]) == exits.BUSY
    assert task_store.stop_file.exists()
    assert task_store.ledger.read_text() == ledger_before


def test_resume_clears_stop_and_ledgers_it_once_the_lock_is_held(project):
    task_store, _ = _stopped_task_with_busy_engine(project)
    cli.main(["resume", "add-greeting"])
    assert not task_store.stop_file.exists()
    assert "resumed by operator" in task_store.ledger.read_text()


def test_raced_start_with_a_different_ref_is_refused_not_driven(project, monkeypatch, capsys):
    repo, _ = project
    (repo / "other").mkdir()
    (repo / "other/add-greeting.md").write_text("another spec\n")
    _start(project)
    capsys.readouterr()
    monkeypatch.setattr(cli.tasks, "known_tasks", lambda repo: {})
    driven = []
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: driven.append(args) or exits.OK)
    code = cli.main(["start", "other/add-greeting.md", "--role", "builder=exec:true"])
    assert code == exits.USAGE and driven == []
    output = capsys.readouterr().out
    assert "specs/add-greeting.md" in output and "other/add-greeting.md" in output


def test_raced_start_with_the_same_ref_resumes_the_winner(project, monkeypatch):
    _start(project)
    monkeypatch.setattr(cli.tasks, "known_tasks", lambda repo: {})
    driven = []
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: driven.append(args) or exits.OK)
    assert _start(project) == exits.OK and len(driven) == 1


def test_after_requires_an_existing_task(project, capsys):
    assert _start(project, "--after", "other") == exits.USAGE
    assert "unknown task other" in capsys.readouterr().out


def test_unknown_task_is_usage(project, capsys):
    assert cli.main(["resume", "nope"]) == exits.USAGE
    assert cli.main(["approve", "nope", "spec"]) == exits.USAGE
    assert "unknown task nope" in capsys.readouterr().out


def test_approve_without_spec_is_usage(project, capsys):
    repo, _ = project
    store.TaskStore(repo, "x").create({"task": "x", "approvals": {}, "status": "awaiting_approval",
                                       "reason": "awaiting approval: sakura flow approve x spec"})
    assert cli.main(["approve", "x", "spec"]) == exits.USAGE
    assert "no spec.md" in capsys.readouterr().out


def test_codex_reviewer_without_trust_is_usage_before_any_run(project, capsys):
    repo, _ = project
    assert _start(project, "--role", "code_reviewer=codex") == exits.USAGE
    assert "Codex code_reviewer needs recorded hook trust: run sakura trust-codex --record" in capsys.readouterr().out
    assert not store.flow_root(repo).exists()


def test_codex_reviewer_with_trust_starts(project):
    repo, _ = project
    install.install(repo)
    codex_trust.record(repo)
    assert _start(project, "--role", "code_reviewer=codex") == exits.OK


def test_status_for_task_text_and_json(project, capsys):
    _start(project)
    capsys.readouterr()
    assert cli.main(["status", "add-greeting", "--json"]) == exits.OK
    payload = json.loads(capsys.readouterr().out)
    assert payload["task"] == "add-greeting" and payload["phase"] == "intake"
    assert payload["status"] == "awaiting_approval" and payload["contract"] == {"passed": 0, "total": 0}
    assert payload["approvals"] == []
    assert payload["roles"]["planner"].startswith("exec:") and payload["relaxations"] == []
    assert cli.main(["status", "add-greeting"]) == exits.OK
    text = capsys.readouterr().out
    assert "status: awaiting_approval" in text and "contract: 0/0" in text


def test_status_reports_stop_request_without_rewriting_saved_phase(project, capsys):
    repo, _ = project
    _start(project)
    capsys.readouterr()
    saved_phase = _state(repo)["phase"]

    assert cli.main(["stop", "add-greeting"]) == exits.OK
    capsys.readouterr()
    assert cli.main(["status", "add-greeting", "--json"]) == exits.OK
    payload = json.loads(capsys.readouterr().out)

    # A lane no engine drives is listed as stopped at once (proof defect D7); its saved phase is not rewritten.
    assert payload["status"] == "stopped" and payload["phase"] == saved_phase
    assert payload["stop_requested"] is True
    assert store.TaskStore(repo, "add-greeting").stop_file.exists()


def test_status_text_shows_stop_request_and_approved_gates(project, capsys):
    _start(project)
    assert cli.main(["approve", "add-greeting", "spec"]) == exits.OK
    assert cli.main(["stop", "add-greeting"]) == exits.OK
    capsys.readouterr()

    assert cli.main(["status", "add-greeting"]) == exits.OK
    text = capsys.readouterr().out
    assert "stop requested: yes" in text and "approved: spec" in text


def test_status_counts_contract_rows(project, capsys):
    repo, _ = project
    _start(project)
    task_store = store.TaskStore(repo, "add-greeting")
    task_store.contract.write_text(json.dumps({"rows": [{"passes": True}, {"passes": False}, {"passes": True}]}))
    capsys.readouterr()
    cli.main(["status", "add-greeting", "--json"])
    assert json.loads(capsys.readouterr().out)["contract"] == {"passed": 2, "total": 3}


def test_start_losing_a_race_resumes_instead_of_failing(project, capsys, monkeypatch):
    repo, _ = project
    assert _start(project) == exits.OK
    capsys.readouterr()
    monkeypatch.setattr(cli.tasks, "known_tasks", lambda _: {})
    assert _start(project) == exits.OK
    out = capsys.readouterr().out
    assert "resuming add-greeting" in out and "sakura flow approve add-greeting spec" in out


def test_resume_after_spec_edit_reopens_gate(project, capsys):
    repo, _ = project
    _start(project)
    cli.main(["approve", "add-greeting", "spec"])
    spec = store.TaskStore(repo, "add-greeting").spec
    spec.write_text(spec.read_text() + "2. more\n")
    capsys.readouterr()
    assert cli.main(["resume", "add-greeting"]) == exits.OK
    assert "sakura flow approve add-greeting spec" in capsys.readouterr().out
    assert _state(repo)["phase"] == "intake"


def test_completed_intake_not_rerun_on_resume(project):
    repo, _ = project
    _start(project)
    cli.main(["approve", "add-greeting", "spec"])
    cli.main(["resume", "add-greeting"])
    assert _state(repo)["completed"]["intake"]
    runs = list(store.TaskStore(repo, "add-greeting").runs_dir.glob("intake-*.log"))
    cli.main(["resume", "add-greeting"])
    assert list(store.TaskStore(repo, "add-greeting").runs_dir.glob("intake-*.log")) == runs


def test_stop_writes_stop_file_for_task_and_for_all(project):
    repo, _ = project
    assert _start(project) == exits.OK
    assert cli.main(["stop", "add-greeting"]) == exits.OK
    stop_file = store.TaskStore(repo, "add-greeting").stop_file
    assert stop_file.read_text().strip().endswith("operator")
    stop_file.unlink()
    assert cli.main(["stop", "--all"]) == exits.OK and stop_file.exists()


def test_stop_without_target_is_usage_error(project):
    assert cli.main(["stop"]) == exits.USAGE


def test_steer_queues_commands_in_the_inbox(project):
    repo, _ = project
    assert _start(project) == exits.OK
    assert cli.main(["steer", "add-greeting", "first"]) == exits.OK
    assert cli.main(["steer", "add-greeting", "second"]) == exits.OK
    task_store = store.TaskStore(repo, "add-greeting")
    queued = [json.loads(path.read_text())["value"] for path in sorted(task_store.inbox_dir.glob("*.json"))]
    assert queued == ["first", "second"] and not task_store.steer_file.exists()


def test_resume_removes_stop_file_and_ledgers_it(project):
    repo, _ = project
    assert _start(project) == exits.OK
    cli.main(["stop", "add-greeting"])
    cli.main(["resume", "add-greeting"])
    task = store.TaskStore(repo, "add-greeting")
    assert not task.stop_file.exists() and "resumed by operator" in task.ledger.read_text()


def test_load_status_includes_queue_facts(repo, tmp_path, monkeypatch):
    from sakura import loadinfo

    class StatusStore:
        contract = tmp_path / "contract.json"
        stop_file = tmp_path / "STOP"
        inbox_dir = tmp_path / "inbox"

        def read_state(self):
            return {"task": "demo", "phase": "build", "status": "running", "reason": "", "overrides": None}

    monkeypatch.setattr(loadinfo, "load_snapshot", lambda _queue: {
        "slots_total": 2, "slots_in_use": 1,
        "queue": [{"label": "pytest", "command": "pytest", "waiting_seconds": 4, "critical": True}],
        "loadavg": [1.0, 2.0, 3.0]})
    payload = cli._task_status(repo, StatusStore())
    text = cli._status_lines(payload)
    assert payload["load"]["slots_in_use"] == 1
    assert payload["stop_requested"] is False
    assert "load slots: 1/2" in text and "waiting: pytest (4 s)" in text


def test_report_writes_repeatable_local_status_for_blocked_task(project, capsys):
    repo, _ = project
    _start(project)
    capsys.readouterr()
    task_store = store.TaskStore(repo, "add-greeting")
    task_store.update_state(lambda state: state.update(
        status="blocked", phase="plan", reason="review requires a correction"
    ))
    rows = [
        contract.Row("R1", "J1", "new route", "web", "1440x900", "true", False, False),
        contract.Row("R7", "J4", "existing route", "web", "1440x900", "true", True, False),
    ]
    contract.save(task_store, rows)
    evidence_dir = task_store.evidence_dir / "contract-base"
    evidence_dir.mkdir(parents=True)
    (evidence_dir / "R1-1440x900.log").write_text("exit 1\n")
    (evidence_dir / "R7-1440x900.log").write_text("exit 0\n")
    before = task_store.read_state()

    assert cli.main(["report", "add-greeting"]) == exits.OK
    first_output = capsys.readouterr().out
    first_report = task_store.local_report.read_text()
    assert "Local status report" in first_output
    assert "Status: blocked / plan" in first_report
    assert "review requires a correction" in first_report
    assert "not ready" in first_report.lower()
    assert "Pull request: not opened" in first_report
    assert "R1 (issue-specific, expected to fail on base): exit 1" in first_report
    assert "R7 (baseline, expected to pass on base): exit 0" in first_report
    assert "Final evaluator verdict: not recorded" in first_report
    assert task_store.read_state() == before

    assert cli.main(["report", "add-greeting"]) == exits.OK
    assert capsys.readouterr().out == first_output
    assert task_store.local_report.read_text() == first_report
    assert not task_store.report.exists()


def test_report_refuses_running_task_without_writing_local_artifact(project, capsys):
    repo, _ = project
    _start(project)
    task_store = store.TaskStore(repo, "add-greeting")
    task_store.update_state(lambda state: state.update(status="running", phase="build"))
    before = task_store.read_state()

    assert cli.main(["report", "add-greeting"]) == exits.USAGE
    assert "still running" in capsys.readouterr().out
    assert task_store.read_state() == before
    assert not task_store.local_report.exists()


def test_report_prefers_existing_final_report(project, capsys):
    repo, _ = project
    _start(project)
    capsys.readouterr()
    task_store = store.TaskStore(repo, "add-greeting")
    task_store.report.write_text("# Final report\n")

    assert cli.main(["report", "add-greeting"]) == exits.OK
    assert capsys.readouterr().out == "# Final report\n"
    assert not task_store.local_report.exists()


def _blocked_plan_with_contract(project):
    repo, _ = project
    _start(project)
    cli.main(["approve", "add-greeting", "spec"])
    task_store = store.TaskStore(repo, "add-greeting")
    rows = [
        contract.Row("R1", "J1", "new behavior", "cli", None, "true", False, False),
        contract.Row("R2", "J2", "baseline behavior", "cli", None, "true", True, False),
    ]
    contract.save(task_store, rows)
    task_store.update_state(lambda state: state.update(
        phase="plan", status="blocked", reason="review requires a new contract row",
        plan_sha256="old-plan", plan_review_findings=["missing new row"],
        rounds={"plan_review": 2}, completed={"intake": "intake-hash", "contract": "old-contract-hash"},
    ))
    evidence = task_store.evidence_dir / "contract-base" / "R1-1440x900.log"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text("previous base evidence\n")
    return repo, task_store


def test_recontract_archives_authenticated_contract_and_rewinds_to_contract(project, capsys):
    repo, task_store = _blocked_plan_with_contract(project)
    old_digest = _state(repo)["contract_sha256"]

    assert cli.main(["recontract", "add-greeting"]) == exits.OK

    output = capsys.readouterr().out
    state = _state(repo)
    archive = task_store.root / "contract-history" / old_digest
    assert "re-contract authorized" in output
    assert state["phase"] == "contract" and state["status"] == "stopped"
    assert state["contract_sha256"] == "" and not task_store.contract.exists()
    assert state["completed"] == {"intake": "intake-hash", "contract": "recontract-authorized"}
    assert state["plan_sha256"] == "" and state["rounds"] == {}
    assert (archive / "contract.json").is_file()
    assert (archive / "evidence/contract-base/R1-1440x900.log").read_text() == "previous base evidence\n"
    assert not (task_store.evidence_dir / "contract-base").exists()
    before_retry = store.TaskStore(repo, "add-greeting").read_state()
    assert cli.main(["recontract", "add-greeting"]) == exits.USAGE
    assert "outside a blocked or failed plan" in capsys.readouterr().out
    assert store.TaskStore(repo, "add-greeting").read_state() == before_retry


@pytest.mark.parametrize("mutation, message", [
    (lambda state: state.update(status="stopped"), "outside a blocked or failed plan"),
    (lambda state: state.update(active_runs=[{"pid": 42}]), "while runs are active"),
])
def test_recontract_refuses_unsafe_state_without_mutation(project, capsys, mutation, message):
    repo, task_store = _blocked_plan_with_contract(project)
    task_store.update_state(mutation)
    before = task_store.read_state()
    old_contract = task_store.contract.read_bytes()

    assert cli.main(["recontract", "add-greeting"]) == exits.USAGE

    assert message in capsys.readouterr().out
    assert task_store.read_state() == before
    assert task_store.contract.read_bytes() == old_contract
    assert not (task_store.root / "contract-history").exists()


@pytest.mark.parametrize("tamper_spec", [True, False])
def test_recontract_refuses_changed_spec_or_contract_digest(project, capsys, tamper_spec):
    repo, task_store = _blocked_plan_with_contract(project)
    before = task_store.read_state()
    if tamper_spec:
        task_store.spec.write_text(task_store.spec.read_text() + "\nUnapproved edit.\n")
        message = "approved spec changed"
    else:
        task_store.contract.write_text(task_store.contract.read_text() + " ")
        message = "contract.json changed outside the engine"

    assert cli.main(["recontract", "add-greeting"]) == exits.USAGE

    assert message in capsys.readouterr().out
    assert task_store.read_state() == before
    assert not (task_store.root / "contract-history").exists()


def test_recontract_failure_after_archive_leaves_state_unchanged(project, capsys, monkeypatch):
    repo, task_store = _blocked_plan_with_contract(project)
    old_digest = _state(repo)["contract_sha256"]
    before = task_store.read_state()
    old_contract = task_store.contract.read_bytes()

    def failing_write(self, change):
        raise OSError("disk full")

    with monkeypatch.context() as patched:
        patched.setattr(store.TaskStore, "update_state", failing_write)
        assert cli.main(["recontract", "add-greeting"]) == exits.USAGE

    assert "disk full" in capsys.readouterr().out
    archive = task_store.root / "contract-history" / old_digest
    assert (archive / "contract.json").is_file()
    assert task_store.read_state() == before
    assert task_store.contract.read_bytes() == old_contract
    assert (task_store.evidence_dir / "contract-base/R1-1440x900.log").is_file()
    assert cli.main(["recontract", "add-greeting"]) == exits.OK
    assert _state(repo)["phase"] == "contract"


def test_recontract_failure_in_authorize_leaves_contract_and_state(project, capsys, monkeypatch):
    repo, task_store = _blocked_plan_with_contract(project)
    before = task_store.read_state()
    old_contract = task_store.contract.read_bytes()

    def tampered(*_args, **_kwargs):
        raise contract.ContractTampered("contract.json changed outside the engine")

    with monkeypatch.context() as patched:
        patched.setattr(contract, "authorize_replan", tampered)
        assert cli.main(["recontract", "add-greeting"]) == exits.USAGE

    assert "changed outside the engine" in capsys.readouterr().out
    assert task_store.read_state() == before
    assert task_store.contract.read_bytes() == old_contract
    assert (task_store.evidence_dir / "contract-base/R1-1440x900.log").is_file()
    assert cli.main(["recontract", "add-greeting"]) == exits.OK


CLAUDE_SESSION = "9b5c773d-e4a1-4311-bca3-43830f5ff541"
INIT_EVENT = json.dumps({"type": "system", "subtype": "init", "session_id": CLAUDE_SESSION})


def _record_run(repo, name, provider="claude", model="sonnet", role="builder", finished=True, identity=None):
    task_store = store.TaskStore(repo, "add-greeting")
    run_records.begin(task_store, name, role, "build", provider, model, "low", "local", repo, {})
    run_records.mark_running(task_store, name, identity or {"pid": os.getpid(), "command": process.command_of(os.getpid())})
    if finished:
        run_records.log_path(task_store, name).write_text(INIT_EVENT + "\n")
        run_records.finish(task_store, name, 0)


def test_sessions_json_lists_every_run_with_host_state_and_safe_resume(project, capsys):
    repo, _ = project
    _start(project)
    _record_run(repo, "build-1")
    _record_run(repo, "plan-1", provider="exec", model="", role="planner")
    _record_run(repo, "review-1", provider="codex", model="", role="code_reviewer", finished=False)
    capsys.readouterr()

    assert cli.main(["sessions", "add-greeting", "--json"]) == exits.OK
    rows = {row["name"]: row for row in json.loads(capsys.readouterr().out)}

    assert set(rows) == {"intake-1", "build-1", "plan-1", "review-1"}, "the intake run of start is recorded too"
    build = rows["build-1"]
    assert (build["host"], build["state"], build["session_id"]) == ("local", "finished", CLAUDE_SESSION)
    assert build["resume"][:3] == ["claude", "--resume", CLAUDE_SESSION]
    assert "--dangerously-skip-permissions" not in build["resume"]
    assert rows["plan-1"]["resume"] is None and "no session id" in rows["plan-1"]["not_resumable"]
    assert rows["review-1"]["resume"] is None and "stop it first" in rows["review-1"]["not_resumable"]
    assert "env" not in build and "identity" not in build


def test_sessions_text_shows_one_line_per_run_without_control_characters(project, capsys):
    repo, _ = project
    _start(project)
    _record_run(repo, "build-1", model="sonnet\x1b]52;c;ZXZpbA==\x07")
    capsys.readouterr()

    assert cli.main(["sessions", "add-greeting"]) == exits.OK
    out = capsys.readouterr().out

    line = next(line for line in out.splitlines() if line.startswith("build-1"))
    assert "finished" in line and "local" in line and CLAUDE_SESSION in line
    assert "\x1b" not in out and "\x07" not in out


def test_sessions_of_a_task_without_runs_and_of_an_unknown_task(project, capsys):
    repo, _ = project
    _start(project)
    store.TaskStore(repo, "add-greeting").update_state(lambda state: state.update(runs={}))
    capsys.readouterr()
    assert cli.main(["sessions", "add-greeting"]) == exits.OK
    assert "no runs" in capsys.readouterr().out
    assert cli.main(["sessions", "nope"]) == exits.USAGE


def test_resume_reconciles_orphans(project, capsys):
    repo, _ = project
    _start(project)
    _record_run(repo, "build-1", finished=False, identity={"pid": 2147480000, "command": "gone"})
    assert cli.main(["resume", "add-greeting"]) == exits.OK
    record = _state(repo)["runs"]["build-1"]
    assert record["state"] == "orphaned" and record["ended_at"]
    assert run_records.done_path(store.TaskStore(repo, "add-greeting"), "build-1").exists()


# -- the Opus escalation gate of a build task ----------------------------------------------------

def _with_build_task(repo, task_id="T1"):
    entry = {"status": "building", "failures": 2}
    store.TaskStore(repo, "add-greeting").update_state(lambda state: state["build"].update({task_id: entry}))


def test_approve_opus_gate_records_the_approval_for_that_task_only(project, capsys):
    repo, _ = project
    _start(project)
    _with_build_task(repo)
    _awaiting(repo, "opus:T1")
    assert cli.main(["approve", "add-greeting", "opus:T1"]) == exits.OK
    approvals = _state(repo)["approvals"]
    assert "opus:T1" in approvals and "opus:T2" not in approvals
    assert "approved; run sakura flow resume add-greeting" in capsys.readouterr().out


def test_approve_opus_gate_binds_the_approval_to_the_current_plan(project):
    repo, _ = project
    _start(project)
    _with_build_task(repo)
    store.TaskStore(repo, "add-greeting").update_state(lambda state: state.update(plan_sha256="d" * 64))
    _awaiting(repo, "opus:T1")
    assert cli.main(["approve", "add-greeting", "opus:T1"]) == exits.OK
    assert _state(repo)["approvals"]["opus:T1"]["plan_sha256"] == "d" * 64


def test_approve_opus_gate_of_an_unknown_build_task_is_usage(project, capsys):
    repo, _ = project
    _start(project)
    _awaiting(repo, "opus:T9")
    assert cli.main(["approve", "add-greeting", "opus:T9"]) == exits.USAGE
    assert "no build task T9" in capsys.readouterr().out
    assert "opus:T9" not in _state(repo)["approvals"]


def _with_preflight(repo, digest="p" * 64):
    saved = {"text": "about 8% of your week", "plan_sha256": digest}
    store.TaskStore(repo, "add-greeting").update_state(
        lambda state: state.update(plan_sha256=digest, budget={"preflight": saved}))


def test_approve_budget_gate_records_the_approved_plan_digest(project, capsys):
    repo, _ = project
    _start(project)
    _with_preflight(repo)
    _awaiting(repo, "budget")
    assert cli.main(["approve", "add-greeting", "budget"]) == exits.OK
    assert _state(repo)["approvals"]["budget"]["plan_sha256"] == "p" * 64
    assert "budget approved" in store.TaskStore(repo, "add-greeting").ledger.read_text()


def test_approve_budget_gate_without_a_preflight_is_usage(project, capsys):
    repo, _ = project
    _start(project)
    _awaiting(repo, "budget")
    assert cli.main(["approve", "add-greeting", "budget"]) == exits.USAGE
    assert "no budget preflight" in capsys.readouterr().out
    assert "budget" not in _state(repo)["approvals"]


def test_approve_unknown_gate_is_usage(project, capsys):
    _start(project)
    with pytest.raises(SystemExit):
        cli.main(["approve", "add-greeting", "bogus"])


def test_opus_gate_reason_names_the_approve_command(project, capsys):
    from sakura.flow import engine
    repo, _ = project
    _start(project)
    ctx = engine.open_context(repo, store.TaskStore(repo, "add-greeting"))
    outcome = engine.PhaseOutcome("await", "T1 needs opus: 2 review rounds", gate="opus:T1")
    engine._stop_at(ctx, "build", outcome)
    reason = _state(repo)["reason"]
    assert reason.startswith("awaiting approval: sakura flow approve add-greeting opus:T1")
    assert "T1 needs opus: 2 review rounds" in reason


def test_start_with_a_file_says_it_took_the_file(project, capsys):
    assert _start(project) == exits.OK
    assert "task file specs/add-greeting.md" in capsys.readouterr().out


@pytest.mark.parametrize("flags_before,text_first", [(["-m"], False), ([], True)])
def test_start_with_task_text_saves_it_and_says_it_took_the_text(project, capsys, flags_before, text_first):
    repo, command = project
    text = "Add a greeting to the page, with a test"
    roles = ["--role", f"planner={command}", "--role", "builder=exec:true"]
    argv = ["start", text, *roles] if text_first else ["start", *flags_before, text, *roles]
    assert cli.main(argv) == exits.OK
    assert "task text from the command line" in capsys.readouterr().out
    task = store.TaskStore(repo, "add-a-greeting-to-the-page")
    assert task.input.read_text() == text + "\n"
    assert task.read_state()["ref"].startswith("text:") and task.spec.exists()


def test_start_says_the_id_of_the_lane_it_made_on_a_line_of_its_own(project, capsys):
    repo, command = project
    argv = ["start", "-m", "Add a greeting", "--new", "--role", f"planner={command}", "--role", "builder=exec:true"]
    assert cli.main(argv) == exits.OK
    assert "\nlane add-a-greeting\n" in "\n" + capsys.readouterr().out + "\n"
    assert store.TaskStore(repo, "add-a-greeting").exists()


def test_start_with_the_same_task_text_again_resumes_the_same_task(project, capsys):
    repo, command = project
    argv = ["start", "-m", "Add a greeting", "--role", f"planner={command}", "--role", "builder=exec:true"]
    assert cli.main(argv) == exits.OK
    capsys.readouterr()
    assert cli.main(argv) == exits.OK
    assert "resuming add-a-greeting" in capsys.readouterr().out


def test_start_with_a_file_and_task_text_is_refused(project, capsys):
    assert cli.main(["start", "specs/add-greeting.md", "-m", "Add a greeting"]) == exits.USAGE
    assert "either a file or the task text" in capsys.readouterr().out


def test_start_with_a_missing_file_name_does_not_turn_it_into_task_text(project, capsys):
    assert _start_ref(project, "specs/typo.md") == exits.BLOCKED
    assert "spec source file not found: specs/typo.md" in capsys.readouterr().out


def _start_ref(project, ref):
    repo, command = project
    return cli.main(["start", ref, "--role", f"planner={command}", "--role", "builder=exec:true"])


def test_a_waiting_line_names_the_file_to_read_and_the_exact_approve_command(project, capsys):
    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    line = _state(repo)["reason"]
    assert line == f"awaiting approval: sakura flow approve add-greeting spec (read {task.spec} first)"
    assert line in capsys.readouterr().out


def test_start_says_once_where_trust_records_stay_when_the_engine_home_was_moved(project, capsys, tmp_path, monkeypatch):
    repo, _ = project
    elsewhere = tmp_path / "account-engine"
    monkeypatch.setattr(engine_home, "root_for_trust", lambda: elsewhere)
    _start(project)
    first = capsys.readouterr().out
    notice = f"Codex trust) stay in {elsewhere}"
    assert first.count(notice) == 1 and engine_home.ENV_VARIABLE in first
    _start(project, "--new")
    assert notice not in capsys.readouterr().out


def test_start_says_nothing_about_trust_records_when_the_engine_home_is_the_account_one(project, capsys):
    _start(project)
    assert "trust records" not in capsys.readouterr().out


def test_approve_help_lists_the_checks_gate(capsys):
    with pytest.raises(SystemExit):
        cli.main(["approve", "--help"])
    assert "checks" in capsys.readouterr().out


def test_status_without_a_task_lists_every_task_with_its_phase_and_what_it_waits_for(project, capsys):
    repo, _ = project
    (repo / "specs/second.md").write_text("add a farewell\n")
    _start(project)
    cli.main(["start", "specs/second.md", "--role", f"planner={project[1]}", "--role", "builder=exec:true"])
    capsys.readouterr()

    assert cli.main(["status"]) == exits.OK
    text = capsys.readouterr().out

    assert "add-greeting  phase: intake  status: awaiting_approval" in text
    assert "second  phase: intake  status: awaiting_approval" in text
    assert "waiting: awaiting approval: sakura flow approve add-greeting spec" in text
    assert "waiting: awaiting approval: sakura flow approve second spec" in text


def test_status_without_a_task_says_when_there_are_none(repo, monkeypatch, capsys):
    monkeypatch.chdir(repo)

    assert cli.main(["status"]) == exits.OK

    assert "tasks: none" in capsys.readouterr().out


def test_status_json_without_a_task_carries_the_task_list(project, capsys):
    _start(project)
    capsys.readouterr()

    assert cli.main(["status", "--json"]) == exits.OK

    [view] = json.loads(capsys.readouterr().out)["tasks"]
    assert (view["task"], view["phase"], view["status"]) == ("add-greeting", "intake", "awaiting_approval")
    assert view["waiting"] and "sakura flow approve add-greeting spec" in view["waiting"][0]


def test_summary_of_a_finished_task_names_the_report_and_where_the_branch_went():
    state = {"status": "report_ready", "reason": "", "pr": {"url": "", "note": "branch feat/x pushed to /r.git; open the pull request yourself"}}

    text = cli._summary(state, "/engine/report.md")

    assert text.splitlines() == ["report_ready: /engine/report.md",
                                 "branch feat/x pushed to /r.git; open the pull request yourself"]


def test_summary_of_a_finished_task_with_a_pull_request_names_its_url():
    state = {"status": "report_ready", "reason": "", "pr": {"url": "https://example.test/pull/3", "head": "h"}}

    assert cli._summary(state, "/engine/report.md").splitlines()[-1] == "https://example.test/pull/3"


# -- review 6: small messages from the second real run ---------------------------------------------------------------

DENIAL = {"at": "t", "run": "build-1", "role": "builder", "tool": "Bash", "detail": "kill 4242", "reason": "denied by the cage"}


def test_a_cage_denial_is_information_never_something_the_person_must_answer(project, capsys):
    repo, _ = project
    _start(project)
    store.TaskStore(repo, "add-greeting").update_state(lambda state: state.update(cage_requests=[DENIAL]))
    capsys.readouterr()

    assert cli.main(["status", "add-greeting", "--json"]) == exits.OK
    payload = json.loads(capsys.readouterr().out)
    assert not any("denied" in item for item in payload["waiting"])
    assert payload["cage_denials"] == ["denied Bash in run build-1: kill 4242: denied by the cage"]
    assert cli.main(["status", "add-greeting"]) == exits.OK
    text = capsys.readouterr().out
    assert "waiting on you: denied" not in text and "cage denied: denied Bash in run build-1" in text
    assert cli.main(["status"]) == exits.OK
    assert "waiting: denied" not in capsys.readouterr().out


def test_stop_on_a_blocked_task_lists_it_as_stopped_not_blocked(project, capsys):
    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    task.update_state(lambda state: state.update(status="blocked", reason="a check row failed"))
    capsys.readouterr()

    assert cli.main(["stop", "add-greeting"]) == exits.OK
    assert _state(repo)["status"] == "stopped" and "a check row failed" in _state(repo)["reason"]
    assert cli.main(["status"]) == exits.OK
    listing = capsys.readouterr().out
    assert "status: stopped" in listing and "status: blocked" not in listing


def test_stop_leaves_a_running_task_to_its_engine_and_keeps_an_awaiting_one_as_it_is(project, capsys):
    repo, _ = project
    _start(project)
    task = store.TaskStore(repo, "add-greeting")
    task.update_state(lambda state: state.update(status="running"))
    with task.lock():  # an engine drives it: it holds the task lock, so the lane keeps its status and gets the stop file
        assert cli.main(["stop", "add-greeting"]) == exits.OK
    assert _state(repo)["status"] == "running" and task.stop_file.exists()


def test_recontract_drops_the_approval_of_the_commands_of_the_discarded_contract(project):
    repo, task_store = _blocked_plan_with_contract(project)
    task_store.update_state(lambda state: state["approvals"].update(checks={"sha256": "old", "commands": ["true"]}))

    assert cli.main(["recontract", "add-greeting"]) == exits.OK

    approvals = _state(repo)["approvals"]
    assert "checks" not in approvals and approvals["spec"], "the spec stays approved, the commands do not"


def test_Start_ALaneWithNoPaceChosen_StartsAtTheEnginesPace_AndKeepsItWhenTheEngineMoves(project):
    from sakura.flow import rules

    rules.set_pace("fast", None)
    _start(project)
    rules.set_pace("slowest", None)

    assert _state(project[0])["overrides"]["knobs"]["lane_pace"] == "fast"


def test_Start_ALaneThatChoseAPace_KeepsIt(project):
    from sakura.flow import rules

    rules.set_pace("fast", None)
    _start(project, "--set", "knobs.lane_pace=slowest")

    assert _state(project[0])["overrides"]["knobs"]["lane_pace"] == "slowest"


@pytest.mark.parametrize(("command", "status"), [(["stop"], "blocked"), (["cleanup", "--discard"], "failed")])
def test_stale_stop_or_discard_leaves_the_new_revision_alone(repo, monkeypatch, command, status):
    from flow_support import make_context
    from sakura.flow import resume

    task = make_context(repo, state={"status": status}).store
    monkeypatch.chdir(repo)
    expected = resume.revision(task.read_state())
    task.update_state(lambda state: state.update(reason="new failure", resume_generation=1))
    state = task.read_state()
    assert cli.main([*command, f"--revision={expected}", "demo"]) == exits.USAGE
    assert task.read_state() == state and not task.stop_file.exists()


def test_stop_marker_replaces_previous_file_privately(repo, monkeypatch):
    from flow_support import make_context

    task = make_context(repo).store
    monkeypatch.chdir(repo)
    task.stop_file.write_text("old marker")
    task.stop_file.chmod(0o644)
    assert cli.main(["stop", "demo"]) == exits.OK
    assert task.stop_file.stat().st_mode & 0o777 == 0o600
