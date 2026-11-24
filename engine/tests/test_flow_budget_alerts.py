"""The request alerts of a flow: opened by the budget gate and guard, resolved on release, resume and finish."""

import sqlite3

import pytest

from sakura import install
from sakura.budget import alerts
from sakura.flow import budget_gate, cli, engine, exits, store
from sakura.flow.phases import evaluate  # noqa: F401 (importing the phases registers them; the fixture clears them)
from sakura.usage.archive import store as archive_store

from test_flow_engine_budget import (PREFLIGHT, WEEK_RESET, budget_clock, ctx_for, isolated_registry,  # noqa: F401
                                     set_snapshot)


def alert_rows(db):
    conn = archive_store.open_db(db)
    try:
        return conn.execute("SELECT id, account, kind, title, reason, resolved_at FROM budget_alerts ORDER BY id").fetchall()
    finally:
        conn.close()


def pause_at_the_reserve(repo, tmp_path, task="demo"):
    db = set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(repo, task=task)
    engine.register("intake", lambda c: engine.PhaseOutcome("next"))
    engine.advance(ctx)
    return db, ctx


def test_a_pause_opens_a_request_alert_with_the_numbers(repo, tmp_path):
    db, ctx = pause_at_the_reserve(repo, tmp_path)

    ((identifier, account, kind, title, reason, resolved),) = alert_rows(db)

    assert identifier == f"request:a:{repo.name}:demo:{WEEK_RESET}" and (account, kind, resolved) == ("a", "request", None)
    assert "demo" in title and "pause until" in title
    assert "week 80%" in reason and "reaches 75%" in reason and "reserve 25%" in reason


def test_a_repeated_wait_keeps_one_alert(repo, tmp_path):
    db, ctx = pause_at_the_reserve(repo, tmp_path)
    engine.advance(ctx)
    assert len(alert_rows(db)) == 1


def test_a_preflight_ask_opens_a_request_alert(repo, tmp_path):
    db = set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(repo)
    result = type("Result", (), {"reason": "week at 10% plus up to 70% passes the 25% reserve"})()

    budget_gate.record_request(ctx, result, "about 40% of your week (30-70), 5-hour window not calibrated; no calibration")

    ((_, _, kind, title, reason, _),) = alert_rows(db)
    assert kind == "request" and "asks" in title and "about 40% of your week" in title
    assert "passes the 25% reserve" in reason


def test_without_a_usage_archive_a_flow_writes_no_alert(repo, tmp_path):
    ctx = ctx_for(repo)
    budget_gate.record_request(ctx, type("Result", (), {"reason": ""})(), "about 40% of your week")
    assert not (tmp_path / "isolated-home/.local/state/sakura/usage.db").exists()
    assert "alert not recorded" not in ctx.store.ledger.read_text()


def test_a_request_alert_that_cannot_be_written_is_ledgered_and_never_stops_the_flow(repo, tmp_path, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(repo)

    def locked(*args):
        raise sqlite3.OperationalError("database is locked")

    monkeypatch.setattr(alerts, "open_request", locked)
    budget_gate.record_request(ctx, type("Result", (), {"reason": ""})(), "about 40% of your week")
    assert "alert not recorded: OperationalError: database is locked" in ctx.store.ledger.read_text()


def test_the_guard_passing_resolves_the_alert(repo, tmp_path):
    db, ctx = pause_at_the_reserve(repo, tmp_path)
    engine.register("contract", lambda c: engine.PhaseOutcome("blocked", "reached contract"))
    set_snapshot(tmp_path, week=10.0)

    assert engine.advance(ctx) == exits.BLOCKED

    assert alert_rows(db)[0][5] is not None


def test_a_flow_that_finishes_resolves_its_alert(repo, tmp_path):
    db, ctx = pause_at_the_reserve(repo, tmp_path)
    ctx.store.update_state(lambda state: state.update(phase="cleanup", status="report_ready", waiting=None))
    set_snapshot(tmp_path, week=10.0)
    engine.register("cleanup", lambda c: engine.PhaseOutcome("next"))

    assert engine.advance(ctx) == exits.OK

    assert alert_rows(db)[0][5] is not None


@pytest.fixture
def project(repo, monkeypatch):
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SAKURA_ACTOR", raising=False)
    return repo


def test_approving_the_budget_wait_gate_resolves_the_alert(project, tmp_path):
    db, ctx = pause_at_the_reserve(project, tmp_path)
    assert alert_rows(db)[0][5] is None

    assert cli.main(["approve", "demo", "budget-wait"]) == exits.OK

    assert alert_rows(db)[0][5] is not None


def test_a_preflight_approval_without_a_wait_resolves_the_alert(project, tmp_path):
    db = set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(project, state={"budget": PREFLIGHT})
    budget_gate.record_request(ctx, type("Result", (), {"reason": ""})(), "about 40% of your week")

    assert cli.main(["approve", "demo", "budget"]) == exits.OK

    assert alert_rows(db)[0][5] is not None
    assert store.TaskStore(project, "demo").read_state()["approvals"]["budget"]
