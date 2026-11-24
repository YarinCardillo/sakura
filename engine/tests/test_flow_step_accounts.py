"""Each step runs on the account its preset names: the launcher's environment, the guard's numbers, a missing account."""

import pytest

from sakura.budget import guard
from sakura.flow import budget_guard, pace_accounts, roles, runner, step_accounts
from sakura.usage import leeway

from flow_support import make_context
from leeway_support import account, make_helper
from test_flow_engine_budget import budget_clock, set_codex_source, set_snapshot  # noqa: F401  (the clock set_snapshot is dated by)

BASE_ENV = {"PATH": "/usr/bin", "KEEP": "yes"}


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


def _two_accounts(tmp_path, monkeypatch, work_week=40.0, work_hidden=False, **rows):
    main_row, main_book = account("main", week=10.0)
    work_row, work_book = account("work", week=work_week, hidden=work_hidden)
    main_row["config_dir"], work_row["config_dir"] = str(tmp_path / "main-dir"), str(tmp_path / "work-dir")
    helper = make_helper(tmp_path / "helper", rows=[(main_row, main_book), (work_row, work_book)])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))


def _ctx(repo, task="a", **accounts):
    return make_context(repo, task=task, overrides={"step_accounts": accounts} if accounts else None)


def _env(ctx, role, provider="claude"):
    return step_accounts.apply(dict(BASE_ENV), ctx.config.values, ctx.store.read_state(), role, provider)


def test_Apply_BuildPinnedToWork_LaunchesTheBuilderOnWorksFolderAndThePlannerOnTheProjects(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    ctx = _ctx(repo, build="work")
    assert _env(ctx, "builder")["CLAUDE_CONFIG_DIR"] == str(tmp_path / "work-dir")
    assert _env(ctx, "planner") == BASE_ENV


def test_Apply_WithOneAccount_EveryRunUsesItWhateverThePipelineSays(repo, tmp_path, monkeypatch):
    row, book = account("main")
    row["config_dir"] = str(tmp_path / "main-dir")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(row, book)])))
    ctx = _ctx(repo, build="work")
    assert _env(ctx, "builder")["CLAUDE_CONFIG_DIR"] == str(tmp_path / "main-dir")
    assert _env(ctx, "planner") == BASE_ENV


def test_Apply_ACodexReviewerPinnedToACodexAccount_GetsCodexHome(repo, tmp_path, monkeypatch):
    main_row, main_book = account("main")
    codex_row, codex_book = account("cx", provider="codex")
    codex_row["config_dir"] = str(tmp_path / "codex-dir")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(main_row, main_book), (codex_row, codex_book)])))
    ctx = _ctx(repo, task_review="cx")
    assert _env(ctx, "code_reviewer", "codex")["CODEX_HOME"] == str(tmp_path / "codex-dir")


def test_Apply_AnAccountOfAnotherProvider_AsksInsteadOfRunning(repo, tmp_path, monkeypatch):
    main_row, main_book = account("main")
    codex_row, codex_book = account("cx", provider="codex")
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(main_row, main_book), (codex_row, codex_book)])))
    with pytest.raises(step_accounts.StepAccountProblem, match="cx is a codex account"):
        _env(_ctx(repo, build="cx"), "builder", "claude")


def test_Apply_AHiddenOrMissingAccount_AsksInsteadOfUsingAnother(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch, work_hidden=True)
    for pinned in ("work", "gone"):
        with pytest.raises(step_accounts.StepAccountProblem, match=f"account {pinned} is hidden or missing"):
            _env(_ctx(repo, task=pinned, build=pinned), "builder")


def test_Apply_PaceMovedStep_UsesTheAccountPaceChose(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    ctx = _ctx(repo)
    ctx.store.update_state(lambda state: state.setdefault("budget", {}).update(pace_account={"build": "work"}))
    assert _env(ctx, "builder")["CLAUDE_CONFIG_DIR"] == str(tmp_path / "work-dir")


def test_Apply_APinnedStepWhenLeewayIsUnavailable_Asks(repo, monkeypatch):
    monkeypatch.setenv(leeway.ENV_VARIABLE, "/nonexistent/leeway")
    with pytest.raises(step_accounts.StepAccountProblem, match="cannot be checked"):
        _env(_ctx(repo, build="work"), "builder")
    assert _env(_ctx(repo, task="b"), "builder") == BASE_ENV


def test_Runner_PointsAPinnedStepAtItsAccountAndRefusesWhenItCannot(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    ctx = _ctx(repo, build="work")
    launcher = runner.AgentRunner(ctx.store, ctx.config)
    request = runner.RunRequest("builder", "build", "t", {}, repo)
    env = launcher._with_budget_account(dict(BASE_ENV), ctx.config, roles.parse_role("claude:sonnet"), request)
    assert env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "work-dir")
    hidden = _ctx(repo, task="b", build="gone")
    with pytest.raises(roles.RoleRefused, match="hidden or missing"):
        runner.AgentRunner(hidden.store, hidden.config)._with_budget_account(
            dict(BASE_ENV), hidden.config, roles.parse_role("claude:sonnet"), request)
    assert "hidden or missing" in hidden.store.ledger.read_text()


def test_Runner_TheFinalReviewRunsOnItsOwnStepsAccountNotTheTaskReviews(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch)
    ctx = _ctx(repo, task_review="main", final_review="work")
    launcher = runner.AgentRunner(ctx.store, ctx.config)
    spec = roles.parse_role("claude:sonnet")
    task_request = runner.RunRequest("code_reviewer", "review", "t", {}, repo)
    final_request = runner.RunRequest("code_reviewer", "review", "t", {}, repo, step="final_review")
    task_env = launcher._with_budget_account(dict(BASE_ENV), ctx.config, spec, task_request)
    final_env = launcher._with_budget_account(dict(BASE_ENV), ctx.config, spec, final_request)
    assert task_env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "main-dir")
    assert final_env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "work-dir")


def test_Guard_TheFinalReviewIsJudgedOnItsOwnStepsAccount(repo, tmp_path, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    _two_accounts(tmp_path, monkeypatch, work_week=80.0)
    ctx = make_context(repo, task="a", overrides={"budget": {"account": "a"},
                                                  "step_accounts": {"task_review": "main", "final_review": "work"}})
    assert budget_guard.evaluate(ctx.store, ctx.config, None, role="code_reviewer").action == guard.RUN
    decision = budget_guard.evaluate(ctx.store, ctx.config, None, role="code_reviewer", step="final_review")
    assert decision.action == guard.PAUSE and "80%" in decision.reason


def test_Guard_DefersOnTheStepsOwnAccountNumbers(repo, tmp_path, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    _two_accounts(tmp_path, monkeypatch, work_week=80.0)
    ctx = make_context(repo, task="a", overrides={"budget": {"account": "a"}, "step_accounts": {"build": "work"}})
    decision = budget_guard.evaluate(ctx.store, ctx.config, None, role="builder")
    assert decision.action == guard.PAUSE and "80%" in decision.reason
    assert budget_guard.evaluate(ctx.store, ctx.config, None, role="planner").action == guard.RUN


def test_Guard_AHiddenAccount_AsksThePersonWithTheAccountRule(repo, tmp_path, monkeypatch):
    _two_accounts(tmp_path, monkeypatch, work_hidden=True)
    ctx = _ctx(repo, build="work")
    decision = budget_guard.evaluate(ctx.store, ctx.config, None, role="builder")
    assert (decision.action, decision.rule) == (guard.ASK, step_accounts.ACCOUNT_RULE)
    assert "hidden or missing" in decision.reason
    assert budget_guard.evaluate(ctx.store, ctx.config, None, role="planner").action == guard.RUN


def test_AccountOf_ReadsThePipelineStepOfARole(repo):
    ctx = _ctx(repo, build="work", task_review="cx")
    values, state = ctx.config.values, ctx.store.read_state()
    assert [step_accounts.account_of(values, state, role) for role in ("builder", "code_reviewer", "planner", "reporter")] == [
        "work", "cx", "project", "project"]


def test_Guard_ACodexStepPinnedToAnotherAccount_IsJudgedOnThePinnedAccountsNumbers(repo, tmp_path, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    set_codex_source(tmp_path, week=10.0)
    work_row, work_book = account("work", provider="codex", week=99.0)
    other_row, other_book = account("other", provider="codex", week=5.0)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=[(work_row, work_book), (other_row, other_book)])))
    ctx = make_context(repo, overrides={"budget": {"account": "a"}, "roles": {"code_reviewer": "codex:gpt"},
                                        "step_accounts": {"task_review": "work"}})

    decision = budget_guard.evaluate(ctx.store, ctx.config, None, "codex", role="code_reviewer", advisor=None)

    assert decision.action == guard.PAUSE and "99%" in decision.reason


# One account resolved once: the folder the launcher points the agent at and the numbers and reserve the guard judges
# must belong to the same account, whichever path chose it.

from test_budget_account_reserve import configure as configure_registry  # noqa: E402


def _registry_and_leeway(tmp_path, monkeypatch, listed):
    """accounts.toml with `proj` (the flow's budget account, reserve 40) and `main` (reserve 20), and a Leeway helper
    listing `listed`, a map of account id to its weekly percentage."""
    configure_registry(tmp_path, {("claude", "proj"): (40, 10.0), ("claude", "main"): (20, 10.0), ("claude", "work"): (20, 10.0)})
    rows = []
    for name, week in listed.items():
        row, book = account(name, week=week)
        row["config_dir"] = f"/{name}-dir"
        rows.append((row, book))
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=rows)))


def _judged_and_launched(repo, accounts, listed, tmp_path, monkeypatch, moved=None):
    _registry_and_leeway(tmp_path, monkeypatch, listed)
    overrides = {"budget": {"account": "proj"}, "step_accounts": accounts, "knobs": {"lane_pace": "fast"}}
    ctx = make_context(repo, task="a", overrides=overrides)
    if moved:
        pace_accounts.set_allowed(repo, [moved], None)
        ctx.store.update_state(lambda state: state.setdefault("budget", {}).update(pace_account={"build": moved}))
    decision = budget_guard.evaluate(ctx.store, ctx.config, None, role="builder")
    return decision, _env(ctx, "builder").get("CLAUDE_CONFIG_DIR")


def test_Guard_TheOnlyAccountIsJudgedOnItsOwnNumbersWhileTheLauncherUsesItsFolder(repo, tmp_path, monkeypatch):
    decision, folder = _judged_and_launched(repo, {"build": "gone"}, {"main": 99.0}, tmp_path, monkeypatch)
    assert folder == "/main-dir"
    assert decision.action == guard.PAUSE and "99%" in decision.reason


def test_Guard_TheOnlyAccountKeepsItsOwnReserveNotTheProjectAccounts(repo, tmp_path, monkeypatch):
    decision, folder = _judged_and_launched(repo, {"build": "gone"}, {"main": 70.0}, tmp_path, monkeypatch)
    assert folder == "/main-dir"
    assert decision.action == guard.RUN


def test_Guard_APinnedAccountIsJudgedOnItsOwnNumbersAndReserveAndLaunchedOnItsFolder(repo, tmp_path, monkeypatch):
    listed = {"main": 10.0, "work": 99.0}
    decision, folder = _judged_and_launched(repo, {"build": "work"}, listed, tmp_path, monkeypatch)
    assert folder == "/work-dir"
    assert decision.action == guard.PAUSE and "99%" in decision.reason and "reserve 20%" in decision.reason


def test_Guard_AnAccountPaceMovedTheStepToIsJudgedAndLaunchedOnTheSameAccount(repo, tmp_path, monkeypatch):
    listed = {"main": 10.0, "work": 99.0}
    decision, folder = _judged_and_launched(repo, {}, listed, tmp_path, monkeypatch, moved="work")
    assert folder == "/work-dir"
    assert decision.action == guard.PAUSE and "99%" in decision.reason and "reserve 20%" in decision.reason


def test_Guard_TheProjectsAccountIsJudgedOnItsOwnNumbersAndTheLauncherLeavesTheEnvironmentAlone(repo, tmp_path, monkeypatch):
    listed = {"proj": 99.0, "main": 10.0}
    decision, folder = _judged_and_launched(repo, {}, listed, tmp_path, monkeypatch)
    assert folder is None
    assert decision.action == guard.PAUSE and "99%" in decision.reason and "reserve 40%" in decision.reason
