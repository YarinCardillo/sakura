"""The budget guard inside a running flow: safe points, role runs, holds, the wake command and the approve release."""

from datetime import datetime, timedelta, timezone

import pytest

from sakura import install
from sakura.budget import settings as budget_settings
from sakura.flow import budget_gate, budget_guard, cli, engine, exits, process, runner, store, wake
from sakura.flow.phases import evaluate  # noqa: F401 (importing the phases registers them; the fixture below clears them)
from sakura.usage.archive import store as archive_store

from flow_support import MonitorLauncher, make_context, task_workspace

UTC = timezone.utc
FAR = "2099-01-01T00:00:00+00:00"
NOW = datetime(2026, 11, 22, 12, 30, tzinfo=UTC)
WEEK_RESET = (NOW + timedelta(days=3)).isoformat()
PREFLIGHT = {"preflight": {"week_pct": [5.0, 10.0, 20.0], "plan_sha256": "p"}}


@pytest.fixture(autouse=True)
def budget_clock(monkeypatch):
    class FrozenDateTime(datetime):
        @classmethod
        def now(cls, tz=None):
            return NOW.astimezone(tz) if tz else NOW.replace(tzinfo=None)

    for module in (budget_gate, budget_guard, cli):
        monkeypatch.setattr(module, "datetime", FrozenDateTime)
    monkeypatch.setattr(budget_guard, "local_now", lambda: NOW)


@pytest.fixture(autouse=True)
def isolated_registry(monkeypatch):
    """`register` also drops the input digests of the phases it replaces, so both registries are swapped for the test."""
    monkeypatch.setattr(engine, "_HANDLERS", {})
    monkeypatch.setattr(engine, "_INPUTS", dict(engine._INPUTS))


def set_snapshot(tmp_path, week, five=10.0, minutes_ago=5, week_reset=WEEK_RESET, trend=(), now=None):
    """usage.toml and an archive under the isolated HOME with one account 'a' and its newest snapshot.

    `trend` is extra (hours_ago, week_pct) snapshots, older than the newest one; `now` defaults to the clock.
    """
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(
        f'db = "{home}/usage.db"\n\n[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "{home}/.claude/projects"\n')
    conn = archive_store.open_db(home / "usage.db")
    conn.execute("DELETE FROM quota_snapshots")
    now = now or NOW
    rows = [(now - timedelta(minutes=minutes_ago), week)] + [(now - timedelta(hours=hours), pct) for hours, pct in trend]
    for at, pct in rows:
        conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                     " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES"
                     " ('a', ?, 'max', 'allowed', ?, ?, ?, ?)", (at.isoformat(), five, FAR, pct, week_reset))
    conn.commit()
    conn.close()
    return home / "usage.db"


def holds(db):
    conn = archive_store.open_db(db)
    try:
        return dict(conn.execute("SELECT task_key, week_pct FROM budget_holds"))
    finally:
        conn.close()


def ctx_for(repo, task="demo", state=None, account="a"):
    return make_context(repo, task=task, overrides={"budget": {"account": account}}, state=state)


def ledger(ctx):
    return ctx.store.ledger.read_text()


def fresh(repo, task="demo"):
    """A new store on a task another store (a CLI run, the wake command) wrote: stores track what they wrote themselves."""
    return store.TaskStore(repo, task)


# -- a running flow crossing into the reserve stops at the next safe point ---------------------------------------

def test_running_flow_pauses_at_the_next_safe_point_and_keeps_state(repo, tmp_path):
    set_snapshot(tmp_path, week=40.0)
    ctx = ctx_for(repo, state={"build": {"T1": {"status": "building"}}, "plan_sha256": "abc"})
    ran = []

    def intake(c):
        ran.append("intake")
        set_snapshot(tmp_path, week=80.0)  # the week crosses into the reserve while intake runs
        return engine.PhaseOutcome("next")

    engine.register("intake", intake)
    engine.register("contract", lambda c: ran.append("contract") or engine.PhaseOutcome("next"))

    assert engine.advance(ctx) == exits.OK

    state = ctx.store.read_state()
    assert ran == ["intake"] and state["phase"] == "contract"
    assert state["status"] == "awaiting_approval"
    assert "sakura flow approve demo budget" in state["reason"] and "reaches 75%" in state["reason"]
    assert state["waiting"]["on"] == "budget" and state["waiting"]["action"] == "pause"
    assert state["waiting"]["until"] == WEEK_RESET
    assert state["build"] == {"T1": {"status": "building"}} and state["plan_sha256"] == "abc"
    text = ledger(ctx)
    assert f"Ruling: pause until {WEEK_RESET} (week 80%" in text
    assert "notify budget_pause" in text and "notify awaiting_approval" in text


def test_a_flow_without_a_usage_archive_is_never_guarded(repo):
    ctx = ctx_for(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("next"))
    engine.register("contract", lambda c: engine.PhaseOutcome("blocked", "reached contract"))
    assert engine.advance(ctx) == exits.BLOCKED
    assert ctx.store.read_state()["phase"] == "contract" and "waiting" not in ctx.store.read_state()


def test_stale_snapshot_asks_before_the_first_phase(repo, tmp_path):
    set_snapshot(tmp_path, week=1.0, minutes_ago=45)
    ctx = ctx_for(repo)
    ran = []
    engine.register("intake", lambda c: ran.append("intake") or engine.PhaseOutcome("next"))
    assert engine.advance(ctx) == exits.OK
    state = ctx.store.read_state()
    assert ran == [] and state["phase"] == "intake" and state["status"] == "awaiting_approval"
    assert state["waiting"] == {"on": "budget", "action": "ask", "until": None, "reason": state["waiting"]["reason"],
                                "rule": "staleness"}
    assert "45 min old" in state["waiting"]["reason"]
    assert "Ruling: ask, the owner decides (quota unknown" in ledger(ctx) and "notify budget_ask" in ledger(ctx)


def test_a_repeated_wait_is_ruled_and_notified_once(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("next"))
    engine.advance(ctx)
    engine.advance(ctx)
    assert ledger(ctx).count("Ruling: pause") == 1 and ledger(ctx).count("notify budget_pause") == 1


def test_the_guard_passing_clears_the_wait_and_the_flow_continues(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("next"))
    engine.register("contract", lambda c: engine.PhaseOutcome("blocked", "reached contract"))
    engine.advance(ctx)
    assert ctx.store.read_state()["waiting"]["action"] == "pause"
    set_snapshot(tmp_path, week=10.0)  # the week reset
    assert engine.advance(ctx) == exits.BLOCKED
    assert "waiting" not in ctx.store.read_state() and ctx.store.read_state()["phase"] == "contract"


def test_the_pr_and_cleanup_phases_spend_nothing_and_are_not_guarded(repo, tmp_path):
    set_snapshot(tmp_path, week=90.0)
    ctx = ctx_for(repo, state={"phase": "pr"})
    engine.register("pr", lambda c: engine.PhaseOutcome("blocked", "reached pr"))
    assert engine.advance(ctx) == exits.BLOCKED


# -- the check before each role run ------------------------------------------------------------------------------

def claude_runner(ctx, launched):
    def monitor(argv, **kwargs):
        launched.append(argv)
        return process.ProcessOutcome(0, False, False, False, 1)

    return runner.AgentRunner(ctx.store, ctx.config, launcher=MonitorLauncher(monitor))


def test_a_claude_role_run_is_refused_at_the_reserve(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"builder": "claude:sonnet"}})
    launched = []
    with pytest.raises(budget_guard.BudgetWait) as wait:
        claude_runner(ctx, launched).run(runner.RunRequest("builder", "build", "x", {}, repo))
    assert wait.value.decision.action == "pause" and launched == []


def test_a_role_that_is_not_claude_ignores_the_claude_reserve(repo, tmp_path):
    set_snapshot(tmp_path, week=99.0)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"builder": "exec:true"}})
    runner.AgentRunner(ctx.store, ctx.config).run(runner.RunRequest("builder", "build", "x", {}, task_workspace(ctx)))


def test_a_wait_raised_inside_a_phase_stops_it_with_the_state_intact(repo, tmp_path):
    set_snapshot(tmp_path, week=40.0)
    ctx = ctx_for(repo, state={"phase": "build", "build": {"T1": {"status": "review"}}})

    def build(c):
        set_snapshot(tmp_path, week=80.0)
        budget_guard.require(c.store, c.config)  # what the runner does before the next role run
        return engine.PhaseOutcome("next")

    engine.register("build", build)
    assert engine.advance(ctx) == exits.OK
    state = ctx.store.read_state()
    assert state["phase"] == "build" and state["status"] == "awaiting_approval" and state["waiting"]["action"] == "pause"
    assert state["build"] == {"T1": {"status": "review"}}


# -- holds -------------------------------------------------------------------------------------------------------

def test_the_hold_is_taken_when_building_starts_and_released_at_report_ready(repo, tmp_path):
    db = set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(repo, state={"phase": "build", "budget": PREFLIGHT})
    seen = []
    engine.register("build", lambda c: seen.append(holds(db)) or engine.PhaseOutcome("next"))
    for phase in ("evaluate", "pr"):
        engine.register(phase, lambda c: engine.PhaseOutcome("next"))

    def report(c):
        c.store.update_state(lambda state: state.update(status="report_ready"))
        return engine.PhaseOutcome("next")

    engine.register("report", report)

    assert engine.advance(ctx) == exits.OK

    assert seen == [{budget_guard.task_key(ctx.store): 20.0}]
    assert holds(db) == {}


def test_cleanup_releases_the_hold(repo, tmp_path):
    db = set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(repo, state={"phase": "cleanup", "status": "report_ready", "budget": PREFLIGHT})
    budget_guard.take_hold(ctx.store, ctx.config)
    assert len(holds(db)) == 1
    engine.register("cleanup", lambda c: engine.PhaseOutcome("next"))
    assert engine.advance(ctx) == exits.OK
    assert ctx.store.read_state()["status"] == "cleaned" and holds(db) == {}


def test_a_flow_without_a_percentage_estimate_takes_no_hold(repo, tmp_path):
    db = set_snapshot(tmp_path, week=10.0)
    ctx = ctx_for(repo, state={"phase": "build", "budget": {"preflight": {"week_pct": None}}})
    engine.register("build", lambda c: engine.PhaseOutcome("blocked", "built"))
    engine.advance(ctx)
    assert holds(db) == {}


def test_holds_count_for_the_second_flow_starting_to_build(repo, tmp_path):
    db = set_snapshot(tmp_path, week=40.0)
    first = ctx_for(repo, task="first", state={"phase": "build", "budget": PREFLIGHT})
    second = ctx_for(repo, task="second", state={"phase": "build", "budget": PREFLIGHT})
    engine.register("build", lambda c: engine.PhaseOutcome("blocked", "built"))
    assert engine.advance(first) == exits.BLOCKED  # 40 + 20 pending = 60 < 75: runs, holds 20
    assert engine.advance(second) == exits.OK      # 40 + 20 held + 20 pending = 80 >= 75
    assert second.store.read_state()["waiting"]["action"] == "pause"
    assert set(holds(db)) == {budget_guard.task_key(first.store)}, "a paused flow gives its hold back"


def test_a_weekly_pause_gives_the_hold_back_so_the_other_flow_can_run(repo, tmp_path):
    set_snapshot(tmp_path, week=40.0)
    first = ctx_for(repo, task="first", state={"phase": "build", "budget": PREFLIGHT})
    budget_guard.take_hold(first.store, first.config)
    second = ctx_for(repo, task="second", state={"phase": "build", "budget": PREFLIGHT})
    engine.register("build", lambda c: engine.PhaseOutcome("blocked", "built"))
    assert engine.advance(second) == exits.OK and second.store.read_state()["waiting"]["action"] == "pause"
    # the first flow now only competes with the live week: 40 + 20 pending
    assert engine.advance(first) == exits.BLOCKED


def test_later_phases_have_no_pending_amount(repo, tmp_path):
    set_snapshot(tmp_path, week=60.0)
    building = ctx_for(repo, state={"phase": "build", "budget": PREFLIGHT})
    evaluating = ctx_for(repo, task="other", state={"phase": "evaluate", "budget": PREFLIGHT})
    assert budget_guard.evaluate(building.store, building.config).action == "pause"
    assert budget_guard.evaluate(evaluating.store, evaluating.config).action == "run"


def test_a_flow_that_spends_half_its_estimate_keeps_running(repo, tmp_path):
    db = set_snapshot(tmp_path, week=40.0)
    ctx = ctx_for(repo, state={"phase": "build", "budget": PREFLIGHT})
    budget_guard.take_hold(ctx.store, ctx.config)
    assert holds(db) == {budget_guard.task_key(ctx.store): 20.0}
    set_snapshot(tmp_path, week=50.0)  # 10 of its 20 points are spent, 10 to come: 60 < 75
    assert budget_guard.evaluate(ctx.store, ctx.config).action == "run"
    set_snapshot(tmp_path, week=56.0)  # 16 spent: 56 + 4 < 75, where counting all 20 again would stop it at 76
    assert budget_guard.evaluate(ctx.store, ctx.config).action == "run"


def test_the_post_build_phases_are_still_checked_against_the_reserve(repo, tmp_path):
    set_snapshot(tmp_path, week=72.0)
    preflight = {"preflight": {**PREFLIGHT["preflight"], "post_build_pct": 4.0}}
    evaluating = ctx_for(repo, state={"phase": "evaluate", "budget": preflight})
    reporting = ctx_for(repo, task="other", state={"phase": "report", "budget": preflight})
    for ctx in (evaluating, reporting):
        decision = budget_guard.evaluate(ctx.store, ctx.config)
        assert decision.action == "pause" and "72%" in decision.reason
    set_snapshot(tmp_path, week=70.0)
    assert budget_guard.evaluate(evaluating.store, evaluating.config).action == "run"


def test_building_again_after_a_weekly_reset_takes_the_hold_afresh(repo, tmp_path):
    db = set_snapshot(tmp_path, week=40.0)
    ctx = ctx_for(repo, state={"phase": "build", "budget": PREFLIGHT})
    budget_guard.take_hold(ctx.store, ctx.config, NOW - timedelta(days=5))
    conn = archive_store.open_db(db)
    old = conn.execute("SELECT created_at FROM budget_holds").fetchone()[0]
    conn.close()
    set_snapshot(tmp_path, week=3.0)  # the week reset since: the hold dates from the week before
    budget_guard.take_hold(ctx.store, ctx.config)
    conn = archive_store.open_db(db)
    row = conn.execute("SELECT created_at, week_at_hold FROM budget_holds").fetchone()
    conn.close()
    assert row[0] > old and row[1] == 3.0


# -- urgent and --when-cheap -------------------------------------------------------------------------------------

def test_a_flow_that_is_not_urgent_defers_to_the_night_when_the_pace_is_high(repo, tmp_path):
    noon = NOW.replace(hour=11, minute=0).astimezone(timezone(timedelta(hours=1)))
    set_snapshot(tmp_path, week=30.0, trend=[(hours, 30.0 - hours * 2.0) for hours in range(1, 6)], now=noon)
    urgent = ctx_for(repo, task="urgent")
    cheap = ctx_for(repo, task="cheap", state={"urgent": False})
    assert budget_guard.evaluate(urgent.store, urgent.config, noon).action == "run"
    decision = budget_guard.evaluate(cheap.store, cheap.config, noon)
    assert decision.action == "defer" and decision.until.hour == 23


# -- the person's approval releases a wait ------------------------------------------------------------------------

@pytest.fixture
def project(repo, monkeypatch):
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SAKURA_ACTOR", raising=False)
    return repo


def test_approving_the_budget_gate_releases_a_budget_wait(project, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(project)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", "reached intake"))
    engine.advance(ctx)
    assert ctx.store.read_state()["waiting"]["action"] == "pause"

    assert cli.main(["approve", "demo", "budget-wait"]) == exits.OK

    ctx = engine.open_context(project, fresh(project))
    state = ctx.store.read_state()
    assert "waiting" not in state and state["budget"]["released_until"]
    assert "budget wait released" in ledger(ctx)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", "reached intake"))
    assert engine.advance(ctx) == exits.BLOCKED, "the guard no longer holds a released flow"
    assert ctx.store.read_state()["reason"] == "reached intake"


def release(repo, task="demo", now=None):
    ctx = engine.open_context(repo, fresh(repo, task))
    settings = budget_settings.load(ctx.config.values)
    assert budget_guard.release_wait(ctx.store, settings, now or NOW)
    return fresh(repo, task), ctx.config


def test_a_released_pause_lasts_to_the_weekly_reset_and_not_into_the_next_window(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    waiting_flow(repo, until=WEEK_RESET, action="pause")
    task_store, cfg = release(repo)
    budget = task_store.read_state()["budget"]
    assert budget["released_until"] == WEEK_RESET and budget["released_action"] == "pause"
    assert budget_guard.evaluate(task_store, cfg).action == "run"
    next_window = datetime.fromisoformat(WEEK_RESET) + timedelta(hours=1)
    set_snapshot(tmp_path, week=80.0, now=next_window, week_reset=(next_window + timedelta(days=7)).isoformat())
    assert budget_guard.evaluate(task_store, cfg, next_window).action == "pause"


def test_a_released_five_hour_defer_lasts_to_its_reset(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0, five=95.0)
    reset = NOW + timedelta(hours=2)
    waiting_flow(repo, until=reset.isoformat(timespec="seconds"), action="defer")
    task_store, cfg = release(repo)
    assert task_store.read_state()["budget"]["released_until"] == reset.isoformat(timespec="seconds")
    assert budget_guard.evaluate(task_store, cfg).action == "run"
    later = reset + timedelta(minutes=1)
    set_snapshot(tmp_path, week=10.0, five=95.0, now=later)
    assert budget_guard.evaluate(task_store, cfg, later).action == "defer"


def test_a_released_ask_lasts_the_snapshot_max_age(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0, minutes_ago=45)
    ctx = waiting_flow(repo, action="ask")
    ctx.store.update_state(lambda state: state["waiting"].update(until=None))
    start = NOW
    task_store, cfg = release(repo, now=start)
    assert task_store.read_state()["budget"]["released_until"] == (start + timedelta(minutes=30)).isoformat(timespec="seconds")
    assert budget_guard.evaluate(task_store, cfg, start + timedelta(minutes=29)).action == "run"
    later = start + timedelta(minutes=31)
    set_snapshot(tmp_path, week=10.0, now=later, minutes_ago=45)
    assert budget_guard.evaluate(task_store, cfg, later).action == "ask"


def test_a_released_ask_lifts_the_staleness_rule_only(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0, minutes_ago=45)
    ctx = waiting_flow(repo, action="ask")
    ctx.store.update_state(lambda state: state["waiting"].update(until=None))
    task_store, cfg = release(repo)
    assert budget_guard.evaluate(task_store, cfg).action == "pause", "the weekly reserve still applies"


def test_a_released_pause_lifts_the_weekly_reserve_only(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0, five=95.0)
    waiting_flow(repo, until=WEEK_RESET, action="pause")
    task_store, cfg = release(repo)
    decision = budget_guard.evaluate(task_store, cfg)
    assert decision.action == "defer" and "5-hour" in decision.reason
    set_snapshot(tmp_path, week=80.0, minutes_ago=45)
    assert budget_guard.evaluate(task_store, cfg).action == "ask", "the staleness rule still applies"


def test_a_released_five_hour_defer_lifts_that_reserve_only(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0, five=95.0)
    reset = (NOW + timedelta(hours=2)).isoformat(timespec="seconds")
    waiting_flow(repo, until=reset, action="defer")
    task_store, cfg = release(repo)
    assert budget_guard.evaluate(task_store, cfg).action == "pause", "the weekly reserve still applies"
    set_snapshot(tmp_path, week=10.0, five=95.0)
    assert budget_guard.evaluate(task_store, cfg).action == "run"


def test_a_wait_remembers_the_rule_that_stopped_the_flow_and_its_release_lifts_that_rule(repo, tmp_path):
    noon = NOW.replace(hour=11, minute=0).astimezone(timezone(timedelta(hours=1)))
    set_snapshot(tmp_path, week=30.0, trend=[(hours, 30.0 - hours * 2.0) for hours in range(1, 6)], now=noon)
    ctx = ctx_for(repo, task="cheap", state={"urgent": False})
    decision = budget_guard.evaluate(ctx.store, ctx.config, noon)
    assert decision.rule == "pacing"
    budget_guard.park(ctx.store, decision, noon)
    assert ctx.store.read_state()["waiting"]["rule"] == "pacing"
    ctx.store.update_state(lambda state: state.update(status="awaiting_approval"))
    task_store, cfg = release(repo, task="cheap", now=noon)
    assert task_store.read_state()["budget"]["released_rule"] == "pacing"
    assert budget_guard.evaluate(task_store, cfg, noon).action == "run"


def test_approving_the_budget_gate_needs_a_preflight(project, capsys):
    ctx_for(project)
    assert cli.main(["approve", "demo", "budget"]) == exits.USAGE
    assert "no budget preflight" in capsys.readouterr().out


def test_approving_the_budget_wait_needs_a_wait(project, capsys):
    ctx = ctx_for(project, state={"budget": PREFLIGHT})
    assert cli.main(["approve", "demo", "budget-wait"]) == exits.USAGE
    assert "not waiting on the budget" in capsys.readouterr().out
    assert "budget-wait" not in ctx.store.read_state()["approvals"]


def test_a_guard_wait_stops_at_the_budget_wait_gate(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(repo)
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", "reached intake"))
    engine.advance(ctx)
    assert "flow approve demo budget-wait (" in ctx.store.read_state()["reason"]


def test_approving_the_budget_gate_does_not_release_a_guard_wait(project, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(project, state={"budget": PREFLIGHT, "plan_sha256": "p"})
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", "reached intake"))
    engine.advance(ctx)
    assert cli.main(["approve", "demo", "budget"]) == exits.OK
    state = fresh(project).read_state()
    assert state["waiting"]["action"] == "pause" and "released_until" not in state["budget"]
    assert state["approvals"]["budget"]["plan_sha256"] == "p"


def test_approving_the_budget_wait_does_not_approve_the_preflight(project, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    ctx = ctx_for(project, state={"budget": PREFLIGHT, "plan_sha256": "p"})
    engine.register("intake", lambda c: engine.PhaseOutcome("blocked", "reached intake"))
    engine.advance(ctx)
    assert cli.main(["approve", "demo", "budget-wait"]) == exits.OK
    state = fresh(project).read_state()
    assert "budget" not in state["approvals"] and "waiting" not in state and state["budget"]["released_until"]


# -- flow start --when-cheap -------------------------------------------------------------------------------------

@pytest.fixture
def specs(project):
    (project / "specs").mkdir()
    (project / "specs/add-greeting.md").write_text("add a greeting\n")
    return project


def start(*extra):
    return cli.main(["start", "specs/add-greeting.md", "--role", "planner=exec:true", *extra])


def test_flow_start_is_urgent_by_default(specs):
    start()
    assert store.TaskStore(specs, "add-greeting").read_state()["urgent"] is True


def test_flow_start_when_cheap_stores_not_urgent(specs):
    start("--when-cheap")
    assert store.TaskStore(specs, "add-greeting").read_state()["urgent"] is False


# -- wake: the timed resume ---------------------------------------------------------------------------------------

def waiting_flow(repo, task="demo", until=None, status="awaiting_approval", action="pause"):
    until = until or (NOW - timedelta(minutes=1)).isoformat(timespec="seconds")
    return ctx_for(repo, task=task, state={"status": status, "phase": "build", "waiting": {
        "on": "budget", "action": action, "until": until, "reason": "week 80%"}})


class Resumer:
    def __init__(self):
        self.resumed = []

    def __call__(self, repo, task_store):
        self.resumed.append(task_store.task_id)


def test_wake_resumes_a_due_wait_that_may_run_now(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo)
    resume = Resumer()
    lines = wake.wake(resume, repo_list=lambda: [repo])
    assert resume.resumed == ["demo"] and lines == ["demo: resumed"]


def test_wake_leaves_a_wait_that_is_not_due(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo, until=(NOW + timedelta(hours=2)).isoformat(timespec="seconds"))
    resume = Resumer()
    assert wake.wake(resume, repo_list=lambda: [repo]) == [] and resume.resumed == []


def test_wake_leaves_a_wait_the_owner_decides(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    ctx = waiting_flow(repo, action="ask")
    ctx.store.update_state(lambda s: s["waiting"].update(until=None))
    resume = Resumer()
    assert wake.wake(resume, repo_list=lambda: [repo]) == [] and resume.resumed == []


def test_wake_moves_the_wait_when_the_guard_still_says_no(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    waiting_flow(repo, action="defer")
    resume = Resumer()
    lines = wake.wake(resume, repo_list=lambda: [repo])
    assert resume.resumed == [] and lines[0].startswith(f"demo: still waiting (pause until {WEEK_RESET}")
    assert fresh(repo).read_state()["waiting"]["action"] == "pause"
    assert f"Ruling: pause until {WEEK_RESET}" in fresh(repo).ledger.read_text()


def test_wake_ignores_flows_that_are_not_awaiting(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo, status="running")
    resume = Resumer()
    assert wake.wake(resume, repo_list=lambda: [repo]) == [] and resume.resumed == []


def test_wake_reports_a_failed_resume_and_goes_on(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo, task="first"), waiting_flow(repo, task="second")

    def resume(repo_path, task_store):
        if task_store.task_id == "first":
            raise RuntimeError("no tmux")

    lines = wake.wake(resume, repo_list=lambda: [repo])
    assert lines == ["first: wake failed: RuntimeError: no tmux", "second: resumed"]
    assert "wake failed: RuntimeError: no tmux" in fresh(repo, "first").ledger.read_text()


def test_wake_without_a_tmux_server_resumes_nothing_and_says_so_in_one_line(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo, task="first"), waiting_flow(repo, task="second")
    waiting_flow(repo, task="later", until=(NOW + timedelta(hours=2)).isoformat(timespec="seconds"))
    resume = Resumer()
    lines = wake.wake(resume, repo_list=lambda: [repo], server_running=lambda: False)
    assert lines == ["wake: no tmux server, 2 flows due"] and resume.resumed == []
    for task in ("first", "second"):
        assert "wake" not in fresh(repo, task).ledger.read_text()


def test_wake_without_a_tmux_server_and_nothing_due_says_nothing(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo, until=(NOW + timedelta(hours=2)).isoformat(timespec="seconds"))
    assert wake.wake(Resumer(), repo_list=lambda: [repo], server_running=lambda: False) == []


def test_wake_with_a_tmux_server_records_which_checkout_resumed_the_flow(repo, tmp_path):
    from sakura.flow import surface

    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo)
    resume = Resumer()
    assert wake.wake(resume, repo_list=lambda: [repo], server_running=lambda: True) == ["demo: resumed"]
    assert f"wake resumes the flow with {surface.KIT_BIN}" in fresh(repo).ledger.read_text()


def test_flow_wake_command_does_not_start_a_tmux_server(project, tmp_path, monkeypatch, capsys):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(project)
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    from sakura.flow import registry
    registry.remember(project)
    detached = []
    monkeypatch.setattr(cli, "_detach", lambda cfg, task_store, command, before=None: detached.append(command) or exits.OK)
    monkeypatch.setattr(cli, "tmux_server_running", lambda: False)

    assert cli.main(["wake"]) == exits.OK

    assert detached == [] and capsys.readouterr().out.strip() == "wake: no tmux server, 1 flows due"


def test_flow_wake_command_resumes_through_the_detached_path(project, tmp_path, monkeypatch, capsys):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(project)
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    from sakura.flow import registry
    registry.remember(project)
    detached = []
    monkeypatch.setattr(cli, "_detach", lambda cfg, task_store, command, before=None: detached.append(command) or exits.OK)
    monkeypatch.setattr(cli, "tmux_server_running", lambda: True)

    assert cli.main(["wake"]) == exits.OK

    assert detached == [["resume", "--actor=cli:budget-wake", "--", "demo"]]
    assert "demo: resumed" in capsys.readouterr().out


def test_wake_keeps_a_timed_wait_when_the_snapshot_is_stale(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0, minutes_ago=45)
    waiting_flow(repo)
    resume = Resumer()
    lines = wake.wake(resume, repo_list=lambda: [repo])

    assert resume.resumed == [] and lines[0].startswith("demo: quota unknown, waiting on (")
    waiting = fresh(repo).read_state()["waiting"]
    assert waiting["action"] == "pause", "the wait is not turned into an owner-only ask"
    until = datetime.fromisoformat(waiting["until"])
    assert until == NOW + wake.WAKE_INTERVAL
    assert "Ruling" not in fresh(repo).ledger.read_text()
    assert wake.wake(resume, repo_list=lambda: [repo]) == [], "not due again before the next interval"


@pytest.mark.parametrize("local_hour,resumed", [(23, True), (12, False)])
def test_wake_reads_the_night_window_in_local_time(repo, tmp_path, monkeypatch, local_hour, resumed):
    local_zone = timezone(timedelta(hours=local_hour - NOW.hour))
    monkeypatch.setattr(budget_guard, "local_now", lambda: NOW.astimezone(local_zone))
    set_snapshot(tmp_path, week=30.0, trend=[(hours, 30.0 - hours * 2.0) for hours in range(1, 6)])
    waiting_flow(repo, action="defer")
    ctx = fresh(repo)
    ctx.update_state(lambda state: state.update(urgent=False))
    resume = Resumer()

    lines = wake.wake(resume, repo_list=lambda: [repo])

    assert (resume.resumed == ["demo"]) is resumed, lines
    if not resumed:
        assert "defer until" in lines[0] and "T23:00:00" in lines[0]


def test_a_pause_with_no_known_reset_does_not_rule_again_at_every_wake(repo, tmp_path):
    set_snapshot(tmp_path, week=90.0, week_reset=None)
    waiting_flow(repo)
    resume = Resumer()
    for step in range(3):
        wake.wake(resume, now=NOW + timedelta(minutes=15 * step), repo_list=lambda: [repo])
    assert resume.resumed == [] and fresh(repo).ledger.read_text().count("Ruling") == 0


def test_one_repository_that_cannot_be_read_does_not_skip_the_others(repo, tmp_path, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    waiting_flow(repo)
    real = wake.tasks.known_tasks
    broken = tmp_path / "broken"
    broken.mkdir()

    def known(path):
        if path == broken:
            raise PermissionError("denied")
        return real(path)

    monkeypatch.setattr(wake.tasks, "known_tasks", known)
    resume = Resumer()
    lines = wake.wake(resume, repo_list=lambda: [broken, repo])
    assert lines[0] == f"{broken}: wake failed: PermissionError: denied" and lines[1] == "demo: resumed"


# -- Codex roles: guarded by the Codex account's own snapshot ------------------------------------------------------

def set_codex_source(tmp_path, week=None, minutes_ago=5, name="a", reset=WEEK_RESET):
    """Add a Codex source to the usage.toml of `set_snapshot` and, when `week` is given, its newest snapshot."""
    home = tmp_path / "isolated-home"
    config = home / ".config/sakura/usage.toml"
    config.write_text(config.read_text() + f'\n[[sources]]\nprovider = "codex"\naccount = "{name}"\n'
                      f'root = "{home}/.codex/sessions"\n')
    if week is None:
        return
    conn = archive_store.open_db(home / "usage.db")
    at = NOW - timedelta(minutes=minutes_ago)
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at) VALUES"
                 " (?, ?, 'prolite', 'allowed', NULL, NULL, ?, ?)", (f"codex:{name}", at.isoformat(), week, reset))
    conn.commit()
    conn.close()


def codex_decision(repo, **kwargs):
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"code_reviewer": "codex:gpt"}}, **kwargs)
    return ctx, budget_guard.evaluate(ctx.store, ctx.config, provider="codex")


def test_codex_role_is_guarded_by_its_codex_snapshot(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    set_codex_source(tmp_path, week=80.0)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"code_reviewer": "codex:gpt"}})
    launched = []

    with pytest.raises(budget_guard.BudgetWait) as wait:
        claude_runner(ctx, launched).run(runner.RunRequest("code_reviewer", "review", "x", {}, repo))

    assert wait.value.decision.action == "pause" and launched == []


def test_stale_codex_snapshot_is_a_lower_bound_not_unknown(repo, tmp_path):
    set_snapshot(tmp_path, week=10.0)
    set_codex_source(tmp_path, week=40.0, minutes_ago=600)
    assert codex_decision(repo)[1].action == "run"

    set_snapshot(tmp_path, week=10.0)
    set_codex_source(tmp_path, week=85.0, minutes_ago=600)
    assert codex_decision(repo, task="two")[1].action == "pause"


def test_claude_roles_unchanged_by_codex_mapping(repo, tmp_path):
    set_snapshot(tmp_path, week=80.0)
    set_codex_source(tmp_path, week=5.0)
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"builder": "claude:sonnet"}})
    with pytest.raises(budget_guard.BudgetWait) as wait:
        claude_runner(ctx, []).run(runner.RunRequest("builder", "build", "x", {}, repo))
    assert wait.value.decision.action == "pause"

    set_snapshot(tmp_path, week=10.0)
    set_codex_source(tmp_path, week=95.0)
    assert budget_guard.evaluate(ctx.store, ctx.config).action == "run"


def test_no_codex_source_leaves_codex_roles_unguarded_with_one_ledger_line(repo, tmp_path):
    set_snapshot(tmp_path, week=99.0)
    ctx, first = codex_decision(repo)
    second = budget_guard.evaluate(ctx.store, ctx.config, provider="codex")

    assert first.action == second.action == "run"
    assert ledger(ctx).count("codex roles run unguarded") == 1


def test_codex_roles_are_never_guarded_without_a_usage_archive(repo):
    assert codex_decision(repo)[1].action == "run"
