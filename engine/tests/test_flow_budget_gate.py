"""The budget gate of the plan phase: preflight stored in state, shown at the approve gate, approved once per plan."""

import json
import sqlite3
from datetime import datetime, timedelta, timezone

import pytest

from sakura.budget import history, preflight
from sakura.flow import budget_gate, plan, status_page
from sakura.flow.phases import plan as plan_phase
from sakura.usage.archive import calibration
from sakura.usage.archive import store as archive_store

from flow_support import make_context
from test_flow_plan import APPROVE, _ctx as plan_ctx

NOW = datetime.now(timezone.utc)
TASKS = [{"id": "T1", "title": "t", "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False,
          "task_class": "integration"}]


def usage_db(tmp_path, week=30.0, minutes_ago=5):
    """A usage archive under the isolated HOME with one plan period and a newest weekly snapshot."""
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(
        f'db = "{home}/usage.db"\n\n[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "{home}/.claude/projects"\n'
        f'\n[[plans]]\naccount = "a"\nplan = "max20x"\nsince = "2026-01-01"\n')
    conn = archive_store.open_db(home / "usage.db")
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES"
                 " ('a', ?, 'max', 'allowed', 20.0, '2099-01-01T00:00:00Z', ?, '2099-01-02T00:00:00Z')",
                 ((NOW - timedelta(minutes=minutes_ago)).isoformat(), week))
    conn.commit()
    conn.close()


def only_another_account(tmp_path):
    """A usage.toml whose only Claude source is not the engine's own config directory."""
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(
        f'[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "{home}/.claude-work/projects"\n')


@pytest.fixture
def thousand_per_pct(monkeypatch):
    """Every model class costs 1000 weighted tokens per 1% of either window."""
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window=calibration.SEVEN_DAY: [
        calibration.Ratio("a", "max20x", name, 1000.0, 6, 6.0, window) for name in ("sonnet", "opus", "haiku")])


def make_ctx(repo, account="a", tasks=TASKS):
    ctx = make_context(repo, overrides={"budget": {"account": account}} if account else None,
                       state={"approvals": {}, "plan_sha256": "", "budget": {}})
    ctx.store.plan_md.write_text("# Plan\n")
    ctx.store.plan_json.write_text(json.dumps({"tasks": tasks}))
    return ctx, plan.digest(ctx.store)


@pytest.fixture
def history_of(monkeypatch):
    """Every part of a flow (five build tasks of one file, one run per role) costs `cost` weighted tokens."""
    def install(cost, model_class="sonnet"):
        samples = [history.Sample("build", "integration", model_class, 1, 0, 0, cost)] * 5
        samples += [history.Sample(role, None, model_class, 0, 0, 0, cost)
                    for role in ("planner", "plan_reviewer", "code_reviewer", "evaluator", "reporter")]
        monkeypatch.setattr(history, "samples", lambda conn, flow_repos=(): samples)
    return install


def stored(ctx):
    return ctx.store.read_state()["budget"]["preflight"]


def test_gate_is_off_without_a_usage_archive(repo):
    ctx, digest = make_ctx(repo, account="")
    assert budget_gate.check(ctx, digest, now=NOW).kind == "next"
    saved = stored(ctx)
    assert saved["off"] is True and saved["fits"] is True and saved["week_pct"] is None
    assert "no usage archive configured" in saved["reason"] and saved["plan_sha256"] == digest
    ledger = ctx.store.ledger.read_text()
    assert "[budget] off (no usage archive configured)" in ledger and "notify" not in ledger


def test_explicit_account_without_a_usage_archive_asks(repo):
    ctx, digest = make_ctx(repo, account="a")
    outcome = budget_gate.check(ctx, digest, now=NOW)
    assert (outcome.kind, outcome.gate) == ("await", "budget")
    saved = stored(ctx)
    assert saved["fits"] is False and saved["off"] is False
    assert "usage archive is not configured" in saved["reason"]


def test_the_plan_phase_passes_without_a_usage_archive(repo):
    ctx, runner = plan_ctx(repo, [APPROVE])
    assert plan_phase.run(ctx).kind == "next"
    assert ctx.store.read_state()["budget"]["preflight"]["off"] is True


def test_opus_extra_text_is_empty_without_a_usage_archive(repo):
    ctx, _ = make_ctx(repo, account="")
    assert budget_gate.opus_extra_text(ctx, plan.load(ctx.store, None)[0]) == ""


def test_an_empty_account_is_the_engines_own_account(repo, tmp_path, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(100.0)
    ctx, digest = make_ctx(repo, account="")
    assert budget_gate.check(ctx, digest, now=NOW).kind == "next"
    assert stored(ctx)["fits"] is True and stored(ctx)["quota_pct"] == 30.0


def test_an_empty_account_that_cannot_be_resolved_stores_an_unknown_quota_and_asks(repo, tmp_path):
    only_another_account(tmp_path)
    ctx, digest = make_ctx(repo, account="")
    outcome = budget_gate.check(ctx, digest, now=NOW)
    assert (outcome.kind, outcome.gate) == ("await", "budget")
    saved = stored(ctx)
    assert saved["fits"] is False and saved["level"] == "ask" and saved["plan_sha256"] == digest
    assert "quota unknown" in saved["reason"] and "budget.account is empty" in saved["reason"]


def test_a_cheap_flow_passes_and_stores_the_numbers(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(100.0)
    ctx, digest = make_ctx(repo)
    assert budget_gate.check(ctx, digest, now=NOW).kind == "next"
    saved = stored(ctx)
    assert saved["fits"] is True and saved["level"] == "ok" and saved["plan_sha256"] == digest
    assert len(saved["week_pct"]) == 3 and saved["basis"] and saved["computed_at"] and saved["reason"] == ""
    assert saved["text"].startswith("about ")


def test_an_expensive_flow_waits_at_the_budget_gate_with_the_line(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(10000.0)
    ctx, digest = make_ctx(repo)
    outcome = budget_gate.check(ctx, digest, now=NOW)
    assert (outcome.kind, outcome.gate) == ("await", "budget")
    assert outcome.reason.startswith(stored(ctx)["text"])
    ledger = ctx.store.ledger.read_text()
    assert "[budget]" in ledger and "asks" in ledger and stored(ctx)["text"] in ledger
    assert "notify budget_ask" in ledger


def test_the_request_alert_has_one_named_hook(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(10000.0)
    ctx, digest = make_ctx(repo)
    seen = []
    monkeypatch.setattr(budget_gate, "record_request", lambda ctx, result, line: seen.append((result, line)))
    budget_gate.check(ctx, digest, now=NOW)
    assert len(seen) == 1 and isinstance(seen[0][0], preflight.Preflight) and seen[0][1] == stored(ctx)["text"]


def test_a_warning_is_shown_and_notified_but_does_not_stop(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(700.0)
    ctx, digest = make_ctx(repo)
    assert budget_gate.check(ctx, digest, now=NOW).kind == "next"
    assert stored(ctx)["level"] == "warn"
    assert "notify budget_warning" in ctx.store.ledger.read_text()


def test_a_stale_snapshot_asks_and_says_since_when(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path, minutes_ago=90)
    history_of(100.0)
    ctx, digest = make_ctx(repo)
    outcome = budget_gate.check(ctx, digest, now=NOW)
    assert outcome.kind == "await"
    assert "quota unknown" in outcome.reason and "90 min old" in outcome.reason
    assert stored(ctx)["fits"] is False


def test_an_unreadable_archive_asks_instead_of_crashing(repo, tmp_path, monkeypatch):
    usage_db(tmp_path)
    monkeypatch.setattr(archive_store, "open_db", lambda path: (_ for _ in ()).throw(sqlite3.OperationalError("locked")))
    ctx, digest = make_ctx(repo)
    outcome = budget_gate.check(ctx, digest, now=NOW)
    assert outcome.kind == "await" and "usage archive" in outcome.reason and "locked" in outcome.reason


def approve(ctx, digest):
    ctx.store.update_state(lambda state: state["approvals"].update({"budget": {"at": "now", "plan_sha256": digest}}))


def test_an_approved_plan_continues_without_recomputing(repo, monkeypatch):
    ctx, digest = make_ctx(repo)
    approve(ctx, digest)
    monkeypatch.setattr(budget_gate, "compute", lambda *args, **kwargs: pytest.fail("must not recompute"))
    assert budget_gate.check(ctx, digest, now=NOW).kind == "next"


def test_the_approval_of_another_plan_does_not_count(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(10000.0)
    ctx, digest = make_ctx(repo)
    approve(ctx, "0" * 64)
    assert budget_gate.check(ctx, digest, now=NOW).kind == "await"


def test_the_plan_phase_gates_then_continues_without_planning_again(repo, tmp_path, monkeypatch, history_of, thousand_per_pct):
    usage_db(tmp_path)
    history_of(10000.0)
    ctx, runner = plan_ctx(repo, [APPROVE])
    ctx.config.values["budget"]["account"] = "a"
    first = plan_phase.run(ctx)
    assert (first.kind, first.gate) == ("await", "budget")
    digest = ctx.store.read_state()["plan_sha256"]
    assert digest == plan.digest(ctx.store)
    runs_before = len(runner.requests)
    approve(ctx, digest)
    assert plan_phase.run(ctx).kind == "next"
    assert len(runner.requests) == runs_before


def test_a_changed_spec_plans_again_instead_of_reusing_the_waiting_plan(repo, tmp_path, monkeypatch, history_of,
                                                                        thousand_per_pct):
    usage_db(tmp_path)
    history_of(10000.0)
    ctx, runner = plan_ctx(repo, [APPROVE, APPROVE])
    ctx.config.values["budget"]["account"] = "a"
    plan_phase.run(ctx)
    runs_before = len(runner.requests)
    ctx.store.spec.write_text("# Spec\n\nA different request.\n")
    assert plan_phase.run(ctx).gate == "budget"
    assert len(runner.requests) > runs_before


def test_the_plan_phase_asks_when_the_archive_is_configured_but_the_quota_unknown(repo, tmp_path):
    only_another_account(tmp_path)
    ctx, runner = plan_ctx(repo, [APPROVE])
    outcome = plan_phase.run(ctx)
    assert (outcome.kind, outcome.gate) == ("await", "budget")
    assert "quota unknown" in outcome.reason
    assert ctx.store.read_state()["budget"]["preflight"]["fits"] is False


def test_the_engine_reason_lets_the_cockpit_approve_the_budget_gate(repo, tmp_path, monkeypatch, history_of,
                                                                    thousand_per_pct):
    from sakura.cockpit import items
    from sakura.flow import engine

    usage_db(tmp_path)
    history_of(10000.0)
    ctx, digest = make_ctx(repo)
    outcome = budget_gate.check(ctx, digest, now=NOW)
    engine._stop_at(ctx, "plan", outcome)
    state = ctx.store.read_state()
    assert state["status"] == "awaiting_approval" and stored(ctx)["text"] in state["reason"]
    assert items._next_command("demo", state, "gate") == "sakura flow approve demo budget --continue"


# -- the extra cost of an Opus run --------------------------------------------------------------


def test_opus_extra_text_without_a_budget_account_says_the_quota_is_unknown(repo, tmp_path):
    only_another_account(tmp_path)
    ctx, _ = make_ctx(repo, account="")
    task = plan.load(ctx.store, None)[0]
    text = budget_gate.opus_extra_text(ctx, task)
    assert text.startswith(" Extra for the Opus run: no percent estimate; quota unknown")


def test_opus_extra_text_gives_the_cost_of_an_opus_run(repo, tmp_path, monkeypatch, thousand_per_pct):
    usage_db(tmp_path)
    opus = [history.Sample("build", "integration", "opus", 1, 0, 0, 5000.0)] * 6
    monkeypatch.setattr(history, "samples", lambda conn, flow_repos=(): opus)
    ctx, _ = make_ctx(repo)
    task = plan.load(ctx.store, None)[0]
    text = budget_gate.opus_extra_text(ctx, task)
    assert text.startswith(" Extra for the Opus run: about 5% of your week")


def test_opus_extra_text_says_when_it_cannot_be_computed(repo, tmp_path):
    usage_db(tmp_path)
    ctx, _ = make_ctx(repo)
    task = plan.load(ctx.store, None)[0]
    text = budget_gate.opus_extra_text(ctx, task)
    assert text.startswith(" Extra for the Opus run: no percent estimate")


# -- the status page and the approve command ----------------------------------------------------


def test_the_status_page_shows_the_stored_line():
    saved = {"text": "about 8% of your week (4-15), 12% of the 5-hour window; max20x calibration", "fits": False,
             "reason": "week at 74% plus up to 15% passes the 25% reserve"}
    state = {"task": "t1", "ref": "r", "phase": "plan", "status": "awaiting_approval", "reason": "", "history": [],
             "build": {}, "completed": {}, "active_runs": [], "pr": {}, "budget": {"preflight": saved}}
    page = status_page.render(state, [], [], None, datetime(2026, 10, 28, 12, tzinfo=timezone.utc), project="demo")
    assert "## Costo previsto" in page
    assert saved["text"] in page and "passes the 25% reserve" in page


def test_the_status_page_has_no_cost_section_before_a_preflight():
    state = {"task": "t1", "ref": "r", "phase": "plan", "status": "running", "reason": "", "history": [],
             "build": {}, "completed": {}, "active_runs": [], "pr": {}}
    page = status_page.render(state, [], [], None, datetime(2026, 10, 28, 12, tzinfo=timezone.utc), project="demo")
    assert "Costo previsto" not in page
