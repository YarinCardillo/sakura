import json
import os

from sakura.quota import codex_logs

SECRET = "someone@example.invalid"


def line(ts, week, resets=1793689200, five=None):
    limits = {"plan_type": "prolite", "primary": None,
              "secondary": {"used_percent": week, "window_minutes": 10080, "resets_at": resets}}
    if five is not None:
        limits["primary"] = {"used_percent": five, "window_minutes": 300, "resets_at": resets - 100000}
    payload = {"type": "token_count", "rate_limits": limits, "account": SECRET}
    return json.dumps({"timestamp": ts, "type": "event_msg", "payload": payload}) + "\n"


def rollout(home, name, *lines, mtime=None):
    path = home / "sessions/2026/11/02" / f"rollout-{name}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('{"type":"session_meta"}\n' + "".join(lines))
    if mtime:
        os.utime(path, (mtime, mtime))
    return path


def test_LatestLimits_ReadsTheLastLimitsLineOfTheNewestRollout(tmp_path):
    rollout(tmp_path, "a", line("2026-11-02T17:00:00Z", 8.0), line("2026-11-02T17:55:00Z", 9.0, five=2.0))
    found = codex_logs.latest_limits(tmp_path)
    assert (found.seven_day_pct, found.five_hour_pct, found.subscription_type) == (9.0, 2.0, "prolite")
    assert found.seven_day_resets_at == "2026-11-03T07:00:00Z"


def test_LatestLimits_SeveralRollouts_TakesTheMostRecentLine(tmp_path):
    rollout(tmp_path, "old", line("2026-11-01T10:00:00Z", 5.0), mtime=1)
    rollout(tmp_path, "new", line("2026-11-02T10:00:00Z", 9.0), mtime=2)
    assert codex_logs.latest_limits(tmp_path).seven_day_pct == 9.0


def test_LatestLimits_NoSessions_ReturnsNone(tmp_path):
    assert codex_logs.latest_limits(tmp_path) is None


def test_LatestLimits_RolloutWithoutLimits_ReturnsNone(tmp_path):
    rollout(tmp_path, "a", '{"type":"event_msg","payload":{"type":"agent_message"}}\n')
    assert codex_logs.latest_limits(tmp_path) is None


def test_LatestLimits_NeverKeepsTheRestOfTheLine(tmp_path):
    rollout(tmp_path, "a", line("2026-11-02T17:55:00Z", 9.0))
    assert SECRET not in repr(codex_logs.latest_limits(tmp_path))
