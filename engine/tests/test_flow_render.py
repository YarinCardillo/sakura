from __future__ import annotations

import io
import json
from types import SimpleNamespace
from pathlib import Path

import pytest

from sakura.flow import cli, exits, render, run_records, store


def test_strip_controls_attack_string():
    assert render.strip_controls("a\x1b]52;c;ZXZpbA==\x07b\x1b]2;x\x1b\\c\x1b[2Jd\x1b") == "abcd"


def test_strip_controls_drops_lone_surrogates_a_terminal_or_utf8_encoder_cannot_take():
    assert render.strip_controls("a\udcffb") == "ab"
    assert render.strip_controls("a\udcffb").encode("utf-8") == b"ab"


def test_strip_c1_and_keeps_newline_tab():
    text = "a\x9b31mb\x9d0;t\x9cc\x07\x00\x7f\x85d\n\te\r"
    assert render.strip_controls(text) == "abcd\n\te"


def test_strip_string_sequences_and_unterminated_osc():
    assert render.strip_controls("x\x1bPq#0\x1b\\y\x1b_apc\x07z\x1b^pm\x1b\\w\x1bXsos\x1b\\v") == "xyzwv"
    assert render.strip_controls("keep\x1b]0;title never ends") == "keep"


def test_strip_two_byte_escape_and_plain_text_unchanged():
    assert render.strip_controls("a\x1b7b\x1b(Bc") == "abc"
    assert render.strip_controls("caffè ✳ ⠋ ok") == "caffè ✳ ⠋ ok"


INVISIBLE = "\u202e\u2066\u2067\u2068\u2069\u200b\u2028\u2029\ufeff\u00ad"


def test_strip_controls_removes_bidi_zero_width_and_line_separators_but_keeps_newline_and_tab():
    assert render.strip_controls(f"a{INVISIBLE}b\n\tc") == "ab\n\tc"


def test_strip_controls_keeps_non_ascii_letters_and_symbols():
    assert render.strip_controls("caf\u00e8 \u2713 \u05e9\u05dc\u05d5\u05dd") == "caf\u00e8 \u2713 \u05e9\u05dc\u05d5\u05dd"


# -- render_line ------------------------------------------------------------

FIXTURES = Path(__file__).parent / "fixtures" / "runs"
CLAUDE_ID = "9b5c773d-e4a1-4311-bca3-43830f5ff541"


def _rendered(provider, text):
    lines = (render.render_line(provider, line) for line in text.splitlines())
    return [line for line in lines if line is not None]


def test_render_claude_fixture_golden():
    text = (FIXTURES / "claude-stream.jsonl").read_text()
    assert _rendered("claude", text) == [
        "$ claude -p --output-format stream-json --verbose --model haiku --effort low",
        f"session {CLAUDE_ID} model claude-haiku-5-5",
        "OK",
        "result: success in 53.1 s, 862780 tokens",
    ]


def test_render_codex_fixture_golden():
    text = (FIXTURES / "codex-json.jsonl").read_text()
    assert _rendered("codex", text) == [
        "session 0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
        "OK",
        "turn: 32659 tokens",
    ]


def test_render_claude_tool_use_and_result():
    use = {"type": "assistant", "message": {"content": [
        {"type": "thinking", "thinking": "hidden"},
        {"type": "tool_use", "name": "Bash", "input": {"command": "x" * 300}}]}}
    result = {"type": "user", "message": {"content": [
        {"type": "tool_result", "content": "one\ntwo\nthree\nfour"}]}}
    listed = {"type": "user", "message": {"content": [
        {"type": "tool_result", "content": [{"type": "text", "text": "a\nb"}]}]}}
    line = render.render_line("claude", json.dumps(use))
    assert line.startswith("> Bash ") and "hidden" not in line
    assert len(line) == len("> Bash ") + 200
    assert render.render_line("claude", json.dumps(result)) == "one\ntwo\nthree"
    assert render.render_line("claude", json.dumps(listed)) == "a\nb"


def test_render_codex_command_and_reasoning():
    command = {"type": "item.completed", "item": {"type": "command_execution", "command": "ls -la", "exit_code": 2}}
    reasoning = {"type": "item.completed", "item": {"type": "reasoning", "text": "secret thoughts"}}
    assert render.render_line("codex", json.dumps(command)) == "> ls -la (exit 2)"
    assert render.render_line("codex", json.dumps(reasoning)) is None


def test_unknown_json_hidden():
    assert render.render_line("claude", '{"type":"rate_limit_event","x":1}') is None
    assert render.render_line("codex", '{"type":"turn.started"}') is None
    assert render.render_line("codex", '{"type":"item.completed","item":{"type":"error","message":"boom"}}') is None
    assert render.render_line("unknown-provider", '{"type":"x"}') == '{"type":"x"}'


def test_render_plain_line_passes_through_and_is_stripped():
    assert render.render_line("claude", "plain text") == "plain text"
    assert render.render_line("codex", "a\x1b]52;c;ZXZpbA==\x07b") == "ab"


def test_render_event_text_is_stripped():
    hostile = {"type": "assistant", "message": {"content": [{"type": "text", "text": "hi\x1b[2J\x1b]2;pwn\x07there\x9b31m"}]}}
    assert render.render_line("claude", json.dumps(hostile)) == "hithere"


def test_render_line_deeply_nested_json_is_shown_as_plain_text():
    line = '{"a":' + "[" * 200000
    assert render.render_line("claude", line) == line


def test_render_claude_result_with_huge_numbers_does_not_crash():
    line = '{"type":"result","subtype":"success","duration_ms":1' + "0" * 400 + ',"usage":{"input_tokens":1' + "0" * 400 + "}}"
    assert render.render_line("claude", line).startswith("result: success in ")


# -- follow -----------------------------------------------------------------


@pytest.fixture
def run_task(repo):
    task_store = store.TaskStore(repo, "demo")
    task_store.create({"task": "demo", "status": "running"})
    task_store.runs_dir.mkdir(parents=True, exist_ok=True)
    return task_store


def _record(task_store, state, session_id="", exit_code=None, provider="claude"):
    def change(current):
        current.setdefault("runs", {})["build-1"] = {
            "name": "build-1", "provider": provider, "state": state, "session_id": session_id, "exit_code": exit_code}

    task_store.update_state(change)


def test_follow_ends_on_orphaned_and_drains_partial_line(run_task):
    run_records.log_path(run_task, "build-1").write_text('{"type":"assistant","message":{"content":[{"type":"text","text":"one"}]}}\nlast words')
    _record(run_task, "orphaned")
    out = io.StringIO()
    render.follow(run_task, "build-1", out, poll_seconds=0)
    assert out.getvalue() == "one\nlast words\nrun build-1 orphaned, exit unknown\n"


def test_follow_prints_reopen_hint(run_task):
    run_records.log_path(run_task, "build-1").write_text("hello\n")
    _record(run_task, "finished", session_id=CLAUDE_ID, exit_code=0)
    out = io.StringIO()
    render.follow(run_task, "build-1", out, poll_seconds=0)
    assert out.getvalue() == "hello\nrun build-1 finished, exit 0\nreopen: sakura flow open demo build-1\n"


def test_follow_renders_as_the_log_grows_then_ends(run_task, monkeypatch):
    log = run_records.log_path(run_task, "build-1")
    log.write_text("first\n")
    _record(run_task, "running")
    out = io.StringIO()

    def append():
        assert out.getvalue() == "first\n"
        with log.open("a") as stream:
            stream.write("second\n")

    def finish():
        assert out.getvalue() == "first\nsecond\n"
        _record(run_task, "aborted", exit_code=1)

    actions = iter((append, finish))
    monkeypatch.setattr(render, "time", SimpleNamespace(sleep=lambda _: next(actions)()))
    render.follow(run_task, "build-1", out)
    assert out.getvalue() == "first\nsecond\nrun build-1 aborted, exit 1\n"


def test_follow_unknown_run_is_refused(run_task):
    with pytest.raises(run_records.RunRecordError, match="not recorded"):
        render.follow(run_task, "../x", io.StringIO(), poll_seconds=0)


def test_follow_waits_for_a_log_that_does_not_exist_yet(run_task):
    _record(run_task, "finished", exit_code=0)
    out = io.StringIO()
    render.follow(run_task, "build-1", out, poll_seconds=0)
    assert out.getvalue() == "run build-1 finished, exit 0\n"


# -- ledger follower and the commands -----------------------------------------


def test_follow_ledger_strips_controls_and_stops(run_task):
    run_task.append_ledger("engine", "danger \x1b]52;c;ZXZpbA==\x07ok")
    out = io.StringIO()
    render.follow_ledger(run_task, out, poll_seconds=0, should_stop=lambda: True)
    assert out.getvalue() == "# Ledger: demo\n- [engine] danger ok\n"


def test_flow_watch_command_renders_a_finished_run(repo, run_task, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    run_records.log_path(run_task, "build-1").write_text("hello\n")
    _record(run_task, "finished", session_id=CLAUDE_ID, exit_code=0)
    assert cli.main(["watch", "demo", "build-1"]) == exits.OK
    assert capsys.readouterr().out.startswith("hello\nrun build-1 finished, exit 0\n")


def test_flow_watch_command_unknown_run_is_usage(repo, run_task, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    assert cli.main(["watch", "demo", "build-9"]) == exits.USAGE
    assert cli.main(["watch", "nope", "build-1"]) == exits.USAGE


def test_flow_events_command_prints_ledger_until_interrupted(repo, run_task, monkeypatch, capsys):
    monkeypatch.chdir(repo)
    run_task.append_ledger("engine", "started \x1b[2J")

    def interrupt(_seconds):
        raise KeyboardInterrupt

    monkeypatch.setattr(render.time, "sleep", interrupt)
    assert cli.main(["events", "demo"]) == exits.OK
    assert "- [engine] started \n" in capsys.readouterr().out
