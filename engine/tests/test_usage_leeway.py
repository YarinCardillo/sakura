"""The `leeway` helper as the engine's one reader of usage and accounts."""

import os
import stat
import subprocess
from datetime import timedelta

import pytest

from sakura.budget import guard
from sakura.cockpit import api
from sakura.cockpit.account_actions import AccountTarget
from sakura.flow import budget_guard, pace
from sakura.usage import leeway

from leeway_support import account, calls, make_helper
from test_cockpit_actions import invoke
from test_cockpit_snapshot import make_snapshot
from test_flow_engine_budget import budget_clock, ctx_for, set_snapshot  # noqa: F401  (the clock set_snapshot is dated by)

MISSING_LINE = "Usage is unavailable: the leeway helper was not found"


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


@pytest.fixture
def helpers(tmp_path):
    return tmp_path / "helpers"


def _locate(env=None, programs=None):
    return leeway.locate(env or {}, programs or {})


def test_Locate_TheNewerCompatibleCandidateWins(helpers):
    old = make_helper(helpers / "old", version="0.1.2")
    new = make_helper(helpers / "new", version="0.1.10")
    assert _locate({leeway.ENV_VARIABLE: str(old)}, {"leeway": str(new)}) == new


def test_Locate_ALargerStoreSchemaBeatsANewerVersion(helpers):
    low = make_helper(helpers / "low", version="0.9.0", store_schema=2)
    high = make_helper(helpers / "high", version="0.1.0", store_schema=3)
    assert _locate({leeway.ENV_VARIABLE: str(low)}, {"leeway": str(high)}) == high


@pytest.mark.parametrize("change", [{"status_schema": 4}, {"advice_schema": 1}, {"accounts_schema": 2}])
def test_Locate_AnIncompatibleSchemaIsSkipped(helpers, change):
    odd = make_helper(helpers / "odd", version="9.9.9", **change)
    good = make_helper(helpers / "good", version="0.1.0")
    assert _locate({leeway.ENV_VARIABLE: str(odd)}, {"leeway": str(good)}) == good


def test_Locate_ACopyOlderThanTheStoreIsSkipped(helpers):
    older = make_helper(helpers / "older", exit_version=3)
    other = make_helper(helpers / "other", version="0.0.1")
    assert _locate({leeway.ENV_VARIABLE: str(older)}, {"leeway": str(other)}) == other


def test_Locate_TheInstalledHelperIsTheLastPlace(helpers, monkeypatch):
    installed = make_helper(helpers / "app")
    monkeypatch.setattr(leeway, "INSTALLED_HELPERS", (str(installed),))
    assert _locate() == installed


def test_Locate_AFileOthersCanWrite_IsNotRun(helpers):
    loose = make_helper(helpers / "loose")
    loose.chmod(loose.stat().st_mode | stat.S_IWOTH)
    with pytest.raises(leeway.LeewayNotFound):
        _locate({leeway.ENV_VARIABLE: str(loose)})
    assert calls(loose) == []


def test_Locate_NothingFound_SaysSoWithoutATraceback():
    with pytest.raises(leeway.LeewayNotFound, match="leeway helper was not found"):
        _locate({leeway.ENV_VARIABLE: "/nonexistent/leeway"})


def test_Calls_StatusAccountsAndAdviseTalkToTheHelper(helpers, monkeypatch):
    rows = [account("main", week=61.5), account("work", provider="codex", week=10)]
    helper = make_helper(helpers, rows=rows)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    assert [row["id"] for row in leeway.status()["accounts"]] == ["main", "work"]
    assert [row["id"] for row in leeway.accounts()] == ["main", "work"]
    advice = leeway.advise("/work/repo", "builder", "integration", "main", 80, ["work", "spare"])
    assert advice["verdict"] == "go"
    assert calls(helper)[-1] == ["advise", "--repo", "/work/repo", "--role", "builder", "--task-class", "integration",
                                 "--account", "main", "--target", "80", "--candidates", "work,spare"]


def test_Advise_WithNoAccountOrTarget_LeavesThoseOptionsOut(helpers, monkeypatch):
    helper = make_helper(helpers)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    leeway.advise("/work/repo", "planner", "design", None, None, None)
    assert calls(helper)[-1] == ["advise", "--repo", "/work/repo", "--role", "planner", "--task-class", "design"]


def test_AccountAction_RenameCallsLeewayAccountsRename(helpers, monkeypatch):
    helper = make_helper(helpers, rows=[account("main")])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    rows = leeway.account_action("rename", "main", "Studio")
    assert calls(helper)[-1] == ["accounts", "rename", "main", "Studio", "--json"]
    assert rows[0]["label"] == "Studio"


def test_AccountAction_AnUnknownVerb_IsRefusedBeforeAnyCall(helpers, monkeypatch):
    helper = make_helper(helpers)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    with pytest.raises(ValueError, match="rename, hide, show"):
        leeway.account_action("format", "main")
    assert not any(call[:1] == ["accounts"] for call in calls(helper))


def test_Calls_AHelperThatPrintsGarbage_IsUnavailableAndAnAdvisorFailure(helpers, monkeypatch):
    helper = make_helper(helpers, broken=True)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    with pytest.raises(leeway.LeewayUnavailable) as raised:
        leeway.status()
    assert isinstance(raised.value, pace.AdvisorUnavailable)


def test_AccountAction_AFailedReplyThatStillCarriesABook_IsNotASuccess(helpers, monkeypatch):
    row, book = account("main")
    helper = make_helper(helpers, rows=[(row, book)], exit_calls=1)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))

    with pytest.raises(leeway.LeewayUnavailable):
        leeway.account_action("rename", "main", "Studio")


def test_Status_APayloadWithoutItsAccounts_IsUnavailableNotAKeyErrorInTheSnapshot(helpers, monkeypatch):
    helper = make_helper(helpers, status={"schema_version": 5})
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))

    document = api.snapshot_document(make_snapshot())

    assert any(line.startswith("Usage is unavailable: leeway") for line in document["problems"])


def test_Locate_InstalledHelpersNamedByTheEnvironment_ReplaceTheRealAppPath(helpers, monkeypatch):
    helper = make_helper(helpers)
    probed = []
    real_probe = leeway._version_of
    monkeypatch.setattr(leeway, "INSTALLED_HELPERS", ("/Applications/Leeway.app/Contents/Helpers/leeway",))
    monkeypatch.setattr(leeway, "_version_of", lambda candidate: probed.append(candidate) or real_probe(candidate))

    assert leeway.locate({leeway.INSTALLED_ENV: str(helper)}, {}) == helper
    with pytest.raises(leeway.LeewayNotFound):
        leeway.locate({leeway.INSTALLED_ENV: ""}, {})

    assert probed == [str(helper)]


def test_Tests_TheInstalledLeewayAppIsNeverProbedAndTheChildrenOfATestAreToldSo():
    assert leeway.INSTALLED_HELPERS == () and os.environ[leeway.INSTALLED_ENV] == ""
    with pytest.raises(leeway.LeewayNotFound):
        leeway.locate(os.environ, {})


# -- the snapshot -----------------------------------------------------------------------------------------------

def test_Snapshot_Accounts_MirrorTheLeewayAccountList(helpers, monkeypatch):
    rows = [account("main", week=61.5, five=33, forecast=88.2, label="Studio", week_resets_at="2026-11-23T00:00:00Z"),
            account("spare", provider="codex", week=5, hidden=True)]
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=rows)))
    document = api.snapshot_document(make_snapshot())
    main, spare = document["accounts"]
    assert {key: main[key] for key in ("id", "label", "provider", "plan", "week_pct", "week_resets_at", "five_hour_pct",
                                       "forecast_at_reset_pct", "exhausts_at", "stale", "hidden")} == {
        "id": "main", "label": "Studio", "provider": "claude", "plan": "max5x", "week_pct": 61.5,
        "week_resets_at": "2026-11-23T00:00:00Z", "five_hour_pct": 33, "forecast_at_reset_pct": 88.2,
        "exhausts_at": None, "stale": False, "hidden": False}
    assert spare["hidden"] is True and spare["week_pct"] is None and spare["state"] == "disabled"
    assert main["target"] == '["account","claude","main"]' and MISSING_LINE not in document["problems"]


def test_Snapshot_Usage_ComesFromLeewayOneEntryPerVisibleAccount(helpers, monkeypatch):
    rows = [account("main", week=50.0, forecast=90.0, week_resets_at="2026-11-23T00:00:00Z"),
            account("spare", provider="codex", hidden=True)]
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=rows)))
    document = api.snapshot_document(make_snapshot())["usage"]
    [entry] = document["providers"]
    assert (entry["provider"], entry["account"], entry["utilization"]) == ("claude", "main", 0.5)
    assert entry["resets_at"] == "2026-11-23T00:00:00Z" and entry["projection"]["at_reset"] == 0.9
    assert document["now"] == "2026-11-18T12:00:00Z"


def test_Snapshot_NoLeeway_SaysUsageIsUnavailableAndKeepsTheRestWorking():
    document = api.snapshot_document(make_snapshot())
    assert MISSING_LINE in document["problems"] and document["usage"]["providers"] == []


def test_Snapshot_ABrokenLeeway_IsAProblemNotAnError(helpers, monkeypatch):
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, broken=True)))
    document = api.snapshot_document(make_snapshot())
    assert any(line.startswith("Usage is unavailable: leeway") for line in document["problems"])


# -- the account controls ---------------------------------------------------------------------------------------

def test_AccountActions_RenameDisableAndEnable_GoThroughLeeway(helpers, monkeypatch):
    helper = make_helper(helpers, rows=[account("main")])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    target = AccountTarget("claude", "main")
    assert invoke("l", target, {"label": "Studio"})[0].exit_code == 0
    assert invoke("n", target)[0].exit_code == 0
    assert invoke("e", target)[0].exit_code == 0
    assert [call[:3] for call in calls(helper) if call[:1] == ["accounts"]] == [
        ["accounts", "rename", "main"], ["accounts", "hide", "main"], ["accounts", "show", "main"]]


def test_AccountActions_WithoutLeeway_StillRunTheEnginesOwnCommand():
    _, spawns = invoke("l", AccountTarget("claude", "main"), {"label": "Studio"})
    assert [part for part in spawns.calls[0][0] if not part.startswith("--actor=")][1:3] == ["accounts", "rename"]


# -- the guard and pace -----------------------------------------------------------------------------------------

def test_Guard_DefersWithLeewaysNumbersNotTheArchives(repo, tmp_path, helpers, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=[account("a", week=80.0)])))
    ctx = ctx_for(repo, account="a")
    decision = budget_guard.evaluate(ctx.store, ctx.config)
    assert decision.action == guard.PAUSE and "80%" in decision.reason


def test_Guard_AnAccountLeewayDoesNotKnow_KeepsTheArchivesNumbers(repo, tmp_path, helpers, monkeypatch):
    set_snapshot(tmp_path, week=10.0)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=[account("other", week=99.0)])))
    ctx = ctx_for(repo, account="a")
    assert budget_guard.evaluate(ctx.store, ctx.config).action == guard.RUN


def test_Guard_PaceAsksLeewaysAdvisorForTheStep(repo, helpers, monkeypatch):
    from sakura.flow import registry
    from flow_support import make_context
    registry.remember(repo)
    make_context(repo, task="busy", state={"status": "running"})
    advice = {"schema_version": 2, "verdict": "wait", "agent": "claude", "account": "main", "model": "", "effort": "",
              "config_dir": "", "reason": "over", "retry_at": "2026-11-19T06:30:00Z", "headroom_pct": None, "target_used": 90}
    helper = make_helper(helpers, rows=[account("main")], advice=advice)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    ctx = make_context(repo, task="a")
    decision = budget_guard.evaluate(ctx.store, ctx.config, role="builder")
    assert (decision.action, decision.rule) == (guard.DEFER, guard.PACE_RULE)
    assert calls(helper)[-1][:7] == ["advise", "--repo", str(repo), "--role", "builder", "--task-class", "integration"]
    assert "--target" in calls(helper)[-1] and "90" in calls(helper)[-1]


def test_Guard_NoLeeway_PaceSaysNothing(repo):
    from flow_support import make_context
    ctx = make_context(repo, task="a")
    assert budget_guard.evaluate(ctx.store, ctx.config, role="builder").action == guard.RUN


# -- sakura quota ----------------------------------------------------------------------------------------------

def test_QuotaCommand_StatusAndAdvise_DelegateToLeewayAndSaySo(helpers, monkeypatch, capsys, tmp_path):
    from sakura.quota import cli as quota_cli
    helper = make_helper(helpers, rows=[account("main", week=61.5)])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    assert quota_cli.main(["status", "--json"], home=tmp_path) == 0
    captured = capsys.readouterr()
    assert '"schema_version": 5' in captured.out
    assert "Leeway" in captured.err and str(helper) in captured.err
    assert quota_cli.main(["advise", "--repo", str(tmp_path), "--role", "builder", "--task-class", "design"],
                          home=tmp_path) == 0
    assert calls(helper)[-1] == ["advise", "--repo", str(tmp_path), "--role", "builder", "--task-class", "design"]


def test_QuotaCommand_WithoutLeeway_KeepsItsOwnReader(tmp_path, capsys):
    from sakura.quota import cli as quota_cli
    quota_cli.main(["status", "--json"], home=tmp_path, run=lambda *a, **k: None)
    assert "Leeway" not in capsys.readouterr().err


def test_ProgramsDoc_NamesTheLeewayEntry():
    from pathlib import Path
    doc = (Path(__file__).resolve().parents[1] / "docs" / "master.md").read_text()
    assert "leeway" in doc and "SAKURA_LEEWAY" in doc


def test_ApiDoc_DescribesTheAccountsAndUsageFromLeeway():
    from pathlib import Path
    doc = (Path(__file__).resolve().parents[1] / "docs" / "cockpit-api.md").read_text()
    for word in ("forecast_at_reset_pct", "hidden", "Usage is unavailable", "leeway accounts rename"):
        assert word in doc, word


# -- arguments that look like options -----------------------------------------------------------------------------

@pytest.mark.parametrize(("verb", "account_id", "value"), [
    ("rename", "main", "--json"), ("rename", "main", "-x"), ("hide", "--help", None), ("rename", "-main", "Studio")])
def test_AccountAction_AnArgumentStartingWithADash_IsRefusedAndNeverReachesTheHelper(helpers, monkeypatch, verb, account_id, value):
    helper = make_helper(helpers, rows=[account("main")])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    with pytest.raises(ValueError, match="cannot start with a dash"):
        leeway.account_action(verb, account_id, value)
    assert not any(call[:1] == ["accounts"] for call in calls(helper))


def test_Advise_AnAccountOrCandidateStartingWithADash_IsRefused(helpers, monkeypatch):
    helper = make_helper(helpers)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    for account_name, candidates in (("--target", None), ("main", ["ok", "-x"])):
        with pytest.raises(ValueError, match="cannot start with a dash"):
            leeway.advise("/work/repo", "builder", "design", account_name, None, candidates)
    assert not any(call[:1] == ["advise"] for call in calls(helper))


def test_AccountAction_ARenameFromTheCockpitWithADash_IsAReasonTheClientSees(helpers, monkeypatch):
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=[account("main")])))
    result, _ = invoke("l", AccountTarget("claude", "main"), {"label": "--json"})
    assert result.exit_code != 0 and "cannot start with a dash" in result.output


# -- nested fields the consumers read (review finding 10, second round) -------------------------------------------

def _view_of(helpers, monkeypatch, change_row=lambda row: None, change_book=lambda entry: None):
    """The cockpit's view of one account `main` whose status row and book row the two callbacks change."""
    from sakura.cockpit import leeway_view
    row, entry = account("main")
    status = {"schema_version": 5, "generated_at": "2026-11-18T12:00:00Z", "poller": "none", "accounts": [row], "history": {}}
    change_row(row)
    change_book(entry)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(helpers, rows=[(row, entry)], status=status)))
    return leeway_view.read(lambda target: [])


def _empty_windows(row):
    row["five_hour"], row["seven_day"] = {}, {}
    del row["forecast"]


def _set(name, key, value):
    def change(row):
        row[name] = value if key is None else {**row[name], key: value}
    return change


def _drop(name):
    return lambda row: row.pop(name)


MALFORMED_ROWS = {
    "the-reviews-reply": _empty_windows,
    "week-pct-text": _set("seven_day", "pct", "40"),
    "five-pct-bool": _set("five_hour", "pct", True),
    "resets-number": _set("seven_day", "resets_at", 7),
    "five-resets-missing": lambda row: row["five_hour"].pop("resets_at"),
    "forecast-pct-text": _set("forecast", "seven_day_pct_at_reset", "high"),
    "exhausts-number": _set("forecast", "exhausts_at", 5),
    "forecast-null": _set("forecast", None, None),
    "forecast-list": _set("forecast", None, []),
    "forecast-missing": _drop("forecast"),
    "as-of-missing": _drop("as_of"),
    "as-of-number": _set("as_of", None, 12),
    "config-dir-null": _set("config_dir", None, None),
    "stale-text": _set("stale", None, "no"),
}


@pytest.mark.parametrize("change", MALFORMED_ROWS.values(), ids=MALFORMED_ROWS.keys())
def test_Status_AMalformedNestedField_IsUnavailableAndNeverReachesAConsumer(helpers, monkeypatch, change):
    view = _view_of(helpers, monkeypatch, change_row=change)
    assert not view.is_available
    assert view.problem.endswith("leeway status answered in a form the engine does not read")


def test_Status_AnAccountWithNoReadingYet_IsStillAccepted(helpers, monkeypatch):
    def unread(row):
        row["five_hour"] = {"pct": None, "resets_at": None}
        row["seven_day"] = {"pct": None, "resets_at": None}
        row["forecast"] = {"seven_day_pct_at_reset": None, "exhausts_at": None}
        row["as_of"] = None
    assert _view_of(helpers, monkeypatch, change_row=unread).is_available


@pytest.mark.parametrize("change", [lambda entry: entry.update(hidden="no"), lambda entry: entry.update(hidden=None),
                                    lambda entry: entry.update(hidden=1), lambda entry: entry.pop("hidden"),
                                    lambda entry: entry.update(folder_exists="yes"), lambda entry: entry.update(label=3)],
                         ids=["hidden-text", "hidden-null", "hidden-number", "hidden-missing", "folder-text", "label-number"])
def test_Accounts_AMalformedBookRow_IsUnavailableNotAKeyErrorInTheView(helpers, monkeypatch, change):
    view = _view_of(helpers, monkeypatch, change_book=change)
    assert view.problem.endswith("leeway accounts answered in a form the engine does not read")


# Keeping the readings fresh: a stale reading asks the helper for one refresh before advice.

GENERATED_AT = "2026-11-18T12:00:00Z"


def _helper_with_age(helpers, monkeypatch, age_minutes, **options):
    """A fake helper whose one account was read `age_minutes` before the status document was made."""
    as_of = (leeway._moment(GENERATED_AT) - timedelta(minutes=age_minutes)).isoformat()
    row, book = account("main", as_of=as_of)
    status = {"schema_version": 5, "generated_at": GENERATED_AT, "poller": "none", "accounts": [row], "history": {}}
    helper = make_helper(helpers, rows=[(row, book)], status=status, **options)
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    return helper


def _verbs(helper):
    return [call[0] for call in calls(helper)]


def test_Advise_ReadingsOlderThanThePollingInterval_AsksForOneRefreshFirst(helpers, monkeypatch):
    helper = _helper_with_age(helpers, monkeypatch, age_minutes=40)
    leeway.advise("/work/repo", "builder", "integration", None, None, None)
    assert [verb for verb in _verbs(helper) if verb != "version"] == ["status", "refresh", "advise"]


def test_Advise_FreshReadings_AsksForNoRefresh(helpers, monkeypatch):
    helper = _helper_with_age(helpers, monkeypatch, age_minutes=3)
    leeway.advise("/work/repo", "builder", "integration", None, None, None)
    assert "refresh" not in _verbs(helper)


def test_Advise_StaleReadingsAskedAMomentAgo_AreNotRefreshedAgain(helpers, monkeypatch):
    helper = _helper_with_age(helpers, monkeypatch, age_minutes=40)
    for _ in range(3):
        leeway.advise("/work/repo", "builder", "integration", None, None, None)
        leeway._answers.clear()
    assert _verbs(helper).count("refresh") == 1


def test_Advise_ASlowRefresh_IsGivenUpOnAndTheAdviceStillAnswers(helpers, monkeypatch):
    helper = _helper_with_age(helpers, monkeypatch, age_minutes=40)
    run = leeway.subprocess.run
    deadlines = []

    def transport(argv, **kwargs):
        if argv[1] == "refresh":
            deadlines.append(kwargs["timeout"])
            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
        return run(argv, **kwargs)

    monkeypatch.setattr(leeway.subprocess, "run", transport)
    advice = leeway.advise("/work/repo", "builder", "integration", None, None, None)
    assert deadlines == [leeway.REFRESH_WAIT_SECONDS]
    assert advice["verdict"] == "go" and _verbs(helper)[-1] == "advise"


def test_Advise_ARefreshThatFails_UsesWhatIsThere(helpers, monkeypatch):
    helper = _helper_with_age(helpers, monkeypatch, age_minutes=40, exit_calls=0)
    monkeypatch.setattr(leeway, "_run_json", _fail_on_refresh(leeway._run_json))
    assert leeway.advise("/work/repo", "builder", "integration", None, None, None)["verdict"] == "go"
    assert _verbs(helper)[-1] == "advise"


def _fail_on_refresh(run_json):
    def run(words, *args, **options):
        if words[0] == "refresh":
            raise leeway.LeewayUnavailable("leeway refresh did not answer")
        return run_json(words, *args, **options)
    return run


def test_NewestReading_IsTheLatestAsOfOfTheAccounts(helpers, monkeypatch):
    older, newer = (account("a", as_of="2026-11-18T10:00:00Z")[0], account("b", as_of="2026-11-18T11:30:00Z")[0])
    assert leeway.newest_reading({"accounts": [older, newer]}) == "2026-11-18T11:30:00Z"
    assert leeway.newest_reading({"accounts": [dict(older, as_of=None)]}) is None


def test_Snapshot_UsageSaysHowOldTheReadingsAre(helpers, monkeypatch):
    _helper_with_age(helpers, monkeypatch, age_minutes=40)
    usage = api.snapshot_document(make_snapshot())["usage"]
    assert usage["as_of"] == "2026-11-18T11:20:00+00:00"
