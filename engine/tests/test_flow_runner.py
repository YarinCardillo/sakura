import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import sys
import textwrap
import threading
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, UTC
from itertools import count

import pytest

from sakura import programs
from sakura.flow import logread, ports, process, roles, run_records, runner, task_repository
from flow_support import MonitorLauncher, make_context, task_workspace

SCRIPT = textwrap.dedent("""
    import json, os, sys
    prompt = sys.stdin.read()
    print(json.dumps({k: v for k, v in os.environ.items() if k.startswith("SAKURA_FLOW_")}))
    print("prompt:", prompt)
    print('SAKURA_RESULT {"verdict": "APPROVE"}')
""")


def _ctx(repo, tmp_path, body=SCRIPT):
    script = tmp_path / "agent.py"
    script.write_text(body)
    overrides = {"roles": {"planner": f"exec:{sys.executable} {script}"}}
    return make_context(repo, overrides=overrides)


def _request(ctx, **extra):
    return runner.RunRequest("planner", "intake", "hello $name", {"name": "world"}, task_workspace(ctx),
                             {"SAKURA_FLOW_BUILD_TASK": "T1", **extra})


def test_run_sets_env_prompt_log_and_clears_active_run(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    assert result.exit_code == 0 and result.result == {"verdict": "APPROVE"}
    assert result.log_path == ctx.store.runs_dir / "intake-1.log"
    env_line = json.loads(result.final.splitlines()[0])
    assert env_line["SAKURA_FLOW_TASK"] == "demo" and env_line["SAKURA_FLOW_ROLE"] == "planner"
    assert env_line["SAKURA_FLOW_PHASE"] == "intake" and "SAKURA_FLOW_DIR" not in env_line
    exchange_folder = Path(env_line["SAKURA_FLOW_EXCHANGE"])
    assert exchange_folder.parent == task_workspace(ctx) / ".sakura-role" and ctx.store.root not in exchange_folder.parents
    assert env_line["SAKURA_FLOW_BUILD_TASK"] == "T1"
    assert "prompt: hello world" in result.final
    assert ctx.store.read_state()["active_run"] is None


def test_only_an_evaluator_run_carries_the_row_tool_environment(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    agents = runner.AgentRunner(ctx.store, ctx.config)
    extra = {"SAKURA_FLOW_BASE_URL": "http://127.0.0.1:1", "SAKURA_FLOW_STACK": "demo:0"}
    evaluator = runner.RunRequest("evaluator", "evaluate", "x", {}, ctx.repo, extra)
    assert agents._row_env(evaluator, "evaluate-1") == {
        "SAKURA_FLOW_RUN": "evaluate-1", "SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "evaluator",
        "SAKURA_FLOW_PHASE": "evaluate", **extra, **task_repository.helper_environment(ctx.store)}
    assert agents._row_env(runner.RunRequest("builder", "build", "x", {}, ctx.repo, extra), "build-1") == {}


def _record_codex_trust(workspace):
    """Every Codex role needs recorded hook trust, reviewers included, and the workspace hooks the engine installs."""
    from sakura import codex_trust, install
    install.install_files(workspace)
    codex_trust.record(workspace)


def _codex_workspace(ctx):
    """Workspace 0 of the task's own repository with the engine's hooks and recorded Codex trust."""
    workspace = task_workspace(ctx)
    _record_codex_trust(workspace)
    return workspace


@pytest.mark.parametrize("role,provider", [("planner", "claude:haiku"), ("plan_reviewer", "codex")])
def test_builtin_provider_logs_exact_launched_argv_without_prompt_or_environment(repo, tmp_path, role, provider):
    marker = "PRIVATE_PROMPT_AND_ENV_MARKER"
    ctx = make_context(repo, overrides={"dial": "save", "roles": {role: provider}})
    workspace = _codex_workspace(ctx) if provider == "codex" else task_workspace(ctx)
    captured = {}

    def monitor(argv, **kwargs):
        captured["argv"] = argv
        captured.update(kwargs)
        return process.ProcessOutcome(0, False, False, False, 1)

    request = runner.RunRequest(role, "intake", marker, {}, workspace, {"TASK15_PRIVATE_ENV": marker})
    result = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)
    metadata = result.log_path.read_text().splitlines()[0]
    recorded_argv = shlex.split(metadata.removeprefix("$ "))

    assert metadata == f"$ {shlex.join(captured['argv'])}"
    assert recorded_argv == captured["argv"]
    assert marker not in metadata and marker not in " ".join(recorded_argv)
    # A role (Claude or Codex) whose class has board_post is also told when to post (one paragraph after the prompt).
    assert captured["stdin_text"] == f"{marker}\n\n{runner.BOARD_NOTE}"
    assert captured["env"]["TASK15_PRIVATE_ENV"] == marker
    if provider.startswith("codex"):
        assert recorded_argv[:2] == [programs.path("codex", repo), "exec"] and "--sandbox" not in recorded_argv
        assert 'default_permissions="sakura-review"' in recorded_argv
        assert "model_reasoning_effort=low" in recorded_argv
    else:
        assert recorded_argv[:2] == [programs.path("claude", repo), "-p"]
        assert "--model" in recorded_argv and "haiku" in recorded_argv


def test_exec_role_does_not_log_command_and_preserves_leading_dollar_output(repo, tmp_path):
    secret = "PRIVATE_COMMAND_ARGUMENT"
    script = tmp_path / "agent.py"
    script.write_text("print('$ provider output')\n")
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} {script} {secret}"}})

    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))

    assert result.final == "$ provider output\n"
    assert secret not in result.log_path.read_text()


@pytest.fixture
def scripted_agent(repo, tmp_path, monkeypatch):
    monkeypatch.setattr(ports, "is_free", lambda _port: True)
    monkeypatch.setattr(process, "command_of", lambda pid: f"role {pid}")
    return _ctx(repo, tmp_path, "")


def test_active_run_is_recorded_while_running(scripted_agent):
    ctx = scripted_agent

    def monitor(_argv, **kwargs):
        kwargs["on_start"](100)
        active = ctx.store.read_state()["active_run"]
        assert active["pid"] == 100 and active["command"] == "role 100"
        return process.ProcessOutcome(0, False, False, False, 100)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    assert agent.run(_request(ctx)).exit_code == 0
    assert ctx.store.read_state()["active_run"] is None


def test_run_numbers_logs_sequentially(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    agent = runner.AgentRunner(ctx.store, ctx.config)
    agent.run(_request(ctx))
    assert agent.run(_request(ctx)).log_path.name == "intake-2.log"


def test_run_refuses_role_set_to_none(repo, tmp_path):
    ctx = make_context(repo, overrides={"roles": {"planner": "none"}})
    with pytest.raises(roles.RoleRefused, match="planner"):
        runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))


def test_editing_roles_are_niced_but_evaluator_is_not(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    observed = []

    def monitor(_argv, **kwargs):
        observed.append(kwargs["nice"])
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    agent.run(_request(ctx))
    agent.run(runner.RunRequest("evaluator", "evaluate", "hello", {}, task_workspace(ctx)))
    assert observed == [10, None]


def test_missing_verdict_line_gives_none_result(repo, tmp_path):
    ctx = _ctx(repo, tmp_path, "print('no verdict')")
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    assert result.result is None and result.exit_code == 0


def _run_concurrently(ctx, count_runs, observe):
    pids = count(100)
    registered = threading.Barrier(count_runs, action=observe)

    def monitor(_argv, **kwargs):
        pid = next(pids)
        kwargs["on_start"](pid)
        registered.wait(10)
        return process.ProcessOutcome(0, False, False, False, pid)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    requests = [_request(ctx)] * count_runs
    with ThreadPoolExecutor(max_workers=count_runs) as pool:
        return list(pool.map(agent.run, requests))


def test_concurrent_runs_of_one_phase_get_distinct_logs(scripted_agent):
    ctx = scripted_agent
    results = _run_concurrently(ctx, 3, lambda: None)
    assert sorted(result.log_path.name for result in results) == ["intake-1.log", "intake-2.log", "intake-3.log"]


def test_concurrent_runs_are_all_recorded_as_active_and_cleared(scripted_agent):
    ctx = scripted_agent
    seen = []

    def observe():
        seen.extend(ctx.store.read_state()["active_runs"])

    _run_concurrently(ctx, 2, observe)
    assert sorted(run["pid"] for run in seen) == [100, 101]
    state = ctx.store.read_state()
    assert state["active_runs"] == [] and state["active_run"] is None


def test_run_records_the_run_from_begin_to_finish(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    record = ctx.store.read_state()["runs"]["intake-1"]
    assert (record["role"], record["phase"], record["provider"], record["host"]) == ("planner", "intake", "exec", "local")
    assert record["state"] == "finished" and record["exit_code"] == 0 and record["ended_at"]
    assert set(record["identity"]) == {"pid", "command", "pgid", "nonce"}
    assert record["cwd"] == str(task_workspace(ctx).resolve())
    assert record["env"]["SAKURA_FLOW_BUILD_TASK"] == "T1" and "PATH" not in record["env"]
    assert run_records.done_path(ctx.store, "intake-1").read_text() == "0\n"


def test_run_record_carries_provider_model_and_effort(repo):
    ctx = make_context(repo, overrides={"dial": "save", "roles": {"planner": "claude:haiku", "plan_reviewer": "codex"}})
    workspace = _codex_workspace(ctx)
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=launcher)
    agent.run(runner.RunRequest("planner", "intake", "x", {}, workspace, {}))
    agent.run(runner.RunRequest("plan_reviewer", "plan_review", "x", {}, workspace, {}))
    runs = ctx.store.read_state()["runs"]
    assert (runs["intake-1"]["provider"], runs["intake-1"]["model"], runs["intake-1"]["effort"]) == ("claude", "haiku", "low")
    assert (runs["plan_review-1"]["provider"], runs["plan_review-1"]["model"], runs["plan_review-1"]["effort"]) == ("codex", "", "low")


def test_spawn_failure_records_aborted(repo):
    ctx = make_context(repo, overrides={"roles": {"planner": "exec:/nonexistent/agent"}})
    with pytest.raises(OSError):
        runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    state = ctx.store.read_state()
    assert state["runs"]["intake-1"]["state"] == "aborted" and state["runs"]["intake-1"]["exit_code"] is None
    assert run_records.done_path(ctx.store, "intake-1").exists()
    assert state["active_runs"] == [] and state["active_run"] is None


def test_stall_restart_two_records(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    outcomes = [process.ProcessOutcome(None, True, False, False, 1), process.ProcessOutcome(0, False, False, False, 1)]
    launcher = MonitorLauncher(lambda argv, **kwargs: outcomes.pop(0))
    runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(_request(ctx))
    runs = ctx.store.read_state()["runs"]
    assert (runs["intake-1"]["state"], runs["intake-1"]["exit_code"]) == ("aborted", None)
    assert (runs["intake-2"]["state"], runs["intake-2"]["exit_code"]) == ("finished", 0)


def test_cwd_outside_repository_and_workspaces_is_refused_before_launch(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    task_workspace(ctx)
    launched = []
    launcher = MonitorLauncher(lambda argv, **kwargs: launched.append(argv))
    elsewhere = runner.RunRequest("planner", "intake", "x", {}, tmp_path, {})
    with pytest.raises(roles.RoleRefused, match="not a workspace of the task's own repository"):
        runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(elsewhere)
    assert launched == []


def test_Runner_ARoleInThePersonsOwnCheckout_IsRefusedBeforeLaunch(repo, tmp_path):
    # The person's checkout holds their Git folder, local config included: no cage may be given it.
    ctx = _ctx(repo, tmp_path)
    task_workspace(ctx)
    launcher = MonitorLauncher(lambda argv, **kwargs: pytest.fail("launched"))
    in_checkout = runner.RunRequest("planner", "intake", "x", {}, repo, {})
    with pytest.raises(roles.RoleRefused, match="not a workspace of the task's own repository"):
        runner.AgentRunner(ctx.store, ctx.config, launcher=launcher).run(in_checkout)


TMUX_PROBE = textwrap.dedent("""
    import json, os, sys
    sys.stdin.read()
    print(json.dumps({k: os.environ.get(k) for k in ("TMUX", "TMUX_PANE", "TMUX_TMPDIR")}))
""")


def test_launched_role_gets_no_tmux_identity_and_a_private_socket_dir(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("TMUX", "/private/tmp/tmux-501/default,123,0")
    monkeypatch.setenv("TMUX_PANE", "%3")
    monkeypatch.setenv("TMUX_TMPDIR", str(tmp_path / "owner-tmux"))
    ctx = _ctx(repo, tmp_path, TMUX_PROBE)
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    seen = json.loads(result.final.splitlines()[0])
    assert seen["TMUX"] is None and seen["TMUX_PANE"] is None
    assert seen["TMUX_TMPDIR"] and seen["TMUX_TMPDIR"] != str(tmp_path / "owner-tmux")


# -- codex review runs fail closed when the engine hooks never ran ------------------------------

COMMAND_RUN = ('{"type":"item.completed","item":{"id":"i1","type":"command_execution",'
               '"command":"ls","aggregated_output":"","exit_code":0,"status":"completed"}}\n')
MESSAGE_ONLY = '{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"done"}}\n'


def _leave_mark(task_store, run):
    """What the engine hook does on a call of `run`."""
    mark = task_store.hook_mark(run)
    mark.parent.mkdir(parents=True, exist_ok=True)
    mark.touch()


def _old_mark_in_task_store(task_store, run):
    """The mark location before H3: inside the task store, where a codex role can write."""
    task_store.runs_dir.mkdir(parents=True, exist_ok=True)
    (task_store.runs_dir / f"{run}.hook-mark").touch()


def _codex_review_run(repo, log_text, leaves_mark):
    ctx = make_context(repo, overrides={"roles": {"code_reviewer": "codex"}})
    workspace = _codex_workspace(ctx)

    def monitor(argv, **kwargs):
        with kwargs["log_path"].open("a") as log:
            log.write(log_text)
        if leaves_mark:
            _leave_mark(ctx.store, kwargs["env"]["SAKURA_FLOW_RUN"])
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    return ctx, agent, runner.RunRequest("code_reviewer", "review", "x", {}, workspace)


def test_codex_review_run_run_gets_its_own_run_name_in_the_environment(repo):
    seen = {}
    ctx, _, request = _codex_review_run(repo, MESSAGE_ONLY, False)

    def monitor(argv, **kwargs):
        seen.update(kwargs["env"])
        return process.ProcessOutcome(0, False, False, False, 1)

    result = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)
    assert seen["SAKURA_FLOW_RUN"] == result.log_path.stem == "review-1"


def test_codex_review_run_with_commands_and_no_hook_mark_is_blocked_and_ledgered(repo):
    ctx, agent, request = _codex_review_run(repo, COMMAND_RUN, leaves_mark=False)
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)
    assert "[review] codex hooks did not run (review-1)" in ctx.store.ledger.read_text()


def test_codex_review_run_with_commands_and_a_hook_mark_runs_on(repo):
    ctx, agent, request = _codex_review_run(repo, COMMAND_RUN, leaves_mark=True)
    assert agent.run(request).exit_code == 0
    assert "hooks did not run" not in ctx.store.ledger.read_text()


def test_codex_review_run_without_command_executions_is_not_blocked(repo):
    ctx, agent, request = _codex_review_run(repo, MESSAGE_ONLY, leaves_mark=False)
    assert agent.run(request).exit_code == 0


def test_hook_mark_of_an_earlier_run_does_not_cover_the_next_one(repo):
    ctx, agent, request = _codex_review_run(repo, COMMAND_RUN, leaves_mark=False)
    _leave_mark(ctx.store, "review-0")
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)


def _codex_reviewer(repo, leaves_mark):
    ctx = make_context(repo, overrides={"roles": {"plan_reviewer": "codex"}})
    workspace = _codex_workspace(ctx)

    def monitor(argv, **kwargs):
        kwargs["log_path"].write_text(COMMAND_RUN)
        if leaves_mark:
            _leave_mark(ctx.store, kwargs["env"]["SAKURA_FLOW_RUN"])
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    return ctx, agent, runner.RunRequest("plan_reviewer", "plan_review", "x", {}, workspace)


def test_codex_reviewer_with_commands_and_no_hook_mark_is_blocked_and_ledgered(repo):
    ctx, agent, request = _codex_reviewer(repo, leaves_mark=False)
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)
    assert "[plan_review] codex hooks did not run (plan_review-1)" in ctx.store.ledger.read_text()


def test_codex_reviewer_with_commands_and_a_hook_mark_runs_on(repo):
    ctx, agent, request = _codex_reviewer(repo, leaves_mark=True)
    assert agent.run(request).exit_code == 0


# -- every tool item needs a hook mark, for both providers ---------------------------------------


def _codex_item(kind):
    return json.dumps({"type": "item.completed", "item": {"id": "i1", "type": kind}}) + "\n"


def _claude_blocks(*kinds):
    blocks = [{"type": kind, "id": "t1", "name": "Bash", "input": {}} if kind == "tool_use" else {"type": kind, "text": "x"}
              for kind in kinds]
    return json.dumps({"type": "assistant", "message": {"role": "assistant", "content": blocks}}) + "\n"


def _provider_run(repo, provider, role, log_text, leaves_mark):
    """A one-attempt run of `role` on `provider` whose log is `log_text`; returns the agent and its request."""
    ctx = make_context(repo, overrides={"roles": {role: provider}})
    workspace = _codex_workspace(ctx)

    def monitor(argv, **kwargs):
        with kwargs["log_path"].open("a") as log:
            log.write(log_text)
        if leaves_mark:
            _leave_mark(ctx.store, kwargs["env"]["SAKURA_FLOW_RUN"])
        return process.ProcessOutcome(0, False, False, False, 1)

    phase = {"builder": "build", "code_reviewer": "review"}.get(role, "intake")
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    return ctx, agent, runner.RunRequest(role, phase, "x", {}, workspace)


@pytest.mark.parametrize("kind", ["file_change", "mcp_tool_call", "web_search", "collab_tool_call", "some_future_item"])
def test_codex_review_run_with_any_tool_item_and_no_hook_mark_is_blocked(repo, kind):
    ctx, agent, request = _provider_run(repo, "codex", "code_reviewer", _codex_item(kind), leaves_mark=False)
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)


def test_codex_review_run_with_a_file_change_and_a_hook_mark_runs_on(repo):
    ctx, agent, request = _provider_run(repo, "codex", "code_reviewer", _codex_item("file_change"), leaves_mark=True)
    assert agent.run(request).exit_code == 0


@pytest.mark.parametrize("kind", ["agent_message", "reasoning", "error", "todo_list"])
def test_codex_review_run_with_only_text_items_is_not_blocked(repo, kind):
    ctx, agent, request = _provider_run(repo, "codex", "code_reviewer", _codex_item(kind), leaves_mark=False)
    assert agent.run(request).exit_code == 0


def test_codex_review_run_with_only_a_todo_list_and_a_message_is_not_blocked(repo):
    log_text = _codex_item("todo_list") + _codex_item("agent_message")
    ctx, agent, request = _provider_run(repo, "codex", "code_reviewer", log_text, leaves_mark=False)
    assert agent.run(request).exit_code == 0


@pytest.mark.parametrize("role", ["planner", "builder", "plan_reviewer", "evaluator"])
def test_claude_role_with_a_tool_use_and_no_hook_mark_is_blocked(repo, role):
    ctx, agent, request = _provider_run(repo, "claude:haiku", role, _claude_blocks("text", "tool_use"), leaves_mark=False)
    with pytest.raises(roles.RoleRefused, match="claude hooks did not run"):
        agent.run(request)
    assert f"claude hooks did not run ({request.phase}-1)" in ctx.store.ledger.read_text()


def test_claude_role_with_a_tool_use_and_a_hook_mark_runs_on(repo):
    ctx, agent, request = _provider_run(repo, "claude:haiku", "planner", _claude_blocks("tool_use"), leaves_mark=True)
    assert agent.run(request).exit_code == 0


def test_claude_role_with_only_text_blocks_is_not_blocked(repo):
    ctx, agent, request = _provider_run(repo, "claude:haiku", "planner", _claude_blocks("text", "thinking"), leaves_mark=False)
    assert agent.run(request).exit_code == 0


# -- the hook check covers every attempt of a restarted run --------------------------------------

STALLED = process.ProcessOutcome(None, True, False, False, 1)
CLEAN = process.ProcessOutcome(0, False, False, False, 1)


def _two_attempts(repo, attempts):
    """A codex reviewer whose attempts are `(log text, leaves mark, outcome)`; returns ctx, agent, request."""
    ctx = make_context(repo, overrides={"roles": {"code_reviewer": "codex"}})
    workspace = _codex_workspace(ctx)
    remaining = list(attempts)

    def monitor(argv, **kwargs):
        text, leaves_mark, outcome = remaining.pop(0)
        with kwargs["log_path"].open("a") as log:
            log.write(text)
        if leaves_mark:
            _leave_mark(ctx.store, kwargs["env"]["SAKURA_FLOW_RUN"])
        return outcome

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    return ctx, agent, runner.RunRequest("code_reviewer", "review", "x", {}, workspace)


def test_stalled_first_attempt_that_ran_a_tool_without_a_mark_blocks_the_phase(repo):
    ctx, agent, request = _two_attempts(repo, [(COMMAND_RUN, False, STALLED), (MESSAGE_ONLY, False, CLEAN)])
    with pytest.raises(roles.RoleRefused, match=r"codex hooks did not run \(review-1\)"):
        agent.run(request)
    assert "[review] codex hooks did not run (review-1)" in ctx.store.ledger.read_text()


def test_restart_that_ran_a_tool_without_its_own_mark_blocks_the_phase(repo):
    ctx, agent, request = _two_attempts(repo, [(COMMAND_RUN, True, STALLED), (COMMAND_RUN, False, CLEAN)])
    with pytest.raises(roles.RoleRefused, match=r"codex hooks did not run \(review-2\)"):
        agent.run(request)


def test_both_attempts_with_their_own_marks_run_on(repo):
    ctx, agent, request = _two_attempts(repo, [(COMMAND_RUN, True, STALLED), (COMMAND_RUN, True, CLEAN)])
    assert agent.run(request).exit_code == 0
    assert "stall restart review" in ctx.store.ledger.read_text()


# -- the mark must be unforgeable and fresh -------------------------------------------------------


def test_a_mark_in_the_task_store_does_not_count(repo):
    ctx, agent, request = _two_attempts(repo, [(COMMAND_RUN, False, CLEAN)])
    _old_mark_in_task_store(ctx.store, "review-1")
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)


def test_a_stale_mark_older_than_the_run_start_does_not_count(repo):
    import os
    ctx, agent, request = _two_attempts(repo, [(COMMAND_RUN, False, CLEAN)])
    _leave_mark(ctx.store, "review-1")
    os.utime(ctx.store.hook_mark("review-1"), (1, 1))
    with pytest.raises(roles.RoleRefused, match="codex hooks did not run"):
        agent.run(request)


# -- budget account: the config dir of the account a claude role runs on -------------------------

def _claude_env(repo, tmp_path, account, role="builder"):
    """Run a claude role with `budget.account` set and return the environment it was launched with."""
    ctx = make_context(repo, overrides={"budget": {"account": account}, "roles": {role: "claude:sonnet"}})
    seen = {}

    def monitor(argv, **kwargs):
        seen.update(kwargs["env"])
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    with pytest.MonkeyPatch.context() as patch:  # the live budget guard has its own tests in test_flow_engine_budget.py
        patch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
        agent.run(runner.RunRequest(role, "build", "x", {}, task_workspace(ctx)))
    return ctx, seen


def _usage_toml(home, text):
    path = home / ".config/sakura/usage.toml"
    path.parent.mkdir(parents=True)
    path.write_text(text)


def test_role_env_sets_config_dir_of_the_budget_account(repo, tmp_path, monkeypatch):
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    _usage_toml(home, f'[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "{home}/.claude-work/projects"\n')
    _, env = _claude_env(repo, tmp_path, "work")
    assert env["CLAUDE_CONFIG_DIR"] == str(home / ".claude-work")


def test_default_account_leaves_config_dir_unset(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    (tmp_path / "home" / ".claude" / "projects").mkdir(parents=True)  # detected as the account 'default'
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/inherited")
    _, env = _claude_env(repo, tmp_path, "default")
    assert "CLAUDE_CONFIG_DIR" not in env


def test_empty_budget_account_leaves_the_environment_as_it_is(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/inherited")
    _, env = _claude_env(repo, tmp_path, "")
    assert env["CLAUDE_CONFIG_DIR"] == "/inherited"


def test_unknown_budget_account_refuses_the_role_and_ledgers_it(repo, monkeypatch):
    monkeypatch.setenv("HOME", str(repo.parent / "home"))
    ctx = make_context(repo, overrides={"budget": {"account": "ghost"}, "roles": {"builder": "claude:sonnet"}})
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(lambda argv, **kwargs: pytest.fail("launched")))
    with pytest.raises(roles.RoleRefused, match="'ghost'"):
        agent.run(runner.RunRequest("builder", "build", "x", {}, task_workspace(ctx)))
    assert "[build] budget account 'ghost'" in ctx.store.ledger.read_text()


def test_a_malformed_usage_config_refuses_the_role_and_ledgers_it(repo, tmp_path, monkeypatch):
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    _usage_toml(home, "this is not toml [")
    monkeypatch.setattr(runner.budget_guard, "require", lambda *args, **kwargs: None)
    ctx = make_context(repo, overrides={"budget": {"account": "work"}, "roles": {"builder": "claude:sonnet"}})
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(lambda argv, **kwargs: pytest.fail("launched")))
    with pytest.raises(roles.RoleRefused, match="cannot read usage config"):
        agent.run(runner.RunRequest("builder", "build", "x", {}, task_workspace(ctx)))
    assert "[build] " in ctx.store.ledger.read_text() and "cannot read usage config" in ctx.store.ledger.read_text()


def test_budget_account_does_not_touch_a_non_claude_role(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/inherited")
    script = tmp_path / "agent.py"
    script.write_text("import os, sys\nsys.stdin.read()\nprint(os.environ.get('CLAUDE_CONFIG_DIR'))\n")
    ctx = make_context(repo, overrides={"budget": {"account": "ghost"},
                                        "roles": {"planner": f"exec:{sys.executable} {script}"}})
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    # An exec role sees neither the account's folder nor the engine's own provider variable (role environments are allow-lists).
    assert result.final.splitlines()[0] == "None"


# -- model routing: the router fills roles left at their defaults ---------------------------------

def _routed_run(repo, role, phase, overrides=None, codex=False, **request_fields):
    """Run `role` once with the stock role config; returns the argv it was launched with and its run record."""
    ctx = make_context(repo, overrides=overrides)
    if codex:
        _codex_workspace(ctx)
    seen = {}

    def monitor(argv, **kwargs):
        seen["argv"] = argv
        return process.ProcessOutcome(0, False, False, False, 1)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    agent.run(runner.RunRequest(role, phase, "x", {}, task_workspace(ctx), **request_fields))
    return seen["argv"], ctx.store.read_state()["runs"][f"{phase}-1"]


def _flag(argv, name):
    return argv[argv.index(name) + 1]


def test_mechanical_builder_runs_on_haiku_and_records_the_route(repo):
    argv, record = _routed_run(repo, "builder", "build", task_class="mechanical")
    assert _flag(argv, "--model") == "haiku"
    assert record["model"] == "haiku" and record["route"] == "mechanical" and "mechanical" in record["reason"]


def test_design_builder_runs_on_sonnet_with_high_effort(repo):
    argv, record = _routed_run(repo, "builder", "build", task_class="design")
    assert (_flag(argv, "--model"), _flag(argv, "--effort")) == ("sonnet", "high")
    assert record["effort"] == "high"


def test_intake_runs_the_planner_on_haiku(repo):
    argv, record = _routed_run(repo, "planner", "intake")
    assert _flag(argv, "--model") == "haiku" and record["route"] == "intake"


def test_planning_phase_keeps_the_planner_role_and_records_no_route(repo):
    argv, record = _routed_run(repo, "planner", "plan")
    assert _flag(argv, "--model") == "opus"
    assert (record["route"], record["reason"]) == ("", "")


def test_explicitly_configured_builder_is_not_rerouted(repo):
    argv, record = _routed_run(repo, "builder", "build", {"roles": {"builder": "claude:opus"}}, task_class="mechanical")
    assert _flag(argv, "--model") == "opus" and record["route"] == "" and record["reason"] == ""


def test_codex_reviewer_is_not_rerouted(repo):
    argv, record = _routed_run(repo, "code_reviewer", "review", {"roles": {"code_reviewer": "codex"}}, codex=True)
    assert argv[0] == programs.path("codex", repo) and record["route"] == ""


def test_opus_route_without_the_owner_approval_is_refused(repo):
    ctx = make_context(repo)
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(lambda argv, **kwargs: pytest.fail("launched")))
    request = runner.RunRequest("builder", "build", "x", {}, task_workspace(ctx), task_class="integration", failures=2)
    with pytest.raises(roles.RoleRefused, match="owner approval"):
        agent.run(request)
    assert "[build]" in ctx.store.ledger.read_text()


def test_approved_opus_route_runs_on_opus(repo):
    argv, record = _routed_run(repo, "builder", "build", task_class="integration", failures=2, opus_approved=True)
    assert _flag(argv, "--model") == "opus" and record["route"] == "integration"


# -- output cap and bounded reads ----------------------------------------------------------------------------------

def test_a_role_that_floods_its_output_is_stopped_and_the_run_says_so(repo, tmp_path, monkeypatch):
    monkeypatch.setattr(process, "OUTPUT_CAP_BYTES", 2 * 1024 * 1024)
    ctx = _ctx(repo, tmp_path, "import sys\nwhile True:\n    sys.stdout.write('x' * 65536)\n    sys.stdout.flush()\n")
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    assert result.overflowed and result.exit_code is None and not result.stalled
    assert "output cap exceeded" in ctx.store.ledger.read_text()
    assert ctx.store.read_state()["active_run"] is None
    assert len(result.final.encode()) <= logread.OUTPUT_CAP_BYTES


def test_a_log_replaced_by_a_fifo_fails_the_run_without_blocking(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)

    def swap(argv, **kwargs):
        kwargs["log_path"].unlink()
        os.mkfifo(kwargs["log_path"])
        return process.ProcessOutcome(0, False, False, False, 1)

    previous = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(AssertionError("the read blocked")))
    signal.alarm(5)
    try:
        with pytest.raises(roles.RoleRefused, match="not a regular file"):
            runner.AgentRunner(ctx.store, ctx.config, MonitorLauncher(swap)).run(_request(ctx))
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)


def test_an_overflowed_run_blocks_the_phase_with_the_cap_named(tmp_path):
    from sakura.flow.phases import runs
    result = runner.RunResult(None, "", None, tmp_path / "intake-1.log", False, False, overflowed=True)
    outcome = runs.failure_outcome(result, "planner", "intake")
    assert outcome.kind == "blocked" and "output cap exceeded" in outcome.reason


def test_a_run_whose_log_was_replaced_blocks_the_phase_naming_it(tmp_path):
    from sakura.flow.phases import runs
    result = runner.RunResult(None, "", None, tmp_path / "intake-1.log", False, False, log_replaced=True)
    outcome = runs.failure_outcome(result, "planner", "intake")
    assert outcome.kind == "blocked" and "log replaced" in outcome.reason


# -- role environment: an allow-list, never the engine's own environment -----------------------------------

ENV_DUMP = "import json, os, sys\nsys.stdin.read()\nprint(json.dumps(dict(os.environ)))\n"
ENGINE_SECRETS = {"SECRET_CANARY": "engine-only", "AWS_SECRET_ACCESS_KEY": "k", "GITHUB_TOKEN": "t",
                  "ANTHROPIC_API_KEY": "a", "SAKURA_QUOTA_URL": "http://q", "HTTPS_PROXY": "http://p"}


def _dumped_role_env(repo, tmp_path, monkeypatch):
    for name, value in ENGINE_SECRETS.items():
        monkeypatch.setenv(name, value)
    ctx = _ctx(repo, tmp_path, ENV_DUMP)
    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))
    return json.loads(result.final.splitlines()[0])


def test_RoleEnvironment_PlantedEngineSecrets_AreNotVisibleToTheRole(repo, tmp_path, monkeypatch):
    seen = _dumped_role_env(repo, tmp_path, monkeypatch)
    assert not set(ENGINE_SECRETS) & set(seen)
    assert "engine-only" not in json.dumps(seen)


def test_RoleEnvironment_Allowlist_KeepsWhatARoleNeedsToRun(repo, tmp_path, monkeypatch):
    seen = _dumped_role_env(repo, tmp_path, monkeypatch)
    assert seen["PATH"] == os.environ["PATH"] and seen["HOME"] == os.environ["HOME"]
    assert seen["SAKURA_FLOW_TASK"] == "demo" and "TMUX_TMPDIR" in seen and "TMUX" not in seen


def test_RoleEnvironment_ClaudeRole_KeepsOnlyItsConfigDirectoryVariable(repo, tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/inherited")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "a")
    monkeypatch.setenv("SECRET_CANARY", "engine-only")
    _, env = _claude_env(repo, tmp_path, "")
    assert env["CLAUDE_CONFIG_DIR"] == "/inherited"
    assert "ANTHROPIC_API_KEY" not in env and "SECRET_CANARY" not in env


def test_RoleEnvironment_ClaudeRole_GetsTheEngineHomePathForItsHooks(repo, tmp_path):
    # A path, not a secret: the cage still denies the folder; the hooks need it to find the task's state.
    _, claude = _claude_env(repo, tmp_path, "")
    assert claude["SAKURA_ENGINE_HOME"] == os.environ["SAKURA_ENGINE_HOME"]


def test_RoleEnvironment_CodexRole_GetsTheEngineHomePathForItsHooks(repo):
    seen = {}
    ctx, _, request = _codex_review_run(repo, MESSAGE_ONLY, False)

    def monitor(argv, **kwargs):
        seen.update(kwargs["env"])
        return process.ProcessOutcome(0, False, False, False, 1)

    runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor)).run(request)
    assert seen["SAKURA_ENGINE_HOME"] == os.environ["SAKURA_ENGINE_HOME"]


def test_RoleEnvironment_ExecRoles_DoNotGetTheEngineHome(repo, tmp_path, monkeypatch):
    assert "SAKURA_ENGINE_HOME" not in _dumped_role_env(repo, tmp_path, monkeypatch)


# -- review 11, security review: an isolated run's own tmp folder ------------------------------------------------------

ISOLATED_SCRIPT = textwrap.dedent("""
    import json, os, sys
    sys.stdin.read()
    target, seen = os.environ.get("PROBE", ""), ""
    if target:
        try:
            seen = open(target).read()
        except OSError as error:
            seen = "refused: " + (error.strerror or str(error))
    print("SAKURA_RESULT " + json.dumps({"tmp": os.environ["TMPDIR"], "seen": seen}))
""")


def test_IsolatedRun_ALaterRoleCannotReadWhatWasLeftInAnEarlierIsolatedRunsTmpFolder(repo, tmp_path):
    script = tmp_path / "isolated.py"
    script.write_text(ISOLATED_SCRIPT)
    overrides = {"roles": {"reporter": f"exec:{sys.executable} {script}"},
                 "workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []}}
    ctx = make_context(repo, overrides=overrides, own_repository=True)
    task_workspace(ctx)
    agents = runner.AgentRunner(ctx.store, ctx.config)

    def isolated(probe=""):
        repository, worktree = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, "report")
        try:
            return agents.run(runner.RunRequest("reporter", "report", "x", {}, worktree, {"PROBE": probe},
                                                repository=repository))
        finally:
            task_repository.remove_role_repository(ctx.store, repository)

    earlier = Path(isolated().result["tmp"])
    assert not earlier.exists(), "an isolated run's own tmp folder goes when the run ends"
    earlier.mkdir()  # a detached survivor of that run makes its folder again and leaves a file there
    (earlier / "planted.txt").write_text("left by a survivor\n")

    try:
        later = isolated(str(earlier / "planted.txt"))
    finally:
        shutil.rmtree(earlier)  # what the survivor made again, outside the test's own folder

    assert later.result["seen"].startswith("refused") and Path(later.result["tmp"]) != earlier
    folder = ctx.store.root / "cage" / later.log_path.stem
    settings = json.loads((folder / "settings.json").read_text())
    for tool in ("Read", "Edit"):
        assert f"{tool}(/{earlier}/**)" in settings["permissions"]["deny"]
    assert str(earlier) in settings["sandbox"]["filesystem"]["denyRead"]
    assert f'(subpath "{earlier}")' in (folder / "sandbox.sb").read_text()


def test_IsolatedRun_ALaterRoleCannotReadAForgottenEarlierRoleRepository(repo, tmp_path):
    # Security review of review 11: a role repository goes (and is dropped from the state) when its run ends, but a
    # detached survivor of that run can make its folders again; every later isolated run is denied them still.
    script = tmp_path / "isolated.py"
    script.write_text(ISOLATED_SCRIPT)
    overrides = {"roles": {"reporter": f"exec:{sys.executable} {script}"},
                 "workspace": {"root": str(tmp_path / "trees" / "{repo}"), "setup": []}}
    ctx = make_context(repo, overrides=overrides, own_repository=True)
    task_workspace(ctx)
    agents = runner.AgentRunner(ctx.store, ctx.config)
    made = []

    def isolated(probe=""):
        repository, worktree = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, "review")
        made.append((repository, worktree))
        try:
            return agents.run(runner.RunRequest("reporter", "report", "x", {}, worktree, {"PROBE": probe},
                                                repository=repository))
        finally:
            task_repository.remove_role_repository(ctx.store, repository)

    isolated()
    earlier_repository, earlier_worktree = made[0]
    assert not earlier_worktree.exists() and not ctx.store.read_state().get(task_repository.ROLE_REPOSITORIES_KEY)
    earlier_worktree.mkdir()  # a survivor of that run makes its folder again and leaves a file there
    (earlier_worktree / "claims.md").write_text("left by a survivor\n")

    later = isolated(str(earlier_worktree / "claims.md"))

    assert later.result["seen"].startswith("refused")
    folder = ctx.store.root / "cage" / later.log_path.stem
    settings = json.loads((folder / "settings.json").read_text())
    for path in (earlier_repository, earlier_worktree):
        assert f"Read(/{os.path.realpath(path)}/**)" in settings["permissions"]["deny"]
        assert f'(subpath "{os.path.realpath(path)}")' in (folder / "sandbox.sb").read_text()


# -- the account of a run, the names of resumed runs, and the times of a log ----------------------

def test_run_record_names_the_budget_account_the_run_was_launched_under(repo, tmp_path, monkeypatch):
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    _usage_toml(home, f'[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "{home}/.claude-work/projects"\n')

    ctx, _ = _claude_env(repo, tmp_path, "work")

    assert ctx.store.read_state()["runs"]["build-1"]["account"] == "work"


def test_run_record_without_a_named_account_has_none(repo, tmp_path, monkeypatch):
    ctx, _ = _claude_env(repo, tmp_path, "")

    assert ctx.store.read_state()["runs"]["build-1"]["account"] is None


def test_run_record_names_the_account_a_pinned_step_runs_on(repo, tmp_path, monkeypatch):
    row = {"id": "acct-7", "label": "Personal", "provider": "claude", "config_dir": str(tmp_path / "personal")}
    monkeypatch.setattr(runner.step_accounts, "chosen_row", lambda *args, **kwargs: row)
    monkeypatch.setattr(runner.step_accounts, "apply", lambda env, *args, **kwargs: {**env, "CLAUDE_CONFIG_DIR": row["config_dir"]})

    ctx, env = _claude_env(repo, tmp_path, "")

    assert env["CLAUDE_CONFIG_DIR"] == row["config_dir"]
    assert ctx.store.read_state()["runs"]["build-1"]["account"] == "Personal"


def test_run_numbers_ignore_the_logs_of_resumed_runs(repo, tmp_path):
    ctx = _ctx(repo, tmp_path)
    ctx.store.runs_dir.mkdir(parents=True, exist_ok=True)
    (ctx.store.runs_dir / "intake-1.2.log").write_text("")

    result = runner.AgentRunner(ctx.store, ctx.config).run(_request(ctx))

    assert result.log_path.name == "intake-1.log"


def test_run_marks_the_times_its_log_grew(scripted_agent, monkeypatch):
    from sakura.flow import run_times

    ctx = scripted_agent
    start = datetime(2026, 11, 22, 12, tzinfo=UTC)
    clock = iter((start, start + timedelta(seconds=1), start + timedelta(seconds=2)))
    marker = run_times.Marker
    monkeypatch.setattr(run_times, "Marker", lambda log: marker(log, now=lambda: next(clock)))
    sizes = []

    def monitor(_argv, **kwargs):
        kwargs["on_start"](100)
        log = kwargs["log_path"]
        with log.open("a") as stream:
            for line in ("first\n", "second\n"):
                stream.write(line)
                stream.flush()
                sizes.append(log.stat().st_size)
                kwargs["on_poll"]()
        return process.ProcessOutcome(0, False, False, False, 100)

    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))
    result = agent.run(_request(ctx))
    assert run_times.time_at(result.log_path, 1) == "2026-11-22T12:00:00+00:00"
    assert run_times.time_at(result.log_path, sizes[0] + 1) == "2026-11-22T12:00:01+00:00"
    assert run_times.time_at(result.log_path, result.log_path.stat().st_size) == "2026-11-22T12:00:01+00:00"


# -- the step a run serves is on its record

def test_run_record_names_the_step_of_the_role_or_the_one_the_request_names(repo):
    ctx = make_context(repo, overrides={"roles": {"planner": "claude:haiku", "code_reviewer": "claude:haiku"}})
    workspace = task_workspace(ctx)
    launcher = MonitorLauncher(lambda argv, **kwargs: process.ProcessOutcome(0, False, False, False, 1))
    agent = runner.AgentRunner(ctx.store, ctx.config, launcher=launcher)
    import sakura.flow.runner as runner_module
    patch = pytest.MonkeyPatch()
    patch.setattr(runner_module.budget_guard, "require", lambda *args, **kwargs: None)
    try:
        agent.run(runner.RunRequest("planner", "intake", "x", {}, workspace, {}))
        agent.run(runner.RunRequest("code_reviewer", "review", "x", {}, workspace, {}))
        agent.run(runner.RunRequest("code_reviewer", "review", "x", {}, workspace, {}, step="plan_review"))
    finally:
        patch.undo()

    runs = ctx.store.read_state()["runs"]
    assert [runs[name]["step"] for name in ("intake-1", "review-1", "review-2")] == ["plan", "task_review", "plan_review"]
