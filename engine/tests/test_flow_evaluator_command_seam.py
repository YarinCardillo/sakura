import contextlib
import io
import os
import re
import shlex
import subprocess
import sys
import threading
import time
from pathlib import Path
from string import Template
from types import SimpleNamespace

import pytest

from sakura import agent_hooks, cli
from sakura import slots
from conftest import git, trust_root_child_environment
from sakura.flow import cage, checks, completion, contract, exchange, ports, process, sealed, stack, stacks, task_repository
from sakura.flow.phases import evaluate
from flow_support import make_context, task_workspace
from sakura.flow.workspaces import KIT_BIN, Workspace


FIXTURE_URL = "http://127.0.0.1:42006"
SCREENSHOT = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000d49444154789c6360606060000000050001a5f645400000000049454e44ae426082"
)
API_STATUS = '{"status":"ready"}'


@pytest.fixture
def evaluator_programs(monkeypatch, stack_process):
    run_stack = stack_process.monitor
    upload_pack = sealed._upload_pack

    def local_upload_pack(source, common):
        words = shlex.split(upload_pack(source, common))
        assert words[:2] == [cage.SEATBELT, "-f"] and words[4:] == ["upload-pack", "--strict"]
        return shlex.join(words[3:])

    monkeypatch.setattr(sealed, "_upload_pack", local_upload_pack)
    monkeypatch.setattr(ports, "is_free", lambda _port: True)
    monkeypatch.setattr(process, "command_of", lambda pid: stack_process.command if pid == stack_process.pid else "fixture row")

    def monitor(argv, *, cwd, env, log_path, on_start, on_poll, log_file=None, keep_group=False, **kwargs):
        if keep_group:
            return run_stack(argv, cwd=cwd, env=env, on_start=on_start, on_poll=on_poll, **kwargs)
        pid = 742
        on_start(pid)
        if argv[:2] == [cage.SEATBELT, "-f"]:
            assert Path(argv[2]).is_file()
            words = shlex.split(argv[-1]) if argv[-2] == "-c" else argv[3:]
        else:
            words = argv
        code, output = replay_program(words, env, cwd, stack_process)
        if log_file is not None:
            log_file.write(output.encode())
            log_file.flush()
        else:
            log_path.parent.mkdir(parents=True, exist_ok=True)
            log_path.write_text(output)
        if on_poll:
            on_poll()
        return process.ProcessOutcome(code, False, False, False, pid)

    monkeypatch.setattr(process, "monitored_run", monitor)
    return stack_process


def replay_program(words, env, cwd, app):
    if words[0] == "sakura":
        result = _execute(cwd, shlex.join(words))
        return result.returncode, result.stdout
    assert words[0] == "python3"
    if words[1:4] == ["-m", "playwright", "screenshot"]:
        assert words[4:6] == ["--viewport-size=800,600", "--wait-for-selector=h1"]
        assert words[6] in (FIXTURE_URL, f"http://127.0.0.1:{app.port}")
        assert Path(words[7]).name == "WEB1-800x600.png"
        Path(words[7]).write_bytes(SCREENSHOT)
        return 0, ""
    assert app.is_running and cwd == app.cwd
    assert env["SAKURA_FLOW_BASE_URL"] == f"http://127.0.0.1:{app.port}"
    if words[1:] == ["web_flow.py"]:
        assert (cwd / "web_flow.py").is_file() and env["SAKURA_FLOW_VIEWPORT"] == "800x600"
        return 1, "Heading not visible: Contract heading\n"
    assert words[1:] == ["api_flow.py"] and (cwd / "api_flow.py").is_file()
    assert not env["SAKURA_FLOW_VIEWPORT"]
    return 0, f"GET /api/status\n{API_STATUS}\n"


def _rows():
    return [
        contract.Row("WEB1", "fixture", "page", "web", "800x600", "python3 flow.py web", False, False),
        contract.Row("API1", "fixture", "status", "api", None, "python3 flow.py api", True, False),
    ]


def _capture_evaluator_request(ctx, rows, base_url, phase="evaluate", evidence_path="evidence", workspace=None):
    contract.save(ctx.store, rows)
    recorder = _RequestRecorder()
    object.__setattr__(ctx, "runner", recorder)
    evaluate._agent(ctx, "evaluator", phase, _Workspace(workspace or ctx.repo), rows, base_url, evidence_path)
    return recorder.request


def test_evaluator_request_grounds_safe_commands_and_authoritative_evidence(repo):
    fixture_url = "http://127.0.0.1:41000"
    ctx = make_context(repo, own_repository=True)
    request = _capture_evaluator_request(ctx, _rows(), fixture_url)
    # The runner adds others_path when it prepares the run's exchange folder (exchange.prepare).
    prompt = Template(request.template).substitute({**request.values, "others_path": "others.md"})
    evidence_dir = str(ctx.store.evidence_dir)

    assert "call the `mcp__sakura__run_row` tool with `row_id`" in prompt
    assert "shared heavy-work governor" in prompt
    assert "`run-row ROW_ID: exit N`" in prompt
    assert "a row passes only when the engine's receipt of its `run_row` run says exit 0" in prompt
    assert "sakura flow run-row" not in prompt
    assert "PASS only when the observed result matches the row contract" in prompt
    assert "baseline rows should PASS" in prompt
    assert "non-baseline rows must be NEEDS_WORK" in prompt
    assert "Return exactly one verdict per row" in prompt
    assert "engine already provides `SAKURA_FLOW_BASE_URL`" in prompt
    assert "for web rows only, the row's `viewport`" in prompt
    assert "Capture evidence of each row's actual journey and state, including expected calibration failures" in prompt
    assert "For stateless page navigation" in prompt
    assert "For stateful, login, or interactive rows" in prompt
    assert "ROW_URL" in prompt and "ROW_READY_SELECTOR" in prompt
    assert "one shell-quoted `python3 -c` argument" in prompt
    assert evidence_dir in prompt
    assert "observed response/output" in prompt
    assert "Never reuse sample status or output as evidence" in prompt
    assert "Calibration writes go only to the supplied base evidence directory (`evidence/base/role`)" in prompt
    assert "final evaluation writes go only to the supplied final evidence directory (`evidence/role`)" in prompt
    assert "the rest of `evidence/` is the engine's, which you may cite but never write" in prompt
    assert "report evidence paths relative to the task root" in prompt
    assert request.extra_env == {"SAKURA_FLOW_BASE_URL": fixture_url, "SAKURA_FLOW_STACK": "demo:0"}

    calibration_request = _capture_evaluator_request(ctx, _rows(), fixture_url, "calibrate", "evidence/base")
    calibration_prompt = Template(calibration_request.template).substitute(
        {**calibration_request.values, "others_path": "others.md"})
    assert f"Save evidence under: {evidence_dir}/base" in calibration_prompt


def _rendered_screenshot_command(prompt, url):
    command = re.search(r"`(sakura run --heavy[^`]+)`", prompt).group(1)
    return (command.replace("ROW_ID-ROW_WIDTHxROW_HEIGHT", "WEB1-800x600")
            .replace("ROW_WIDTH,ROW_HEIGHT", "800,600")
            .replace("ROW_READY_SELECTOR", "h1")
            .replace("ROW_URL", url)
            .replace("ROW_ID", "WEB1"))


def _rendered_evidence_writer_command(prompt, row_id, text):
    expression = re.search(r"Expression shape: `([^`]+)`", prompt).group(1)
    expression = expression.replace("ROW_ID", row_id).replace("PYTHON_STRING_LITERAL", repr(text))
    return f"python3 -c {shlex.quote(expression)}"


def _write_fixture_programs(repo):
    for name in ("web_flow.py", "api_flow.py", "fixture_server.py"):
        (repo / name).write_text("raise RuntimeError('use the fixture transport')\n")
        git(repo, "add", name)
    git(repo, "commit", "-qm", "add evaluator fixtures")


def _pre_tool(repo, command, session):
    return agent_hooks._dispatch(repo, "PreToolUse", {
        "session_id": session, "tool_name": "Bash", "tool_input": {"command": command}
    })


def _execute(repo, command):
    words = shlex.split(command)
    if words[0] == "python3":
        return subprocess.run([sys.executable, *words[1:]], cwd=repo, capture_output=True, text=True, timeout=15)
    assert words[0] == "sakura"
    current = Path.cwd()
    out, err = io.StringIO(), io.StringIO()
    try:
        os.chdir(repo)
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = cli.main(words[1:])
            except SystemExit as outcome:
                code = int(outcome.code or 0)
    finally:
        os.chdir(current)
    return subprocess.CompletedProcess(words, code, out.getvalue(), err.getvalue())


def _run_allowed_commands(repo, ctx, commands, expected_codes=None, *, session="commands"):
    ctx.store.stop_file.write_text("operator stop\n")
    stopped = _pre_tool(repo, commands[0], f"eval-{session}-stopped")
    assert stopped["hookSpecificOutput"]["permissionDecision"] == "deny"
    ctx.store.stop_file.unlink()

    results = []
    for index, command in enumerate(commands):
        assert _pre_tool(repo, command, f"eval-{session}-{index}") is None
        result = _execute(repo, command)
        expected = expected_codes[index] if expected_codes else 0
        assert result.returncode == expected, result.stdout + result.stderr
        results.append(result)
    return results


def _assert_compounds_denied(repo, command):
    for index, suffix in enumerate(("; echo $?", " && true", " | cat", "\ntrue")):
        denied = _pre_tool(repo, command + suffix, f"eval-denied-{index}")
        assert denied["hookSpecificOutput"]["permissionDecision"] == "deny"


def _approved(*commands):
    """The person approved these row commands: the engine runs no contract command without it."""
    return {"approvals": {"checks": {"commands": sorted(commands)}}}


def _evaluator_overrides(tmp_path):
    return {"knobs": {"evaluator": True}, "workspace": {"root": str(tmp_path / "worktrees")}}


def _prepare_evaluator(ctx, rows, lane, base_url):
    (ctx.store.evidence_dir / "role").mkdir(parents=True, exist_ok=True)
    request = _capture_evaluator_request(ctx, rows, base_url, workspace=lane)
    prepared = exchange.prepare(ctx.store, request)
    return prepared, Template(request.template).substitute(prepared.values)


def _assert_row_receipt(ctx, rows, row_id, checkout, started_ns):
    checks.ingest_row_receipts(ctx.store)
    row = next(row for row in rows if row.id == row_id)
    assert checks.receipt_passes(ctx.store.read_state(), row, contract.rows_digest(rows), checkout.revision,
                                 started_ns, checks.EVALUATOR_RECEIPTS_KEY, ctx.store.root)


def test_real_hook_runs_browser_and_cli_fixture_and_evidence_validates(repo, tmp_path, monkeypatch, evaluator_programs):
    fixture_url = FIXTURE_URL
    ctx = make_context(repo, own_repository=True, overrides=_evaluator_overrides(tmp_path),
                       state=_approved("python3 flow.py web", "sakura status --help"))
    rows = [
        contract.Row("WEB1", "fixture", "page", "web", "800x600", "python3 flow.py web", False, False),
        contract.Row("CLI1", "fixture", "status", "cli", None, "sakura status --help", True, False),
    ]
    lane = task_workspace(ctx, "0", "main-lane")
    prepared, prompt = _prepare_evaluator(ctx, rows, lane, fixture_url)
    commands = [_rendered_screenshot_command(prompt, fixture_url), "sakura flow run-row CLI1"]
    _configure_evaluator_environment(monkeypatch, ctx, fixture_url)
    started_ns = time.time_ns()
    with completion.opened(ctx) as checkout:
        results = _run_allowed_commands(lane, ctx, commands)
    _run_allowed_commands(lane, ctx, [_rendered_evidence_writer_command(prompt, "CLI1", results[1].stdout)],
                          session="evidence")
    _assert_compounds_denied(lane, commands[1])
    verdicts = [
        {"id": "WEB1", "viewport": "800x600", "verdict": "NEEDS_WORK", "evidence": "evidence/role/WEB1-800x600.png"},
        {"id": "CLI1", "viewport": None, "verdict": "PASS", "evidence": "evidence/role/CLI1.txt"},
    ]
    assert exchange.collect(prepared, exchange.evidence_names({"rows": verdicts})) == []
    _assert_fixture_evidence(ctx.store.root / verdicts[0]["evidence"], ctx.store.root / verdicts[1]["evidence"])
    _assert_row_receipt(ctx, rows, "CLI1", checkout, started_ns)
    assert evaluate._validate_evidence(ctx, rows, verdicts, started_ns, require_pass=True) == []


def test_real_hook_runs_web_and_api_rows_through_registered_stack(repo, tmp_path, monkeypatch, evaluator_programs):
    _write_fixture_programs(repo)
    web_command, api_command = "python3 web_flow.py", "python3 api_flow.py"
    rows = [
        contract.Row("WEB1", "fixture", "page", "web", "800x600", web_command, False, False),
        contract.Row("API1", "fixture", "status", "api", None, api_command, True, False),
    ]
    ctx = make_context(repo, own_repository=True, overrides={
        **_evaluator_overrides(tmp_path),
        "knobs": {"evaluator": True},
        "app": {"start": "python3 fixture_server.py", "ready_url": "http://127.0.0.1:$SAKURA_FLOW_PORT/"},
        "checks": {"local_sockets": True},
        "load": {"slots": 1},
    }, state=_approved(web_command, api_command))
    lane = task_workspace(ctx, "0", "main-lane")
    port_start, port_end = ctx.store.read_state()["workspaces"]["0"]["ports"]
    workspace = Workspace("0", lane, "main-lane", range(port_start, port_end + 1))
    _configure_evaluator_environment(monkeypatch, ctx, "")
    with completion.opened(ctx) as checkout, stack.start(ctx, workspace, checkout) as app:
        prepared, prompt = _prepare_evaluator(ctx, rows, lane, app.base_url)
        monkeypatch.setenv("SAKURA_FLOW_BASE_URL", app.base_url)
        monkeypatch.setenv("SAKURA_FLOW_STACK", "demo:0")
        commands = ["sakura flow run-row WEB1", _rendered_screenshot_command(prompt, app.base_url),
                    "sakura flow run-row API1"]
        started_ns = time.time_ns()
        results = _run_allowed_commands(lane, ctx, commands, expected_codes=(1, 0, 0))
        api_evidence_command = _rendered_evidence_writer_command(prompt, "API1", results[2].stdout)
        _run_allowed_commands(lane, ctx, [api_evidence_command], session="evidence")
        _assert_compounds_denied(lane, commands[2])
    assert "Contract heading" in results[0].stdout
    assert results[2].stdout == 'GET /api/status\n{"status":"ready"}\n'
    verdicts = [
        {"id": "WEB1", "viewport": "800x600", "verdict": "NEEDS_WORK", "evidence": "evidence/role/WEB1-800x600.png"},
        {"id": "API1", "viewport": None, "verdict": "PASS", "evidence": "evidence/role/API1.txt"},
    ]
    assert exchange.collect(prepared, exchange.evidence_names({"rows": verdicts})) == []
    assert (ctx.store.root / verdicts[1]["evidence"]).read_text() == 'GET /api/status\n{"status":"ready"}\n'
    assert (ctx.store.root / verdicts[0]["evidence"]).read_bytes() == SCREENSHOT
    _assert_row_receipt(ctx, rows, "API1", checkout, started_ns)
    assert evaluate._validate_evidence(ctx, rows, verdicts, started_ns, require_pass=True) == []


def test_long_evaluator_row_holds_one_slot_and_refreshes_stack(repo, tmp_path, monkeypatch, evaluator_programs):
    from concurrent.futures import ThreadPoolExecutor

    app_process = evaluator_programs
    ctx = make_context(repo, own_repository=True, overrides={
        "knobs": {"evaluator": True}, "load": {"slots": 1, "idle_stack_seconds": 1},
        "app": {"start": "python3 fixture_server.py"},
    }, state=_approved("true", "printf queued"))
    lane = task_workspace(ctx, "0", "main-lane")
    row = contract.Row("LONG", "long", "active use", "api", None, "true", False, False)
    queued = contract.Row("QUEUED", "queued", "slot wait", "api", None, "printf queued", False, False)
    contract.save(ctx.store, [row, queued])
    monkeypatch.chdir(lane)
    monkeypatch.setenv("SAKURA_FLOW_STACK", "demo:0")
    monkeypatch.setenv("SAKURA_HEAVY_SLOTS", "1")
    entered, waiting, release = threading.Event(), threading.Event(), threading.Event()
    monitor, try_slots = process.monitored_run, slots.SlotQueue._try_slots
    now = SimpleNamespace(value=100)
    monkeypatch.setattr(stacks, "time", SimpleNamespace(time=lambda: now.value))
    monkeypatch.setattr(evaluate, "time", SimpleNamespace(time=lambda: now.value))

    def try_and_report(queue, ticket):
        label = slots._ticket_label(ticket)
        lease = try_slots(queue, ticket)
        if label == "contract flow QUEUED":
            waiting.set()
        return lease

    def hold_first(argv, *, on_start, on_poll, log_file=None, **kwargs):
        if kwargs.get("keep_group"):
            return monitor(argv, on_start=on_start, on_poll=on_poll, log_file=log_file, **kwargs)
        on_start(742)
        if "true" in argv:
            entered.set()
            assert release.wait(15)
            now.value += 2
        on_poll()
        return process.ProcessOutcome(0, False, False, False, 742)

    monkeypatch.setattr(slots.SlotQueue, "_try_slots", try_and_report)
    monkeypatch.setattr(process, "monitored_run", hold_first)
    queue = slots.SlotQueue(slots.default_root(), 1)
    with completion.opened(ctx) as checkout:
        app = stack.start(ctx, Workspace("0", lane, "main-lane", range(42006, 42016)), checkout)
        try:
            _configure_evaluator_environment(monkeypatch, ctx, app.base_url)
            assert _pre_tool(lane, "sakura flow run-row LONG", "eval-row-long") is None
            with ThreadPoolExecutor(max_workers=2) as workers:
                running = workers.submit(evaluate.run_row, ctx, row)
                try:
                    assert entered.wait(10)
                    second = workers.submit(evaluate.run_row, ctx, queued)
                    assert waiting.wait(10)
                    snapshot = queue.snapshot()
                    assert [item.label for item in snapshot.in_use] == ["contract flow LONG"]
                    assert [item.label for item in snapshot.waiting] == ["contract flow QUEUED"]
                finally:
                    release.set()
                assert [running.result(timeout=10), second.result(timeout=10)] == [0, 0]
            assert process.is_alive(app.pid)
            assert ctx.store.read_state()["stacks"]["demo:0"]["last_used"] == 102
            assert stacks.sweep(ctx, now=104) == ["demo:0"]
            assert not app_process.is_running
        finally:
            app.stop()


def _configure_evaluator_environment(monkeypatch, ctx, base_url):
    for key, value in {**task_repository.helper_environment(ctx.store), **trust_root_child_environment()}.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setenv("PATH", os.pathsep.join((str(KIT_BIN.parent), str(Path(sys.executable).parent), "/usr/bin:/bin")))
    monkeypatch.setenv("SAKURA_FLOW_TASK", ctx.store.task_id)
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "evaluator")
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.setenv("SAKURA_FLOW_BASE_URL", base_url)
    monkeypatch.delenv("SAKURA_FLOW_VIEWPORT", raising=False)


def _assert_fixture_evidence(screenshot, cli_evidence):
    assert screenshot.read_bytes().startswith(b"\x89PNG")
    assert "usage: sakura status" in cli_evidence.read_text()


class _RequestRecorder:
    request = None

    def run(self, request):
        self.request = request
        return type("Result", (), {"exit_code": 0, "stalled": False, "stopped": False,
                                    "final": "", "result": {"rows": []}, "log_path": Path("run.log")})()


class _Workspace:
    def __init__(self, path):
        self.path = path
        self.name = "0"
