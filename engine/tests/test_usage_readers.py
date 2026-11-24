from datetime import datetime, timezone
from pathlib import Path

from sakura.usage import claude, codex
from sakura.usage.model import Sample, Usage

FIXTURES = Path(__file__).parent / "fixtures" / "usage"
RUN_ENDED_AT = datetime(2026, 10, 23, 12, 30, tzinfo=timezone.utc)
CODEX_SESSION = "00000000-0000-4000-8000-0000000000c1"


def utc(*parts):
    return datetime(*parts, tzinfo=timezone.utc)


def test_claude_run_usage_keeps_cache_write():
    event = claude.run_usage(FIXTURES / "claude_run.log", RUN_ENDED_AT)

    assert event.at == RUN_ENDED_AT
    assert event.usage == Usage(uncached_input=38, cache_write=90098, cache_read=1572407,
                                output=12344, cost_usd=0.05903319)
    assert event.usage.total == 38 + 90098 + 1572407 + 12344


def test_claude_samples_both_windows():
    at = utc(2026, 10, 23, 12, 21)

    samples = claude.run_samples(FIXTURES / "claude_run.log", at)

    by_window = {sample.window: sample for sample in samples}
    assert set(by_window) == {"five_hour", "seven_day"}
    assert by_window["seven_day"] == Sample(at, "claude", "seven_day", 0.53, utc(2026, 10, 28, 7, 0))
    assert by_window["five_hour"].utilization == 0.17
    assert by_window["five_hour"].resets_at == utc(2026, 10, 23, 12, 30)


def test_transcript_dedupes_message_ids():
    events = claude.transcript_events(FIXTURES / "claude_transcript.jsonl")

    assert [event.usage.output for event in events] == [882, 1041]
    assert events[0].at == utc(2026, 10, 20, 19, 35, 34, 667000)
    assert events[0].usage == Usage(2, 24004, 0, 882, None)


def test_codex_events_are_deltas_of_cumulative_totals():
    events = codex.session_events(FIXTURES / "codex_rollout.jsonl")

    assert [event.usage.output for event in events] == [111, 379, 92]
    assert [event.usage.cache_read for event in events] == [10112, 41088, 49408]
    assert events[1].at == utc(2026, 10, 18, 16, 59, 48, 412000)
    assert sum(event.usage.total for event in events) == 143217 + 582


def test_codex_uncached_is_input_minus_cached():
    first = codex.session_events(FIXTURES / "codex_rollout.jsonl")[0]

    assert first.usage == Usage(uncached_input=41295 - 10112, cache_write=0, cache_read=10112,
                                output=111, cost_usd=None)


def test_codex_samples_weekly_only_and_scaled():
    samples = codex.samples(FIXTURES / "codex_rollout.jsonl")

    assert [sample.utilization for sample in samples] == [0.03, 0.035, 0.035, 0.04]
    assert {(sample.provider, sample.window) for sample in samples} == {("codex", "seven_day")}
    assert samples[0].at == utc(2026, 10, 18, 16, 59, 24, 298000)
    assert samples[0].resets_at == utc(2026, 10, 24, 20, 43, 58)


def test_codex_rollout_for_finds_session(tmp_path):
    day = tmp_path / "2026" / "10" / "18"
    day.mkdir(parents=True)
    wanted = day / f"rollout-2026-10-18T16-59-20-{CODEX_SESSION}.jsonl"
    wanted.write_text("{}\n")
    (day / "rollout-2026-10-18T16-59-20-other.jsonl").write_text("{}\n")

    assert codex.rollout_for(tmp_path, CODEX_SESSION) == wanted
    assert codex.rollout_for(tmp_path, "00000000-0000-4000-8000-0000000000ff") is None
    assert codex.rollout_for(tmp_path, "*") is None
    assert codex.rollout_for(tmp_path / "missing", CODEX_SESSION) is None


def test_unknown_shapes_tolerated(tmp_path):
    junk = tmp_path / "junk.log"
    junk.write_text('{"type":"result","usage":"x"}\n[1,2]\n"s"\n\n{"type":"rate_limit_event"}\n'
                    '{"payload":{"type":"token_count","info":{"total_token_usage":"x"}}}\n'
                    + "[" * 100000 + "\n")
    missing = tmp_path / "missing.log"
    when = utc(2026, 10, 1)

    assert claude.run_usage(junk, when) is None
    assert claude.run_usage(missing, when) is None
    assert claude.run_samples(junk, when) == []
    assert claude.run_samples(missing, when) == []
    assert claude.transcript_events(junk) == []
    assert claude.transcript_events(missing) == []
    assert codex.session_events(junk) == []
    assert codex.session_events(missing) == []
    assert codex.samples(junk) == []
    assert codex.samples(missing) == []
