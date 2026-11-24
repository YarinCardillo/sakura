import json
import shutil
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import cli
from sakura.flow import store
from sakura.lanes.status import LaneRow
from sakura.usage import aggregate, command, projection, report
from sakura.usage.model import Sample, Usage

FIXTURES = Path(__file__).parent / "fixtures" / "usage"
CODEX_SESSION = "00000000-0000-4000-8000-0000000000c1"
LANE_SESSION = "11111111-2222-4333-8444-555555555555"
RUN_ENDED_AT = "2026-10-23T12:30:00+00:00"
HOUR = timedelta(hours=1)
RESET = datetime(2026, 10, 28, 7, 0, tzinfo=timezone.utc)
PERIOD_START = RESET - timedelta(days=7)
CODEX_FIXTURE_TOTAL = 143217 + 582
CLAUDE_FIXTURE_TOTAL = 38 + 90098 + 1572407 + 12344


def utc(*parts):
    return datetime(*parts, tzinfo=timezone.utc)


NOW = utc(2026, 10, 23, 13, 0)


# -- projection ---------------------------------------------------------------------------------


def series(start, values, resets_at, provider="claude", window="seven_day", step=10):
    return [Sample(start + timedelta(hours=step * index), provider, window, value, resets_at)
            for index, value in enumerate(values)]


def test_projection_linear():
    start = utc(2026, 10, 20, 0, 0)
    samples = series(start, [0.40, 0.50], start + 50 * HOUR)

    result = projection.project(samples, start + 10 * HOUR)

    assert result.current == 0.50
    assert result.at_reset == pytest.approx(0.90)
    assert result.limit_at is None
    assert result.trend == "rising"


def test_projection_limit_time():
    start = utc(2026, 10, 20, 0, 0)
    now = start + 10 * HOUR
    samples = series(start, [0.60, 0.70], start + 80 * HOUR)

    result = projection.project(samples, now)

    assert result.at_reset == pytest.approx(1.40)
    assert result.limit_at == now + 30 * HOUR


def test_projection_ignores_other_window_and_previous_period():
    start = utc(2026, 10, 20, 0, 0)
    reset = start + 50 * HOUR
    previous = series(start - 3 * HOUR, [0.95, 0.99], start - HOUR)
    other_window = series(start, [0.10, 0.90], start + 5 * HOUR, window="five_hour")
    other_provider = series(start, [0.0, 0.9], reset, provider="codex")
    own = series(start, [0.40, 0.50], reset)

    result = projection.project(previous + other_window + other_provider + own, start + 10 * HOUR,
                                provider="claude")

    assert (result.current, round(result.at_reset, 6)) == (0.50, 0.90)


def test_projection_flat_on_negative_slope():
    start = utc(2026, 10, 20, 0, 0)
    samples = series(start, [0.50, 0.40], start + 50 * HOUR)

    result = projection.project(samples, start + 10 * HOUR)

    assert (result.trend, result.current, result.at_reset, result.limit_at) == ("flat", 0.40, 0.40, None)


def test_projection_none_when_stale_or_single():
    start = utc(2026, 10, 20, 0, 0)
    reset = start + 50 * HOUR
    pair = series(start, [0.40, 0.50], reset)

    assert projection.project(pair[:1], start + HOUR) is None
    assert projection.project(pair, pair[-1].at + 7 * HOUR) is None
    assert projection.project(pair, pair[-1].at + 5 * HOUR) is not None
    assert projection.project(pair, reset + HOUR) is None


def test_projection_uses_only_the_last_day():
    start = utc(2026, 10, 20, 0, 0)
    old = [Sample(start, "claude", "seven_day", 0.0, start + 100 * HOUR)]
    recent = series(start + 40 * HOUR, [0.40, 0.50], start + 100 * HOUR)

    result = projection.project(old + recent, start + 50 * HOUR)

    assert result.at_reset == pytest.approx(0.50 + 0.01 * 50)


# -- aggregation --------------------------------------------------------------------------------


def make_task(repo, task_id, runs):
    task = store.TaskStore(repo, task_id)
    task.create({"task": task_id, "status": "running", "runs": runs})
    task.runs_dir.mkdir(parents=True, exist_ok=True)
    return task


def run_record(name, provider, session_id="", ended_at=RUN_ENDED_AT):
    return {"name": name, "provider": provider, "state": "finished", "session_id": session_id, "ended_at": ended_at}


def put_codex_rollout(sessions_dir, session_id=CODEX_SESSION):
    folder = sessions_dir / "2026" / "10" / "18"
    folder.mkdir(parents=True, exist_ok=True)
    target = folder / f"rollout-2026-10-18T16-59-00-{session_id}.jsonl"
    shutil.copy(FIXTURES / "codex_rollout.jsonl", target)
    return target


def message_line(message_id, at, output=10):
    usage = {"input_tokens": 1, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": output}
    return json.dumps({"type": "assistant", "timestamp": at.isoformat(), "message": {"id": message_id, "usage": usage}})


def put_transcript(tmp_path, *lines):
    path = tmp_path / "claude" / f"{LANE_SESSION}.jsonl"
    path.parent.mkdir(exist_ok=True)
    path.write_text("\n".join(lines) + "\n")
    return path


def lane_row(name="alpha", tool="claude", session_id=LANE_SESSION, transcript="", group="main"):
    return LaneRow(group=group, name=name, worktree=f"/work/{name}", branch="", last_commit="", stack="up", tool=tool,
                   state="waiting", model="", session_id=session_id, pr="", transcript=transcript)


@pytest.fixture
def world(repo, tmp_path):
    """One flow task with a Claude run (limits and a result) and a Codex run, plus a Codex sessions directory."""
    sessions = tmp_path / "codex-home" / "sessions"
    put_codex_rollout(sessions)
    task = make_task(repo, "demo", {"plan-1": run_record("plan-1", "claude", "a1"),
                                    "build-1": run_record("build-1", "codex", CODEX_SESSION)})
    shutil.copy(FIXTURES / "claude_run.log", task.runs_dir / "plan-1.log")
    return type("World", (), {"repo": repo, "tmp": tmp_path, "sessions": sessions, "task": task})


def provider(week, name):
    return next(entry for entry in week.providers if entry.provider == name)


def test_week_counts_each_session_once(world):
    lanes = [lane_row(tool="codex", session_id=CODEX_SESSION), lane_row(tool="codex", session_id=CODEX_SESSION)]

    week = aggregate.week([world.repo], lanes, world.sessions, NOW)

    codex = provider(week, "codex")
    assert codex.totals.total == CODEX_FIXTURE_TOTAL
    assert [(agent.item, agent.agent) for agent in codex.agents] == [(("flow", str(world.repo), "demo"), "build-1")]
    assert provider(week, "claude").totals.total == CLAUDE_FIXTURE_TOTAL


def test_week_reads_utilization_and_period(world):
    week = aggregate.week([world.repo], [], world.sessions, NOW)

    claude, codex = provider(week, "claude"), provider(week, "codex")
    assert (claude.utilization, claude.resets_at, claude.period_start) == (0.53, RESET, PERIOD_START)
    assert claude.utilization_at == utc(2026, 10, 23, 12, 30)
    assert codex.utilization == 0.04
    assert codex.resets_at == utc(2026, 10, 24, 20, 43, 58)


def test_session_spanning_reset_split(world):
    before, after = PERIOD_START - HOUR, PERIOD_START + HOUR
    path = put_transcript(world.tmp, message_line("m1", before, output=100), message_line("m2", after, output=7))

    week = aggregate.week([world.repo], [lane_row(transcript=str(path))], world.sessions, NOW)

    lane = next(agent for agent in provider(week, "claude").agents if agent.item[0] == "lane")
    assert lane.usage == Usage(1, 0, 0, 7, None)


def test_period_is_rolled_forward_when_the_last_reset_has_passed(world):
    week = aggregate.week([world.repo], [], world.sessions, RESET + timedelta(days=2))

    for name in ("claude", "codex"):
        entry = provider(week, name)
        assert entry.utilization is None and entry.projection is None
        assert entry.resets_at > RESET - timedelta(days=7)
        assert entry.totals.total == 0
    assert provider(week, "claude").resets_at == RESET + timedelta(days=7)


def test_share_labelled_local(world):
    path = put_transcript(world.tmp, message_line("m1", PERIOD_START + HOUR, output=999))
    week = aggregate.week([world.repo], [lane_row(transcript=str(path))], world.sessions, NOW)

    claude = provider(week, "claude")
    assert sum(agent.share_of_local_tokens for agent in claude.agents) == pytest.approx(1.0)
    text = report.render(week)
    assert "share of locally observed tokens" in text
    assert "of weekly limit" not in text


def test_unreferenced_rollout_is_never_read(world):
    put_codex_rollout(world.sessions, "00000000-0000-4000-8000-0000000000d9")

    week = aggregate.week([world.repo], [], world.sessions, NOW)

    assert provider(week, "codex").totals.total == CODEX_FIXTURE_TOTAL


def test_live_run_and_unreadable_state_are_skipped(repo, tmp_path):
    make_task(repo, "live", {"build-1": run_record("build-1", "claude", ended_at=None)})
    broken = store.TaskStore(repo, "broken")
    broken.root.mkdir(parents=True)
    broken.state_file.write_text("{not json")

    week = aggregate.week([repo], [], tmp_path / "none", NOW)

    assert all(entry.utilization is None and entry.totals is None for entry in week.providers)


def test_report_strips_terminal_controls(world):
    hostile = "\x1b]2;pwned\x07\x1b[31mred"
    path = put_transcript(world.tmp, message_line("m1", PERIOD_START + HOUR))
    week = aggregate.week([world.repo], [lane_row(name=hostile, transcript=str(path), group=hostile)],
                          world.sessions, NOW)

    for text in (report.render(week), report.render_json(week)):
        assert "\x1b" not in text and "\x07" not in text


@pytest.mark.parametrize("item", [("flow", "/work/evil\nrepo", "task"), ("flow", "/work/repo", "ta\rsk\tx"),
                                  ("lane", "grp\n    forged line", "alpha")])
@pytest.mark.parametrize("agent_name", ["claude", "claude\n    forged line"])
def test_render_keeps_a_hostile_label_on_one_line(item, agent_name):
    usage = Usage(1, 0, 0, 1, None)
    agent = aggregate.AgentUsage(item, agent_name, usage, 1.0)
    entry = aggregate.ProviderWeek("claude", None, None, None, None, None, usage, (agent,))

    lines = report.render(aggregate.WeekUsage(NOW, (entry,))).splitlines()

    assert sum(1 for line in lines if line.startswith("    ")) == 1
    assert len(lines) == 6


def test_render_shows_not_available_for_unreadable_figures():
    empty = aggregate.week([], [], Path("/nonexistent-sessions"), NOW)

    text = report.render(empty)

    assert text.count("not available") >= 4
    assert "not enough data" not in text


def test_render_projection_without_trend_says_not_enough_data(world):
    text = report.render(aggregate.week([world.repo], [], world.sessions, NOW))

    assert "not enough data" in text


# -- command ------------------------------------------------------------------------------------


def test_usage_command_not_available(tmp_path, monkeypatch, capsys):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("CODEX_HOME", raising=False)

    assert cli.main(["usage"]) == 0
    out = capsys.readouterr().out

    assert out.count("not available") >= 4
    assert "claude" in out and "codex" in out
    assert cli.main(["usage", "--json"]) == 0
    data = json.loads(capsys.readouterr().out)
    assert [entry["provider"] for entry in data["providers"]] == ["claude", "codex"]
    assert all(entry["utilization"] is None and entry["totals"] is None for entry in data["providers"])


def test_usage_command_reads_flow_logs_and_codex_rollouts(world, monkeypatch, capsys):
    monkeypatch.chdir(world.repo)
    monkeypatch.setenv("CODEX_HOME", str(world.sessions.parent))
    monkeypatch.setattr(command, "_utc_now", lambda: NOW)

    assert cli.main(["usage", "--json"]) == 0
    claude, codex = json.loads(capsys.readouterr().out)["providers"]

    assert claude["utilization"] == 0.53
    assert codex["totals"]["total"] == CODEX_FIXTURE_TOTAL
