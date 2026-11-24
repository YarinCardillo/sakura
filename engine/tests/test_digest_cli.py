"""The digest commands: when a run happens, the lock, the done-today mark and the status."""

import fcntl
import io
import json
import os
import time
from datetime import datetime, timezone

import pytest

from sakura import engine_home
from sakura.digest import cli, config, intake, publish, run, schema, state, summarize
from sakura.outside import store

UTC = timezone.utc
MORNING = datetime(2026, 11, 17, 8, 0, tzinfo=UTC)
SETTINGS = config.DigestConfig("07:30", "sonnet", "en", 24, (config.Source("work", run=("/opt/work.sh",)),))
ITEM = schema.Item("pr-1", "Review", "https://example.org/pr/1", "2026-11-17T05:00:00+00:00", "Looks good")
NEWS = schema.ModelOutput((schema.News("work", "Review", "A review arrived.", ()),))


@pytest.fixture(autouse=True)
def utc_clock(monkeypatch):
    """The commands read the system's time zone; the tests fix it so a date means the same everywhere."""
    monkeypatch.setenv("TZ", "UTC")
    time.tzset()
    yield
    monkeypatch.undo()
    time.tzset()


class Steps:
    """Counts calls and answers like the real steps; publishing is the real one."""

    def __init__(self, fail_summary: bool = False, fail_sources: bool = False):
        self.calls, self.fail_summary, self.fail_sources = [], fail_summary, fail_sources

    def collect(self, settings, memory, now):
        self.calls.append("collect")
        if self.fail_sources:
            return [intake.SourceResult(schema.SourceReport("work", False, 0, 0, "timed out after 120 seconds"))]
        return [intake.SourceResult(schema.SourceReport("work", True, 1, 0), (ITEM,), "cursor-1")]

    def summarize(self, results, settings, window):
        self.calls.append("summarize")
        if self.fail_summary:
            raise summarize.SummarizeError("out.json breaks the schema")
        return NEWS

    def as_steps(self) -> run.Steps:
        return run.Steps(self.collect, self.summarize, publish.publish, publish.write_files)


def _run(steps: Steps, now=MORNING, scheduled=True):
    return run.run(SETTINGS, now, scheduled=scheduled, steps=steps.as_steps(), tz=UTC, pusher=_Pusher())


class _Pusher:
    def push(self, event, title, message):
        return "sent"


def test_Run_Scheduled_DoneToday_Skips():
    state.save(state.State(done_date="2026-11-17"))
    steps = Steps()
    outcome = _run(steps)
    assert not outcome.ran and "already done today" in outcome.reason and steps.calls == []


def test_Run_Scheduled_BeforeAt_Skips():
    steps = Steps()
    outcome = _run(steps, now=datetime(2026, 11, 17, 7, 29, tzinfo=UTC))
    assert not outcome.ran and outcome.reason == "before 07:30" and steps.calls == []
    assert _run(steps, now=datetime(2026, 11, 17, 7, 30, tzinfo=UTC)).ran


def test_Run_Scheduled_AfterAt_RunsOnceAndMarksTheDay():
    steps = Steps()
    first = _run(steps)
    assert first.ran and first.published.lane == "digest-2026-11-17" and steps.calls == ["collect", "summarize"]
    memory = state.load()
    assert memory.done_date == "2026-11-17" and memory.of("work").cursor == "cursor-1"
    assert (config.digest_folder() / "latest.json").exists()
    again = _run(steps)
    assert not again.ran and steps.calls == ["collect", "summarize"]


def test_Run_OnDemand_RunsEvenIfDone():
    state.save(state.State(done_date="2026-11-17"))
    steps = Steps()
    assert _run(steps, now=datetime(2026, 11, 17, 5, 0, tzinfo=UTC), scheduled=False).ran
    assert steps.calls == ["collect", "summarize"]


def test_Run_TwoAtOnce_SecondSkips():
    folder = state.ensure_folder()
    descriptor = os.open(folder / config.LOCK_FILE, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(descriptor, fcntl.LOCK_EX)
    try:
        steps = Steps()
        outcome = _run(steps, scheduled=False)
        assert not outcome.ran and "in progress" in outcome.reason and steps.calls == []
    finally:
        os.close(descriptor)
    assert _run(steps, scheduled=False).ran


def test_Run_SummarizerFails_NothingPublishedAndStateUnchanged():
    before = state.State({"work": state.SourceState("old", "2026-11-16T06:00:00+00:00")}, "2026-11-16")
    state.save(before)
    with pytest.raises(run.RunRefused, match="nothing was published"):
        _run(Steps(fail_summary=True))
    assert state.load() == before and not (config.digest_folder() / "latest.json").exists()
    assert store.lanes(store.path()) == []


def test_Run_NoSources_Refused():
    empty = config.DigestConfig("07:30", "sonnet", "en", 24, ())
    with pytest.raises(run.RunRefused, match="no sources"):
        run.run(empty, MORNING, scheduled=False, steps=Steps().as_steps(), tz=UTC)


def test_Status_LastRunAndNext():
    first = run.status(SETTINGS, datetime(2026, 11, 17, 7, 0, tzinfo=UTC), tz=UTC)
    assert (first.last_run_at, first.is_pending_today) == ("", False)
    assert first.next_at == datetime(2026, 11, 17, 7, 30, tzinfo=UTC)
    overdue = run.status(SETTINGS, MORNING, tz=UTC)
    assert overdue.is_pending_today and overdue.next_at == datetime(2026, 11, 17, 7, 30, tzinfo=UTC)
    _run(Steps())
    done = run.status(SETTINGS, MORNING, tz=UTC)
    assert done.last_date == "2026-11-17" and done.last_run_at == "2026-11-17T08:00:00+00:00"
    assert done.next_at == datetime(2026, 11, 18, 7, 30, tzinfo=UTC) and not done.is_pending_today
    assert done.sources == (("work", "2026-11-17T08:00:00+00:00"),)


# The command line

def _configured():
    path = engine_home.root() / config.CONFIG_FILE
    path.write_text('at = "07:30"\n[[source]]\nid = "work"\nrun = ["/opt/work.sh"]\n')
    path.chmod(0o600)


def _main(*argv, steps=None, env=None, now=MORNING):
    out = io.StringIO()
    code = cli.main(list(argv), now=lambda: now, env=env if env is not None else {}, stdout=out,
                    steps=(steps or Steps()).as_steps())
    return code, out.getvalue()


def test_Cli_Run_WritesAndReports():
    _configured()
    code, text = _main("run")
    assert code == 0 and "digest written: lane digest-2026-11-17, 0 checks, 0 decisions" in text


def test_Cli_RunScheduled_SkipsWithExitZero():
    _configured()
    state.save(state.State(done_date="2026-11-17"))
    code, text = _main("run", "--scheduled")
    assert code == 0 and "skipped: already done today" in text


def test_Cli_Status_JsonAndText():
    _configured()
    _main("run")
    code, text = _main("status", "--json")
    document = json.loads(text)
    assert code == 0 and document["last_date"] == "2026-11-17" and document["sources"][0]["id"] == "work"
    assert "/opt/work.sh" not in text  # a source's program or URL is never shown
    assert "last good run" in _main("status")[1]


def test_Cli_InsideARole_Refused():
    _configured()
    for command in (("run",), ("status",)):
        code, text = _main(*command, env={"SAKURA_FLOW_ROLE": "builder"})
        assert code == 2 and text == ""


def test_Cli_NoConfigOrBadUsage_Refused(capsys):
    assert _main("run")[0] == 2 and "digest.toml" in capsys.readouterr().err
    assert _main("nonsense")[0] == 2


def test_Cli_SummarizerFailure_ExitsFailed(capsys):
    _configured()
    assert _main("run", steps=Steps(fail_summary=True))[0] == 4
    assert "nothing was published" in capsys.readouterr().err


def test_Brakes_EngineRoleRunsSakuraDigest_Denied(repo):
    from flow_support import make_context
    from test_flow_brakes import BUILDER, _bash, _denied

    make_context(repo)
    env = {**BUILDER, "SAKURA_FLOW_ROLE": "builder"}
    for verb in ("run", "status", "install", "uninstall"):
        reason = _denied(repo, _bash(f"sakura digest {verb}"), env)
        assert reason and "operator" in reason and f"digest {verb}" in reason


@pytest.mark.parametrize("verb", [("run",), ("run", "--scheduled"), ("run", "--detach"), ("status",), ("install",), ("uninstall",)])
def test_Cli_InsideTheCageWithoutTheRoleVariable_EveryVerbRefused(monkeypatch, verb, tmp_path):
    from sakura.digest import launchd

    _configured()
    monkeypatch.setattr(engine_home, "sandboxed", lambda: True)  # the cage cannot write the trust root
    spawned, fake = [], []
    agent = launchd.Agent(lambda arguments: fake.append(arguments) or 0, tmp_path / "agents", tmp_path)
    out = io.StringIO()
    code = cli.main(list(verb), env={}, now=lambda: MORNING, stdout=out, steps=Steps().as_steps(), agent=agent,
                    spawn=spawned.append)
    assert code == 2 and out.getvalue() == "" and spawned == [] and fake == []
    assert not (config.digest_folder() / "latest.json").exists() and not (tmp_path / "agents").exists()


def test_Cli_TaskVariableAlone_Refused():
    _configured()
    assert _main("status", env={"SAKURA_FLOW_TASK": "demo"})[0] == 2


# A day is done only when something was published

def test_Run_EveryScheduledSourceFails_DayStaysOpenAndALaterSuccessMarksIt():
    failing = Steps(fail_sources=True)
    with pytest.raises(run.RunRefused, match="every source failed"):
        _run(failing)
    assert failing.calls == ["collect"]  # no model run for nothing
    assert state.load().done_date == "" and store.lanes(store.path()) == []
    failed = json.loads((config.digest_folder() / "latest.json").read_text())
    assert failed["sources"] == [{"id": "work", "ok": False, "items": 0, "skipped": 0, "error": "timed out after 120 seconds"}]
    assert failed["news"] == [] and (config.digest_folder() / "2026-11-17.json").exists()
    steps = Steps()
    retry = _run(steps, now=datetime(2026, 11, 17, 9, 0, tzinfo=UTC))
    assert retry.ran and steps.calls == ["collect", "summarize"]
    assert state.load().done_date == "2026-11-17" and store.lanes(store.path())[0].id == "digest-2026-11-17"
    latest = json.loads((config.digest_folder() / "latest.json").read_text())
    assert latest["sources"][0]["ok"] is True and latest["news"]  # the success overwrote the failed attempt's file
    assert not _run(steps, now=datetime(2026, 11, 17, 10, 0, tzinfo=UTC)).ran


def test_Run_PublishFails_DayStaysOpen(monkeypatch):
    def broken(digest, language, now, *, pusher=None):
        raise publish.PublishError("the lane digest-2026-11-17 cannot be written: full")

    steps = Steps().as_steps()
    steps = run.Steps(steps.collect, steps.summarize, broken, steps.write_files)
    with pytest.raises(run.RunRefused, match="cannot be written"):
        run.run(SETTINGS, MORNING, scheduled=True, steps=steps, tz=UTC)
    # Nothing moved, but a source with nothing saved keeps the window it used, so the retry never shrinks it.
    assert state.load().done_date == "" and state.load().of("work") == state.SourceState("", "2026-11-16T08:00:00+00:00")


def test_Run_SomeSourcesFail_OthersPublishAndTheDayIsDone():
    two = config.DigestConfig("07:30", "sonnet", "en", 24, (*SETTINGS.sources, config.Source("feed", feed="https://example.org/f")))
    steps = Steps()
    good = steps.collect

    def mixed(settings, memory, now):
        return [*good(settings, memory, now), intake.SourceResult(schema.SourceReport("feed", False, 0, 0, "HTTP 500"))]

    parts = steps.as_steps()
    outcome = run.run(two, MORNING, scheduled=True, steps=run.Steps(mixed, parts.summarize, parts.publish, parts.write_files),
                      tz=UTC)
    assert outcome.ran and state.load().done_date == "2026-11-17"
    assert state.load().of("feed") == state.SourceState("", "2026-11-16T08:00:00+00:00")
    assert state.load().of("work").cursor == "cursor-1"


def test_Run_ItemsLostToTheInputBudget_OnlyThatSourceIsWithheldAndKeepsItsState(monkeypatch):
    monkeypatch.setattr(summarize, "MAX_INPUT_BYTES", 600)
    two = config.DigestConfig("07:30", "sonnet", "en", 24, (*SETTINGS.sources, config.Source("feed", feed="https://example.org/f")))
    big = tuple(schema.Item(f"i{number}", f"Item {number}", "", f"2026-11-16T0{number}:00:00+00:00", "x" * 150)
                for number in range(1, 6))
    steps = Steps()
    steps.collect = lambda settings, memory, now: [
        intake.SourceResult(schema.SourceReport("work", True, 1, 0), (ITEM,), "cursor-1"),
        intake.SourceResult(schema.SourceReport("feed", True, 5, 0), big, "cursor-feed")]
    parts = steps.as_steps()
    outcome = run.run(two, MORNING, scheduled=True, steps=run.Steps(steps.collect, parts.summarize, parts.publish,
                                                                     parts.write_files), tz=UTC)
    assert outcome.ran and state.load().of("work").cursor == "cursor-1"
    assert state.load().of("feed") == state.SourceState("", "2026-11-16T08:00:00+00:00")  # its window, not a cursor
    report = {item["id"]: item for item in json.loads((config.digest_folder() / "latest.json").read_text())["sources"]}
    assert report["feed"]["ok"] is False and report["feed"]["error"].startswith("too much new: narrow this source")


FEED_SETTINGS = config.DigestConfig("07:30", "sonnet", "en", 24, (config.Source("releases", feed="https://example.org/f"),))


def _entries(count: int, when: str) -> bytes:
    body = "".join(f"<entry><id>e{number}</id><title>Entry {number}</title><updated>{when}</updated></entry>"
                   for number in range(count))
    return f'<feed xmlns="http://www.w3.org/2005/Atom">{body}</feed>'.encode()


def _feed_run(count: int, now: datetime, body_when: str = "2026-11-16T10:00:00Z"):
    """A scheduled run over a fake feed of `count` entries, and the titles the model was shown."""
    shown = []
    steps = Steps()
    steps.collect = lambda settings, memory, when: intake.collect(settings, memory, when,
                                                                  fetch=lambda url, timeout: _entries(count, body_when))
    parts = steps.as_steps()

    def summarize_step(results, settings, window):
        shown.extend(item.id for result in results for item in result.items)
        return NEWS

    return run.run(FEED_SETTINGS, now, scheduled=True, steps=run.Steps(steps.collect, summarize_step, parts.publish,
                                                                         parts.write_files), tz=UTC), shown


def test_Run_FirstRunOverflow_NextDayRetriesTheSameWindow_NoneLostNoneRepeated():
    day_one = datetime(2026, 11, 17, 8, 0, tzinfo=UTC)
    with pytest.raises(run.RunRefused, match="every source failed"):
        _feed_run(201, day_one)  # nothing from the source is published
    report = json.loads((config.digest_folder() / "latest.json").read_text())["sources"][0]
    assert report["error"].startswith("too much new: narrow this source")
    assert state.load().done_date == "" and state.load().of("releases") == state.SourceState("", "2026-11-16T08:00:00+00:00")
    # The next day the source returns 150: the window is still the first one, so entries of 16 November are all there.
    outcome, shown = _feed_run(150, datetime(2026, 11, 18, 8, 0, tzinfo=UTC))
    assert outcome.ran and sorted(shown) == sorted(f"e{number}" for number in range(150))
    after = state.load().of("releases")
    assert after.cursor == "2026-11-16T10:00:00+00:00" and after.at == "2026-11-18T08:00:00+00:00"
    outcome, shown = _feed_run(150, datetime(2026, 11, 19, 8, 0, tzinfo=UTC))
    assert shown == []  # none repeated
