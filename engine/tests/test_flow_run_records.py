import json
from pathlib import Path

import pytest

from sakura.flow import run_records, store
from sakura.flow.run_records import RunNotResumable, RunRecordError

FIXTURES = Path(__file__).parent / "fixtures" / "runs"
CLAUDE_ID = "9b5c773d-e4a1-4311-bca3-43830f5ff541"
CODEX_ID = "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"
CODEX_LOG = f'$ codex exec --json\n{{"type":"thread.started","thread_id":"{CODEX_ID}"}}\n{{"type":"turn.completed"}}\n'


@pytest.fixture
def task(repo, tmp_path):
    """A task store whose state holds one recorded workspace outside the repository."""
    workspace = tmp_path / "workspace-0"
    workspace.mkdir()
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo", "status": "running", "workspaces": {"0": {"path": str(workspace)}}})
    return task_store


def begin(task, name="build-1", **overrides):
    arguments = {"role": "builder", "phase": "build", "provider": "claude", "model": "sonnet", "effort": "low",
                 "host": "local", "cwd": task.repo, "env": {}}
    arguments.update(overrides)
    return run_records.begin(task, name, **arguments)


def record_of(task, name="build-1"):
    return task.read_state()["runs"][name]


def finished_record(**changes):
    record = {"name": "build-1", "role": "builder", "provider": "claude", "model": "sonnet", "state": "finished",
              "session_id": CLAUDE_ID, "cwd": "/work/repo"}
    record.update(changes)
    return record


def test_session_id_claude_init():
    text = (FIXTURES / "claude-stream.jsonl").read_text()
    assert run_records.session_id_from_log("claude", text) == CLAUDE_ID


def test_session_id_codex_thread_started():
    assert run_records.session_id_from_log("codex", CODEX_LOG) == CODEX_ID


def test_session_id_rejects_non_uuid():
    forged = '{"type":"system","subtype":"init","session_id":"../../etc/passwd"}\n'
    upper = f'{{"type":"thread.started","thread_id":"{CODEX_ID.upper()}"}}\n'
    suffixed = f'{{"type":"thread.started","thread_id":"{CODEX_ID} --x"}}\n'
    assert run_records.session_id_from_log("claude", forged) == ""
    assert run_records.session_id_from_log("codex", upper) == ""
    assert run_records.session_id_from_log("codex", suffixed) == ""
    assert run_records.session_id_from_log("claude", "no json at all\n") == ""


def test_begin_rejects_cwd_outside_repo_and_workspaces(task, tmp_path):
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    with pytest.raises(RunRecordError, match="not the repository or a recorded workspace"):
        begin(task, cwd=outside)
    assert "runs" not in task.read_state() or task.read_state()["runs"] == {}
    begin(task, "build-2", cwd=tmp_path / "workspace-0")
    assert record_of(task, "build-2")["cwd"] == str((tmp_path / "workspace-0").resolve())


def test_begin_keeps_only_flow_env(task):
    env = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "builder", "ANTHROPIC_API_KEY": "secret", "PATH": "/bin"}
    begin(task, env=env)
    record = record_of(task)
    assert record["env"] == {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "builder"}
    assert record["state"] == "starting"
    assert (record["role"], record["phase"], record["provider"], record["host"]) == ("builder", "build", "claude", "local")
    assert (record["model"], record["effort"], record["session_id"], record["exit_code"]) == ("sonnet", "low", "", None)
    assert record["started_at"] and record["ended_at"] is None


def test_begin_records_default_and_custom_transcript_root(task, tmp_path, monkeypatch):
    home = tmp_path / "isolated-home"
    begin(task, "build-1")
    begin(task, "build-2", provider="codex")
    assert record_of(task, "build-1")["transcript_root"] == str(home / ".claude")
    assert record_of(task, "build-2")["transcript_root"] == str(home / ".codex")
    custom = tmp_path / "claude-config"
    begin(task, "build-3", env={"CLAUDE_CONFIG_DIR": str(custom)})
    begin(task, "build-4", provider="codex", env={"CODEX_HOME": str(custom)})
    assert record_of(task, "build-3")["transcript_root"] == str(custom.resolve())
    assert record_of(task, "build-4")["transcript_root"] == str(custom.resolve())
    begin(task, "build-5", provider="exec")
    assert record_of(task, "build-5")["transcript_root"] == ""


def test_begin_takes_pending_steer_in_the_same_update(task):
    task.update_state(lambda state: state.update(steer_pending=[{"id": "a", "text": "one"}]))
    assert begin(task, "build-1") == []
    task.update_state(lambda state: state.update(steer_pending=[{"id": "a", "text": "one"}]))
    taken = begin(task, "build-2", take_steer=True)
    assert taken == [{"id": "a", "text": "one"}]
    state = task.read_state()
    assert state["steer_pending"] == [] and state["runs"]["build-2"]["state"] == "starting"
    assert begin(task, "build-3", take_steer=True) == []


def test_begin_rejects_unsafe_run_name_and_duplicate(task):
    with pytest.raises(RunRecordError, match="run name"):
        begin(task, "../escape-1")
    begin(task, "build-1")
    with pytest.raises(RunRecordError, match="already recorded"):
        begin(task, "build-1")


def test_finish_writes_done_after_state(task):
    begin(task)
    run_records.mark_running(task, "build-1", {"pid": 4242, "command": "claude -p"})
    assert record_of(task)["state"] == "running" and record_of(task)["identity"] == {"pid": 4242, "command": "claude -p"}
    task.runs_dir.mkdir(parents=True, exist_ok=True)
    run_records.log_path(task, "build-1").write_text((FIXTURES / "claude-stream.jsonl").read_text())
    seen = []
    original = task.update_state
    task.update_state = lambda change: (original(change), seen.append(run_records.done_path(task, "build-1").exists()))[0]
    run_records.finish(task, "build-1", 0)
    assert seen == [False]
    record = record_of(task)
    assert (record["state"], record["exit_code"], record["session_id"]) == ("finished", 0, CLAUDE_ID)
    assert record["ended_at"]
    assert run_records.done_path(task, "build-1").read_text().strip() == "0"


def test_finish_aborted_without_exit_code_and_without_log(task):
    begin(task)
    run_records.finish(task, "build-1", None, aborted=True)
    record = record_of(task)
    assert (record["state"], record["exit_code"], record["session_id"]) == ("aborted", None, "")
    assert run_records.done_path(task, "build-1").exists()


def test_finish_is_idempotent_and_keeps_the_first_outcome(task):
    begin(task)
    run_records.finish(task, "build-1", 3)
    run_records.finish(task, "build-1", 0, aborted=True)
    assert record_of(task)["state"] == "finished" and record_of(task)["exit_code"] == 3


def test_paths_are_derived_from_the_run_name(task):
    assert run_records.log_path(task, "build-2") == task.runs_dir / "build-2.log"
    assert run_records.done_path(task, "build-2") == task.runs_dir / "build-2.done"
    begin(task)
    assert not any("path" in key for key in record_of(task))


def test_reconcile_marks_dead_runs_orphaned(task):
    begin(task, "build-1")
    begin(task, "build-2")
    begin(task, "build-3")
    begin(task, "build-4")
    run_records.mark_running(task, "build-1", {"pid": 1, "command": "alive"})
    run_records.mark_running(task, "build-2", {"pid": 2, "command": "dead"})
    run_records.finish(task, "build-4", 0)
    asked = []

    def is_alive(identity):
        asked.append(identity)
        return identity["command"] == "alive"

    assert run_records.reconcile(task, is_alive) == ["build-2", "build-3"]
    assert asked == [{"run": "build-1", "host": "local", "pid": 1, "command": "alive"},
                     {"run": "build-2", "host": "local", "pid": 2, "command": "dead"}]
    states = {name: run["state"] for name, run in task.read_state()["runs"].items()}
    assert states == {"build-1": "running", "build-2": "orphaned", "build-3": "orphaned", "build-4": "finished"}
    assert record_of(task, "build-2")["ended_at"]
    assert run_records.done_path(task, "build-2").exists() and run_records.done_path(task, "build-3").exists()
    assert not run_records.done_path(task, "build-1").exists()
    assert run_records.reconcile(task, is_alive) == []


def test_reconcile_keeps_the_session_id_an_orphaned_run_logged_so_it_can_be_continued(task):
    begin(task, "build-1")
    run_records.mark_running(task, "build-1", {"pid": 2, "command": "dead"})
    run_records.log_path(task, "build-1").parent.mkdir(parents=True, exist_ok=True)
    run_records.log_path(task, "build-1").write_text(
        json.dumps({"type": "system", "subtype": "init", "session_id": CLAUDE_ID}) + "\n")

    run_records.reconcile(task, lambda identity: False)

    assert record_of(task)["state"] == "orphaned" and record_of(task)["session_id"] == CLAUDE_ID


def test_resume_argv_exact_for_each_role_and_provider(tmp_path):
    common = tmp_path / "repo" / ".git"
    claude = finished_record()
    assert run_records.resume_argv(claude, common) == ["claude", "--resume", CLAUDE_ID, "--model", "sonnet", "--add-dir", str(common)]
    evaluator = finished_record(role="evaluator")
    assert run_records.resume_argv(evaluator, common)[-2:] == ["--tools", "Bash,Read"]
    writer = finished_record(provider="codex", session_id=CODEX_ID, role="builder")
    assert run_records.resume_argv(writer, common) == [
        "codex", "resume", CODEX_ID, "--sandbox", "workspace-write", "-C", "/work/repo", "--add-dir", str(common)]
    reviewer = finished_record(provider="codex", session_id=CODEX_ID, role="code_reviewer")
    assert run_records.resume_argv(reviewer, common) == [
        "codex", "resume", CODEX_ID, "--sandbox", "read-only", "-C", "/work/repo"]
    for argv in (run_records.resume_argv(claude, common), run_records.resume_argv(writer, common)):
        assert "--dangerously-skip-permissions" not in argv and "--dangerously-bypass-hook-trust" not in argv


def test_resume_refuses_running_and_missing_id(tmp_path):
    common = tmp_path / ".git"
    with pytest.raises(RunNotResumable, match=r"run build-1 is running; stop it first: sakura flow stop <task>"):
        run_records.resume_argv(finished_record(state="running"), common)
    with pytest.raises(RunNotResumable, match="no session id recorded for build-1"):
        run_records.resume_argv(finished_record(session_id=""), common)
    with pytest.raises(RunNotResumable, match="no session id recorded for build-1"):
        run_records.resume_argv(finished_record(session_id="x --dangerously-skip-permissions"), common)
    with pytest.raises(RunNotResumable, match="has no resumable provider"):
        run_records.resume_argv(finished_record(provider="exec"), common)
    for state in ("aborted", "orphaned"):
        assert run_records.resume_argv(finished_record(state=state), common)[0] == "claude"


def test_record_tamper_is_state_tamper(task):
    begin(task)
    state = json.loads(task.state_file.read_text())
    state["runs"]["build-1"]["state"] = "finished"
    state["runs"]["build-1"]["session_id"] = CLAUDE_ID
    task.state_file.write_text(json.dumps(state, indent=2, sort_keys=True))
    with pytest.raises(store.StateTampered):
        run_records.reconcile(task, lambda identity: True)


def test_session_id_from_real_codex_json_capture():
    text = (FIXTURES / "codex-json.jsonl").read_text()
    assert run_records.session_id_from_log("codex", text) == "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"
    assert all(f"/{root}/" not in text for root in ("Users", "home"))


def test_CheckedCwd_Repository_ReturnsResolved(task):
    assert run_records.checked_cwd(task, task.read_state(), task.repo) == task.repo.resolve()


def test_CheckedCwd_Elsewhere_Raises(task, tmp_path):
    with pytest.raises(RunRecordError, match="not the repository or a recorded workspace"):
        run_records.checked_cwd(task, task.read_state(), tmp_path)


def test_TranscriptRoot_ConfiguredAndDefault(tmp_path):
    configured = tmp_path / "cfg"
    assert run_records.transcript_root("claude", {"CLAUDE_CONFIG_DIR": str(configured)}) == str(configured.resolve())
    assert run_records.transcript_root("codex", {}, home=tmp_path) == str(tmp_path / ".codex")
    assert run_records.transcript_root("other", {}) == ""


def test_FlowEnv_KeepsOnlyAllowlistedStrings():
    env = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_N": 1, "PATH": "/bin"}
    assert run_records.flow_env(env) == {"SAKURA_FLOW_TASK": "demo"}


def test_begin_records_the_route_and_reason(task):
    begin(task, route="mechanical", reason="mechanical task: haiku")
    assert record_of(task)["route"] == "mechanical" and record_of(task)["reason"] == "mechanical task: haiku"


def test_begin_without_a_route_records_empty_strings(task):
    begin(task)
    assert (record_of(task)["route"], record_of(task)["reason"]) == ("", "")


def test_describe_shows_route_and_reason_and_reads_records_that_lack_them():
    shown = run_records.describe(finished_record(route="design", reason="design task: sonnet high"), Path("/common"))
    assert (shown["route"], shown["reason"]) == ("design", "design task: sonnet high")
    older = run_records.describe(finished_record(), Path("/common"))
    assert (older["route"], older["reason"]) == (None, None)


def test_begin_records_the_account_and_the_run_it_continues(task):
    begin(task, "build-1", account="work")
    begin(task, "build-1.2", account="work", resumes="build-1")
    begin(task, "review-1")

    assert (record_of(task)["account"], record_of(task)["resumes"]) == ("work", None)
    assert (record_of(task, "build-1.2")["account"], record_of(task, "build-1.2")["resumes"]) == ("work", "build-1")
    assert (record_of(task, "review-1")["account"], record_of(task, "review-1")["resumes"]) == (None, None)


@pytest.mark.parametrize("name", ["build-1.2", "build-1.2.3", "review_2-4.10"])
def test_begin_accepts_the_name_of_a_resumed_run(task, name):
    begin(task, name)

    assert record_of(task, name)["name"] == name


@pytest.mark.parametrize("name", ["build-1.", "build-1.x", "build.2", "../build-1.2", "build-1.2/x", ".build-1"])
def test_begin_rejects_a_resumed_name_that_is_not_a_log_stem(task, name):
    with pytest.raises(RunRecordError):
        begin(task, name)
