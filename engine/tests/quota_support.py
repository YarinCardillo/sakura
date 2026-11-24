"""Shared fakes for the quota service tests: a runner that answers `claude` and `codexbar` from canned output."""

from __future__ import annotations

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

NOW = datetime(2026, 11, 2, 21, 0, tzinfo=timezone.utc)
WEEK_RESET = 1793689200  # 2026-11-03T07:00:00Z
FIVE_RESET = 1793667600  # 2026-11-03T01:00:00Z


def clock():
    return NOW


def event(five=0.05, seven=0.5, week_reset=WEEK_RESET):
    info = {"status": "allowed", "isUsingOverage": False, "unifiedWindows": {
        "five_hour": {"utilization": five, "resetsAt": FIVE_RESET, "status": "allowed"},
        "seven_day": {"utilization": seven, "resetsAt": week_reset, "status": "allowed"}}}
    return json.dumps({"type": "rate_limit_event", "rate_limit_info": info}) + "\n"


def codexbar_output(week=9, reset="2026-11-04T09:56:53Z"):
    usage = {"loginMethod": "prolite", "primary": None,
             "secondary": {"windowMinutes": 10080, "usedPercent": week, "resetsAt": reset}}
    return json.dumps([{"provider": "codex", "usage": usage}])


class FakeRunner:
    """`claude` answers by CLAUDE_CONFIG_DIR (a missing variable is the default dir, key ""); `codexbar` is canned.

    `probes` maps a config dir (str) or "" to the stdout of the probe, or to None for a launch failure.
    """

    def __init__(self, probes: dict | None = None, codexbar: str | None = None):
        self.probes, self.codexbar, self.calls = probes or {}, codexbar, []

    def __call__(self, argv, env, cwd):
        self.calls.append((argv, dict(env)))
        if Path(argv[0]).name == "codexbar":
            return _done(argv, 0 if self.codexbar else 127, self.codexbar or "")
        if argv[1:3] == ["auth", "status"]:
            return _done(argv, 0, json.dumps({"subscriptionType": "max"}))
        out = self.probes.get(env.get("CLAUDE_CONFIG_DIR", ""))
        return _done(argv, 0 if out is not None else 127, out or "")

    def probed(self):
        return [env.get("CLAUDE_CONFIG_DIR", "") for argv, env in self.calls
                if argv[0] == "claude" and argv[1] == "-p"]


def _done(argv, code, out):
    return subprocess.CompletedProcess(argv, code, out, "")
