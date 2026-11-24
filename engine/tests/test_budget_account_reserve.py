"""The reserve of the account a flow runs on stops the flow: its own `reserve_week_pct`, else the global one."""

import json
from datetime import datetime, timedelta, timezone

import pytest

from sakura.budget import history
from sakura.flow import budget_gate, budget_guard, plan
from sakura.usage.archive import calibration
from sakura.usage.archive import store as archive_store

from flow_support import make_context

UTC = timezone.utc
NOW = datetime.now(UTC)
FAR = "2099-01-01T00:00:00+00:00"
WEEK_RESET = (NOW + timedelta(days=3)).replace(microsecond=0).isoformat()
TASKS = [{"id": "T1", "title": "t", "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False,
          "task_class": "integration"}]


def configure(tmp_path, accounts):
    """accounts.toml and an archive under the isolated HOME.

    `accounts` maps `(provider, id)` to `(own reserve or None, newest weekly percentage or None)`.
    """
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(f'db = "{home}/usage.db"\n')
    entries = ""
    for (provider, account_id), (reserve, _) in accounts.items():
        entries += (f'\n[[accounts]]\nprovider = "{provider}"\nid = "{account_id}"\n'
                    f'config_dir = "{home}/.{provider}-{account_id}"\n')
        entries += f"reserve_week_pct = {reserve}\n" if reserve is not None else ""
        entries += '\n[[accounts.plans]]\nplan = "max20x"\nsince = "2026-01-01"\n' if provider == "claude" else ""
    (home / ".config/sakura/accounts.toml").write_text(entries)
    conn = archive_store.open_db(home / "usage.db")
    for (provider, account_id), (_, week) in accounts.items():
        key = account_id if provider == "claude" else f"codex:{account_id}"
        conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                     " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES"
                     " (?, ?, 'max', 'allowed', 10.0, ?, ?, ?)",
                     (key, (NOW - timedelta(minutes=5)).isoformat(), FAR, week, WEEK_RESET))
    conn.commit()
    conn.close()


def decision(repo, account, provider="claude"):
    overrides = {"budget": {"account": account}, "roles": {"code_reviewer": "codex:gpt"}}
    ctx = make_context(repo, task=f"{provider}-{account}", overrides=overrides)
    return budget_guard.evaluate(ctx.store, ctx.config, NOW, provider=provider)


def test_guard_uses_the_account_reserve(repo, tmp_path):
    configure(tmp_path, {("claude", "a"): (40, 60.0)})
    stopped = decision(repo, "a")
    assert stopped.action == "pause" and stopped.rule == "week"
    assert "reserve 40%" in stopped.reason


def test_codex_role_guard_uses_the_codex_account_reserve(repo, tmp_path):
    configure(tmp_path, {("claude", "a"): (None, 10.0), ("codex", "a"): (40, 60.0)})
    stopped = decision(repo, "a", provider="codex")
    assert stopped.action == "pause" and "reserve 40%" in stopped.reason
    assert decision(repo, "a").action == "run"


def test_global_reserve_applies_without_an_account_value(repo, tmp_path):
    configure(tmp_path, {("claude", "a"): (10, 80.0), ("claude", "b"): (None, 80.0)})
    unset = decision(repo, "b")
    assert unset.action == "pause" and "reserve 25%" in unset.reason
    assert decision(repo, "a").action == "run"


@pytest.fixture
def one_flow(monkeypatch):
    """Every part of a flow costs 3000 weighted tokens, at 1000 per 1% of either window."""
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window=calibration.SEVEN_DAY: [
        calibration.Ratio("a", "max20x", name, 1000.0, 6, 6.0, window) for name in ("sonnet", "opus", "haiku")])
    samples = [history.Sample("build", "integration", "sonnet", 1, 0, 0, 3000.0)] * 5
    samples += [history.Sample(role, None, "sonnet", 0, 0, 0, 3000.0)
                for role in ("planner", "plan_reviewer", "code_reviewer", "evaluator", "reporter")]
    monkeypatch.setattr(history, "samples", lambda conn, flow_repos=(): samples)


def preflight_of(repo):
    ctx = make_context(repo, overrides={"budget": {"account": "a"}},
                       state={"approvals": {}, "plan_sha256": "", "budget": {}})
    ctx.store.plan_md.write_text("# Plan\n")
    ctx.store.plan_json.write_text(json.dumps({"tasks": TASKS}))
    budget_gate.check(ctx, plan.digest(ctx.store), now=NOW)
    return ctx.store.read_state()["budget"]["preflight"]


def test_preflight_uses_the_account_reserve(repo, tmp_path, one_flow):
    configure(tmp_path, {("claude", "a"): (50, 30.0)})
    saved = preflight_of(repo)
    assert saved["fits"] is False and "50" in saved["reason"]


def test_preflight_fits_under_the_global_reserve_alone(repo, tmp_path, one_flow):
    configure(tmp_path, {("claude", "a"): (None, 30.0)})
    assert preflight_of(repo)["fits"] is True
