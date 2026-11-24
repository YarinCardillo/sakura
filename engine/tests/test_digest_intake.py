"""The intake: how a person's program is run and how what it prints is bounded."""

import json
import signal
import sys
import tempfile
import textwrap
from datetime import datetime, timezone
from pathlib import Path

import pytest
from sakura import programs
from sakura.digest import config, installation, intake, schema, state, supervise

NOW = datetime(2026, 11, 17, 6, 30, tzinfo=timezone.utc)
SETTINGS = config.DigestConfig("07:30", "sonnet", "en", 24, ())



@pytest.fixture(autouse=True)
def installed_test_interpreter(monkeypatch):
    check = programs.untrusted
    interpreter = Path(sys.executable).resolve()

    def trust_test_python(path, *args, **kwargs):
        return "" if Path(path) == interpreter else check(path, *args, **kwargs)

    monkeypatch.setattr(programs, "untrusted", trust_test_python)


def _program(tmp_path: Path, body: str, name: str = "collect.py") -> tuple[str, ...]:
    script = tmp_path / name
    script.write_text(textwrap.dedent(body))
    return (sys.executable, str(script))


def _source(tmp_path: Path, body: str, source_id: str = "work", timeout: int = 20) -> config.Source:
    return config.Source(source_id, run=_program(tmp_path, body, f"{source_id}.py"), timeout_seconds=timeout)


def _item(number: int, **fields) -> str:
    return json.dumps({"id": f"i{number}", "title": f"Item {number}", **fields})


def test_Intake_ProgramGetsSinceAndCursor(tmp_path):
    source = _source(tmp_path, """
        import json, os
        print(json.dumps({"id": "env", "title": "env", "text": os.environ["SAKURA_DIGEST_SINCE"] + "|" +
                          os.environ["SAKURA_DIGEST_CURSOR"] + "|" + os.getcwd()}))
        print(json.dumps({"cursor": "after-7"}))
    """)
    result = intake.run_source(source, "2026-11-16T06:30:00+00:00", "after-3")
    since, cursor, folder = result.items[0].text.split("|")
    assert (since, cursor, result.cursor) == ("2026-11-16T06:30:00+00:00", "after-3", "after-7")
    assert "sakura-digest-source-" in folder and not Path(folder).exists()
    assert result.report == schema.SourceReport("work", True, 1, 0)


def test_Intake_FirstRunLooksBackTheConfiguredHours(tmp_path):
    source = _source(tmp_path, 'import os; print(\'{"id":"s","title":"t","text":"\' + os.environ["SAKURA_DIGEST_SINCE"] + \'"}\')')
    found = intake.collect(config.DigestConfig("07:30", "sonnet", "en", 24, (source,)), state.State(), NOW)
    assert found[0].items[0].text == "2026-11-16T06:30:00+00:00"


def test_Intake_ItemsBounded(tmp_path):
    source = _source(tmp_path, """
        for number in range(250):
            print('{"id": "i%d", "title": "t"}' % number)
    """)
    result = intake.run_source(source, "", "")
    assert result.items == () and result.report.skipped == 0  # all or nothing: see the overflow tests below
    assert not result.report.ok and result.cursor == ""
    big = _source(tmp_path, f"print('{{\"id\":\"b\",\"title\":\"t\",\"text\":\"' + 'x' * {schema.MAX_ITEM_TEXT + 1} + '\"}}')", "big")
    assert intake.run_source(big, "", "").report.skipped == 1


def test_Intake_OutputOverTheByteLimit_FailsTheSource(tmp_path, supervised_child):
    supervised_child.output[0] = _chunks(schema.MAX_SOURCE_BYTES + 1, b"x")
    supervised_child.deadline = 30
    source = _source(tmp_path, "", timeout=30)
    result = intake.run_source(source, "", "")
    assert not result.report.ok and "more than" in result.report.error and result.items == ()
    assert supervised_child.signals == [signal.SIGTERM]
    assert supervised_child.elapsed == 0


def _chunks(total, byte):
    return [byte * min(supervise.READ_CHUNK, total - offset)
            for offset in range(0, total, supervise.READ_CHUNK)]


def test_Intake_BadLineSkippedAndCounted(tmp_path):
    source = _source(tmp_path, """
        import json
        print("not json")
        print(json.dumps({"id": "good", "title": "Good"}))
        print(json.dumps({"id": "good", "title": "Same id again"}))
        print(json.dumps({"id": "hidden", "title": "a\\u202eb"}))
        print(json.dumps({"id": "extra", "title": "t", "token": "x"}))
        print(json.dumps([1, 2]))
        print("")
        print(json.dumps({"cursor": "c1"}))
        print(json.dumps({"cursor": ["list"]}))
    """)
    result = intake.run_source(source, "", "")
    assert [item.id for item in result.items] == ["good"] and result.cursor == "c1"
    assert result.report == schema.SourceReport("work", True, 1, 6)


def test_Intake_TimeoutReportedOthersStillRun(tmp_path):
    slow = _source(tmp_path, "", "slow", timeout=1)
    fast = _source(tmp_path, 'print(\'{"id":"ok","title":"fine"}\')', "fast")
    failing = _source(tmp_path, "import sys; sys.stderr.write('boom happened'); sys.exit(3)", "failing")
    calls = []
    inspected = {source.id: installation.inspect(source.run).argv for source in (slow, failing, fast)}  # what the engine runs

    def run(argv, env, timeout):
        calls.append((argv, timeout))
        if argv == inspected["slow"]:
            return supervise.Execution(None, b"", "", timed_out=True)
        return supervise.run_program(argv, env, timeout)

    settings = config.DigestConfig("07:30", "sonnet", "en", 24, (slow, failing, fast))
    first, second, third = intake.collect(settings, state.State(), NOW, run=run)
    assert calls == [(inspected["slow"], 1), (inspected["failing"], 20), (inspected["fast"], 20)]
    assert (first.report.ok, first.report.error) == (False, "timed out after 1 seconds")
    assert second.report.error == f"exited with status 3; {intake.DIAGNOSTICS_NOTE}" and "boom" not in second.report.error
    assert third.report.ok and third.items[0].id == "ok"


def test_Intake_TimeoutEndsTheWholeProcessGroup(tmp_path, supervised_child):
    supervised_child.deadline = 1
    source = _source(tmp_path, "", timeout=1)
    assert not intake.run_source(source, "", "").report.ok
    assert supervised_child.elapsed == pytest.approx(1)
    assert supervised_child.launches[0][1]["start_new_session"] is True
    assert supervised_child.signals == [signal.SIGTERM]
    assert not supervised_child.group_alive


def test_Intake_StdinClosed(tmp_path):
    source = _source(tmp_path, """
        import json, sys
        print(json.dumps({"id": "in", "title": repr(sys.stdin.read())}))
    """)
    assert intake.run_source(source, "", "").items[0].title == "''"


def test_Intake_MissingProgram_ReportedNotRaised(tmp_path):
    source = config.Source("work", run=(str(tmp_path / "missing.sh"),))
    result = intake.run_source(source, "", "")
    assert not result.report.ok and "protected installation" in result.report.error


def test_Intake_CursorMovesOnlyAfterSuccess(tmp_path):
    good = intake.SourceResult(schema.SourceReport("good", True, 1, 0), cursor="new-good")
    bad = intake.SourceResult(schema.SourceReport("bad", False, 0, 0, "exited with status 1"), cursor="new-bad")
    before = state.State({"good": state.SourceState("old-good", "2026-11-16T06:00:00+00:00"),
                          "bad": state.SourceState("old-bad", "2026-11-16T06:00:00+00:00")})
    after = state.committed(before, [good.report, bad.report],
                            {"good": good.cursor, "bad": bad.cursor}, NOW, "2026-11-17")
    assert after.of("good") == state.SourceState("new-good", "2026-11-17T06:30:00+00:00")
    assert after.of("bad") == before.of("bad")
    assert after.done_date == "2026-11-17"
    assert before.done_date == ""  # nothing moved until the run is committed


def test_Intake_SourceWithoutCursor_KeepsTheOldOne():
    report = schema.SourceReport("a", True, 0, 0)
    after = state.committed(state.State({"a": state.SourceState("kept", "")}), [report], {}, NOW, "2026-11-17")
    assert after.of("a").cursor == "kept"


def test_State_SaveAndLoad_RoundTrip():
    memory = state.State({"a": state.SourceState("c", "2026-11-17T06:30:00+00:00")}, "2026-11-17", "2026-11-17T06:30:00+00:00")
    assert state.load() == state.State()
    state.save(memory)
    assert state.load() == memory
    assert (config.digest_folder() / config.STATE_FILE).stat().st_mode & 0o777 == 0o600
    assert config.digest_folder().stat().st_mode & 0o777 == 0o700


def test_Intake_ItemWithAnUnconvertibleTime_SkippedOthersKept():
    output = (b'{"id": "edge", "title": "t", "at": "0001-01-01T00:00:00+01:00"}\n'
              b'{"id": "sneaky", "title": "a\\u034fb"}\n{"id": "fine", "title": "t"}\n')
    items, _cursor, skipped = intake.parse_output(output)
    assert [item.id for item in items] == ["fine"] and skipped == 2


def test_Intake_FailedProgramsStderr_NeverReachesTheReport(tmp_path):
    source = _source(tmp_path, """
        import sys
        sys.stderr.write("GET https://api.example.org/x?token=SECRET failed: 401\\n")
        sys.exit(3)
    """)
    result = intake.run_source(source, "", "")
    assert result.report == schema.SourceReport("work", False, 0, 0, f"exited with status 3; {intake.DIAGNOSTICS_NOTE}")
    assert "SECRET" not in json.dumps(schema.digest_to_json(schema.Digest(
        "2026-11-17", "2026-11-17T06:30:00+00:00", schema.Window("", ""), "sonnet", "digest-2026-11-17", (result.report,),
        schema.ModelOutput())))


def test_Intake_StderrDrained_BoundedTailKeptNothingOnDisk(tmp_path):
    before = sorted(Path(tempfile.gettempdir()).glob("sakura-digest-source-*"))
    source = _source(tmp_path, """
        import sys
        for number in range(3000):
            sys.stderr.write("noise %05d " % number + "x" * 190 + "\\n")
        sys.stderr.write("the last words\\n")
        print('{"id": "i", "title": "t"}')
    """)
    execution = intake.run_program(source.run, {}, 30)
    assert execution.exit_code == 0 and len(execution.stderr) <= intake.STDERR_TAIL_BYTES
    assert execution.stderr.endswith("the last words") and "noise 00000" not in execution.stderr
    assert intake.run_source(source, "", "").report.ok
    assert sorted(Path(tempfile.gettempdir()).glob("sakura-digest-source-*")) == before


def test_Intake_StderrOverTheByteCap_EndsTheSource(tmp_path, supervised_child):
    supervised_child.output[1] = _chunks(supervise.MAX_STDERR_BYTES + 1, b"y")
    supervised_child.deadline = 30
    source = _source(tmp_path, "", timeout=30)
    result = intake.run_source(source, "", "")
    assert not result.report.ok and "more than" in result.report.error and "yyy" not in result.report.error
    assert supervised_child.signals == [signal.SIGTERM]
    assert supervised_child.elapsed == 0


def test_Intake_MoreItemsThanAllowed_NothingDeliveredAndTheReportSaysSo(tmp_path):
    source = _source(tmp_path, """
        for number in range(250):
            print('{"id": "i%d", "title": "t"}' % number)
        print('{"cursor": "after-250"}')
    """)
    result = intake.run_source(source, "", "after-0")
    assert (result.report.ok, result.report.items, result.items, result.cursor) == (False, 0, (), "")
    assert result.report.error.startswith("too much new: narrow this source")
    before = state.State({"work": state.SourceState("after-0", "2026-11-16T06:00:00+00:00")})
    after = state.committed(before, [result.report], {"work": result.cursor}, NOW, "2026-11-17")
    assert after.of("work") == before.of("work")


def test_Intake_FeedWithMoreEntriesThanAllowed_SameRule():
    entries = "".join(f"<entry><id>e{number}</id><title>t</title><updated>2026-11-17T05:00:00Z</updated></entry>"
                      for number in range(201))
    body = f'<feed xmlns="http://www.w3.org/2005/Atom">{entries}</feed>'.encode()
    source = config.Source("releases", feed="https://example.org/f.atom")
    result = intake.run_source(source, "2026-11-16T06:30:00+00:00", "", fetch=lambda url, timeout: body)
    assert (result.report.ok, result.items, result.cursor) == (False, (), "")
    assert result.report.error.startswith("too much new: narrow this source")


def test_Intake_ExactlyTheLimit_Delivered():
    entries = "".join(f"<entry><id>e{number}</id><title>t</title><updated>2026-11-17T05:00:00Z</updated></entry>"
                      for number in range(schema.MAX_ITEMS_PER_SOURCE))
    body = f'<feed xmlns="http://www.w3.org/2005/Atom">{entries}</feed>'.encode()
    source = config.Source("releases", feed="https://example.org/f.atom")
    result = intake.run_source(source, "2026-11-16T06:30:00+00:00", "", fetch=lambda url, timeout: body)
    assert result.report.ok and len(result.items) == schema.MAX_ITEMS_PER_SOURCE


def test_State_WindowKeptForASourceWithNoSavedState_OnlyThen():
    sinces = {"new": "2026-11-16T06:30:00+00:00", "known": "2026-11-16T07:00:00+00:00"}
    memory = state.State({"known": state.SourceState("c", "2026-11-15T01:00:00+00:00")})
    kept = state.with_windows(memory, sinces)
    assert kept.of("new") == state.SourceState("", "2026-11-16T06:30:00+00:00")
    assert kept.of("known") == memory.of("known") and kept.done_date == ""
    failed = schema.SourceReport("new", False, 0, 0, "too much new: narrow this source")
    after = state.committed(memory, [failed], {}, NOW, "2026-11-17", sinces)
    assert after.of("new") == state.SourceState("", "2026-11-16T06:30:00+00:00") and after.done_date == "2026-11-17"


def test_digest_never_passes_an_installation_in_a_lane_root_to_its_runner(tmp_path, monkeypatch):
    from sakura import programs

    executable = tmp_path / "collect"
    executable.write_text("#!/bin/sh\n")
    executable.chmod(0o700)
    monkeypatch.setattr(programs, "forbidden_roots", lambda: [str(tmp_path)])
    calls = []

    def run(*arguments):
        calls.append(arguments)
        return intake.Execution(0, b"", "")

    result = intake.run_source(config.Source("test", run=(str(executable),)), "", "", run=run)

    assert not result.report.ok and calls == []


def test_digest_program_environment_contains_only_the_launch_allowlist(monkeypatch):
    monkeypatch.setenv("SAKURA_TEST_UNRELATED", "fixture")
    calls = []

    def run(argv, environment, timeout):
        calls.append(environment)
        return intake.Execution(0, b"", "")

    result = intake.run_source(config.Source("test", run=("/bin/sh",)), "window", "cursor", run=run)

    assert result.report.ok
    [environment] = calls
    assert "SAKURA_TEST_UNRELATED" not in environment
    assert environment["SAKURA_DIGEST_SINCE"] == "window"
    assert environment["SAKURA_DIGEST_CURSOR"] == "cursor"
