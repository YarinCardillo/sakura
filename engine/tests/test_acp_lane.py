"""ACP turn and lane behavior exercised with local pipes and fixture agents."""

import json
import sys
from pathlib import Path

import pytest
from sakura.flow import acp, launchers, process, roles, run_log, run_records

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "acp_lane_agent.py"
SESSION = "session:fixture/one"


@pytest.fixture(autouse=True)
def fixture_process_table(tmp_path, monkeypatch, request):
    if request.node.name == "test_detached_child_is_cleared_using_real_process_table":
        return
    monkeypatch.setattr(process, "command_of", lambda pid: f"fixture adapter {pid}")

    def census(pgid, nonce, is_held, timeout=None):
        marker = tmp_path / "descendant"
        members = (pgid,)
        carriers = (int(marker.read_text()),) if marker.exists() else ()
        return process._Census(carriers, members, is_held)

    monkeypatch.setattr(process, "_census", census)


def launch(root, name, scenario, *, session="", permission=None, timeout=6):
    turn = acp.Turn(name, "fixture-v1", "deliver the note", session_id=session,
                    permission=permission, mcp_servers=({"type": "http", "name": "sakura"},))
    return launchers.LaunchRequest([sys.executable, str(FIXTURE), name, scenario], root, {}, root / "run.log",
                                   timeout, timeout, root / "STOP", acp=turn)


@pytest.mark.parametrize("name", ["claude-fixture", "codex-fixture", "goose-fixture"])
def test_fragmented_turn_delivers_only_completed_assistant_text(tmp_path, name):
    request = launch(tmp_path, name, "success")
    identities = []
    result = launchers.LocalLauncher().run(request, identities.append)
    assert result.exit_code == 0 and roles.parse_result(result.final) == {"verdict": "APPROVE"}
    log = request.log_path.read_text()
    assert run_records.session_id_from_log("goose", log) == SESSION
    assert "stderr" not in log and '"verdict":"tool"' not in log
    entries = run_log.entries(roles.RoleSpec("goose"), log)
    assert [entry.tool for entry in entries if entry.kind == "tool"] == ["workspace_command"]
    assert len(identities) == 1 and identities[0]["nonce"]
    assert (tmp_path / "closed").is_file()


@pytest.mark.parametrize("name", ["claude-fixture", "codex-fixture", "goose-fixture"])
@pytest.mark.parametrize(("scenario", "reason"), [("tool-only", "empty_assistant_reply"), ("lost", "adapter_disconnected"),
                                                   ("auth", "adapter_authentication_required"), ("refused", "turn_incomplete"),
                                                   ("detached", "adapter_left_processes"), ("bad-exit", "adapter_bad_exit"),
                                                   ("unclosed-tool", "turn_incomplete")])
def test_non_delivery_has_no_result(tmp_path, name, scenario, reason):
    request = launch(tmp_path, name, scenario)
    outcome = launchers.LocalLauncher().run(request, lambda identity: None)
    assert outcome.exit_code != 0 and outcome.final == "" and outcome.reason == reason
    assert "private diagnostic" not in request.log_path.read_text()


@pytest.mark.parametrize("allowed", [True, False])
def test_permission_uses_only_the_engine_decision(tmp_path, allowed):
    request = launch(tmp_path, "goose-fixture", "permission", permission=lambda message: allowed)
    assert launchers.LocalLauncher().run(request, lambda identity: None).exit_code == 0
    answer = json.loads((tmp_path / "permission.json").read_text())
    assert answer["result"]["outcome"] == {"outcome": "selected", "optionId": "yes" if allowed else "no"}


def test_stop_during_permission_never_delivers_partial_output(tmp_path):
    def stop(message):
        (tmp_path / "STOP").touch()
        return True

    request = launch(tmp_path, "goose-fixture", "stop", permission=stop)
    result = launchers.LocalLauncher().run(request, lambda identity: None)
    assert result.stopped and result.final == "" and result.exit_code is None
    assert "sakura_acp_prompt_accepted" not in request.log_path.read_text()
    assert (tmp_path / "cancelled").is_file()


def test_load_replay_is_private_and_unsupported_load_never_prompts(tmp_path):
    first = launch(tmp_path, "goose-fixture", "no-load", session=SESSION)
    outcome = launchers.LocalLauncher().run(first, lambda identity: None)
    assert outcome.reason == "session_load_unsupported" and not (tmp_path / "prompt.json").exists()
    second = launch(tmp_path, "goose-fixture", "success", session=SESSION)
    outcome = launchers.LocalLauncher().run(second, lambda identity: None)
    assert outcome.exit_code == 0
    assert "stale replay" not in second.log_path.read_text()
    assert roles.parse_result(outcome.final) == {"verdict": "APPROVE"}


def test_role_accepts_goose_model_names_with_colons():
    assert roles.parse_role("goose:qwen2.5:3b") == roles.RoleSpec("goose", "qwen2.5:3b")


def test_detached_child_is_cleared_using_real_process_table(tmp_path):
    request = launch(tmp_path, "goose-fixture", "detached")
    result = launchers.LocalLauncher().run(request, lambda identity: None)
    assert result.left_running and result.final == ""


@pytest.fixture
def lane(repo, tmp_path, monkeypatch):
    from contextlib import contextmanager
    from dataclasses import replace
    from types import SimpleNamespace

    from flow_support import make_context, task_workspace
    from sakura.accounts import goose, registry
    from sakura.flow import cage, goose_cage, ports, runner
    from sakura.setup import harnesses

    account = registry.Account("goose", "fixture-local", "Fixture Local", tmp_path / "account", True, None,
                               (), (), "ollama")
    monkeypatch.setattr(registry, "load", lambda **kwargs: registry.Registry((account,), False))
    monkeypatch.setattr(goose, "readiness", lambda selected: {"readiness": "ready", "reason": None})
    monkeypatch.setattr(goose, "lane_identity", lambda selected: [str(selected.config_dir), selected.goose_provider, 1])
    monkeypatch.setattr(cage, "_account_folders", lambda home: [])
    monkeypatch.setattr(ports, "is_free", lambda port: True)
    installed = SimpleNamespace(agent_name="goose-fixture", lane_models=(("ollama", "fixture-model"),),
                                adapter=SimpleNamespace(path=str(FIXTURE), identity=(1,), version="fixture-v1"),
                                recheck=lambda cwd: None)
    monkeypatch.setattr(harnesses, "qualify", lambda provider, cwd=None: installed)
    monkeypatch.setattr(goose_cage, "qualify", lambda installed, upstream, model: (goose_cage.PROVIDERS[upstream], model))
    outcome = {"scenario": "success"}

    @contextmanager
    def cage_fixture(installed, provider, upstream, model, storage, site, policy, worker):
        yield goose_cage.Launch((sys.executable, str(FIXTURE), installed.agent_name, outcome["scenario"]),
                                storage / "cwd", {}, {"type": "http", "name": "sakura"}, frozenset({"workspace_command"}))

    monkeypatch.setattr(goose_cage, "open_launch", cage_fixture)
    ctx = make_context(repo, overrides={"roles": {"planner": "goose:fixture-model"}})
    ctx = replace(ctx, runner=runner.AgentRunner(ctx.store, ctx.config))
    return ctx, task_workspace(ctx), account, outcome


def test_agent_runner_records_and_resumes_the_same_bound_goose_lane(lane):
    from sakura import repository
    from sakura.flow import agent_resume, conversations, runner

    ctx, workspace, account, _ = lane
    first = ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    assert first.result == {"verdict": "APPROVE"}
    state = ctx.store.read_state()
    record = state["runs"]["intake-1"]
    assert record["account_ref"]["id"] == account.id and record["model"] == "fixture-model"
    assert record["session_id"] == SESSION and state["active_run"] is None
    reopened = conversations.plan_reopen(record, ctx.store, state, repository.common_dir(ctx.repo))
    assert reopened.acp and not reopened.argv
    name = agent_resume.start(ctx.store, ctx.config, repository.common_dir(ctx.repo), record["name"], "continue")
    assert agent_resume.run_begun(ctx, name)
    continued = ctx.store.read_state()["runs"][name]
    assert continued["acp_binding"] == record["acp_binding"]
    assert continued["pending_text"] is None
    assert "stale replay" not in run_records.log_path(ctx.store, name).read_text()


@pytest.mark.parametrize("fresh", [False, True])
def test_flow_open_runs_goose_without_tmux(lane, monkeypatch, fresh):
    from sakura.flow import agent_resume, cli, exits, runner, store

    ctx, workspace, _, _ = lane
    ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    monkeypatch.chdir(ctx.repo)
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: pytest.fail("Goose requested tmux"))
    monkeypatch.setattr(cli.engine, "open_context", lambda repo, store: ctx)

    workers = []
    monkeypatch.setattr(agent_resume, "spawn_worker", lambda task, run, begun: workers.append((run, begun)))
    command = ["open", ctx.store.task_id, "intake-1", "--message", "continue"]
    if fresh:
        command.append("--fresh")

    assert cli.main(command) == exits.OK
    task = store.TaskStore(ctx.repo, ctx.store.task_id)
    runs = task.read_state()["runs"]
    if fresh:
        assert not workers
        assert runs["intake-2"]["resumes"] is None
        assert runs["intake-2"]["exit_code"] == 0
    else:
        [(name, begun)] = workers
        assert begun
        assert runs[name]["resumes"] == "intake-1"
        assert runs[name]["pending_text"] == "continue"


def test_stale_goose_binding_and_removed_account_block_reopen(lane, monkeypatch):
    from sakura import repository
    from sakura.accounts import goose, registry
    from sakura.flow import conversations, runner

    ctx, workspace, _, _ = lane
    ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    state = ctx.store.read_state()
    record = state["runs"]["intake-1"]
    monkeypatch.setattr(goose, "lane_identity", lambda selected: ["different"])
    assert "configuration changed" in conversations.resume_problem(record, ctx.store, state, repository.common_dir(ctx.repo))
    monkeypatch.setattr(registry, "load", lambda **kwargs: registry.Registry((), False))
    assert "account" in conversations.resume_problem(record, ctx.store, state, repository.common_dir(ctx.repo))


def test_unacknowledged_lane_steer_is_returned_on_disconnect(lane):
    from sakura.flow import runner

    ctx, workspace, _, outcome = lane
    outcome["scenario"] = "lost"
    steer = {"id": "fixture-steer", "text": "keep me", "actor": "cli:person"}
    ctx.store.update_state(lambda state: state.update(steer_pending=[steer]))
    result = ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    state = ctx.store.read_state()
    assert result.result is None and state["steer_pending"] == [steer]
    assert state["runs"]["intake-1"]["acp_failure"] == "adapter_disconnected"


def test_result_marker_before_a_tool_is_not_final_assistant_text(tmp_path):
    request = launch(tmp_path, "goose-fixture", "earlier-result")
    result = launchers.LocalLauncher().run(request, lambda identity: None)
    assert result.exit_code == 0 and roles.parse_result(result.final) is None
    assert "earlier" in request.log_path.read_text()


def test_refused_load_preserves_message_and_requires_fresh_attempt(lane):
    from sakura import repository
    from sakura.flow import agent_resume, conversations, runner
    from sakura.flow.run_view import NO, listening

    ctx, workspace, _, outcome = lane
    ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    common = repository.common_dir(ctx.repo)
    outcome["scenario"] = "no-load"
    name = agent_resume.start(ctx.store, ctx.config, common, "intake-1", "keep pending")
    assert not agent_resume.run_begun(ctx, name)
    state = ctx.store.read_state()
    continued = state["runs"][name]
    assert continued["pending_text"] == "keep pending"
    assert continued["session_id"] == SESSION and continued["acp_failure"] == "session_load_unsupported"
    assert listening(ctx.store, state, continued, common)[0] == NO
    assert "fresh lane attempt" in conversations.resume_problem(continued, ctx.store, state, common)


def test_orphaned_acp_run_clears_wait_and_requeues_steer(lane):
    ctx, workspace, account, _ = lane
    steer = {"id": "keep", "text": "Try again", "actor": "cli:person"}
    ctx.store.update_state(lambda state: state.update(steer_pending=[steer]))
    run_records.begin(ctx.store, "intake-1", "planner", "intake", "goose", "fixture-model", "", "local",
                      workspace, {}, take_steer=True, account_ref={"kind": "goose", "id": account.id},
                      acp_binding={"harness": "goose"})
    run_records.mark_running(ctx.store, "intake-1", {"pid": 1, "command": "fake"})
    run_records.set_acp_permission(ctx.store, "intake-1", True)
    ctx.store.runs_dir.mkdir(exist_ok=True)
    run_records.log_path(ctx.store, "intake-1").write_text(
        json.dumps({"type": "sakura_acp_session", "session_id": SESSION}) + "\n")
    assert run_records.reconcile(ctx.store, lambda identity: False) == ["intake-1"]
    state = ctx.store.read_state()
    record = state["runs"]["intake-1"]
    assert record["state"] == "orphaned" and record["session_id"] == SESSION
    assert "acp_permission" not in record and state["steer_pending"] == [steer]


def test_launched_adapter_stays_alive_across_the_seatbelt_exec(monkeypatch):
    tracked = {"pid": 123, "pgid": 123, "command": "sandbox wrapper", "nonce": "fixture", "transport": "acp"}
    monkeypatch.setattr(process, "command_of", lambda pid: "adapter after exec")
    monkeypatch.setattr(process, "acp_alive", lambda pid, pgid, nonce: (pid, pgid, nonce) == (123, 123, "fixture"),
                        raising=False)
    assert launchers.LocalLauncher().is_alive(tracked)


def test_fresh_goose_attempt_is_explicit_and_does_not_load_old_session(lane):
    from sakura.flow import agent_resume, runner

    ctx, workspace, account, _ = lane
    ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    result = agent_resume.fresh(ctx, "intake-1", "new instruction")
    assert result.result == {"verdict": "APPROVE"}
    state = ctx.store.read_state()
    record = state["runs"]["intake-2"]
    assert record["resumes"] is None and record["account_ref"]["id"] == account.id
    assert record["acp_binding"]["storage_run"] == "intake-2"


def test_feed_displays_earlier_reply_without_a_delivery_verdict(lane):
    from sakura.flow import agent_feed, runner

    ctx, workspace, _, outcome = lane
    outcome["scenario"] = "earlier-result"
    ctx.runner.run(runner.RunRequest("planner", "intake", "initial", {}, workspace))
    page = agent_feed.read(ctx.store, "intake-1")
    assert any("earlier" in entry["text"] for entry in page.entries if entry["kind"] == "text")
    assert not any(entry["kind"] == "result" for entry in page.entries)


@pytest.mark.parametrize("provider", ["claude", "codex"])
def test_native_fixture_transcripts_still_use_the_same_feed(lane, provider):
    from sakura.flow import agent_feed

    ctx, workspace, _, _ = lane
    name = "intake-1"
    reply = 'First line.\nSAKURA_RESULT {"verdict":"APPROVE"}'
    if provider == "claude":
        fixture_log = {"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}}
    else:
        fixture_log = {"type": "item.completed", "item": {"type": "agent_message", "text": reply}}
    run_records.begin(ctx.store, name, "planner", "intake", provider, "", "", "local", workspace, {})
    ctx.store.runs_dir.mkdir(exist_ok=True)
    run_records.log_path(ctx.store, name).write_text(json.dumps(fixture_log) + "\n")
    run_records.finish(ctx.store, name, 0)
    page = agent_feed.read(ctx.store, name)
    assert any(entry.get("result", {}).get("verdict") == "APPROVE" for entry in page.entries)
    assert any(entry.get("text") == "First line." for entry in page.entries)
