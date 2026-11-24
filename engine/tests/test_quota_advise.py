from pathlib import Path

import pytest

from sakura.accounts.registry import Account, Registry
from sakura.quota import advise
from sakura.quota.config import Allowed, Project, QuotaConfig

NOW = "2026-11-02T21:00:00+00:00"
WEEK_RESET = "2026-11-04T07:00:00Z"
FIVE_RESET = "2026-11-03T01:00:00Z"
APP = Path("/work/app")


def account(provider, account_id, week=20.0, next_round=None, five=5.0):
    return {"id": account_id, "label": account_id.title(), "provider": provider, "config_dir": f"/dirs/{provider}-{account_id}",
            "five_hour": {"pct": five, "resets_at": FIVE_RESET},
            "seven_day": {"pct": week, "resets_at": WEEK_RESET},
            "forecast": {"seven_day_pct_at_reset": week, "next_round_pct": week + 1 if next_round is None else next_round,
                         "exhausts_at": None, "confidence": "high"}}


def document(*accounts):
    return {"generated_at": NOW, "accounts": list(accounts), "history": {}}


LIMITS = {("claude", "work"): 95.0, ("claude", "personal"): 80.0, ("codex", "personal"): 75.0}
RULES = QuotaConfig(
    projects=(Project("work", (APP,), Allowed(("work", "personal"), ("personal",))),),
    default=Allowed(("personal",), ("personal",)))


def ask(doc, role="builder", task="integration", agent=None, repo=APP, config=RULES):
    return advise.advise(advise.Request(repo, role, task, agent), doc, LIMITS, config)


def test_Advise_RoomOnPreferredAccount_IsGoWithTheRoutedModel():
    result = ask(document(account("claude", "work"), account("claude", "personal")), role="reviewer", agent="claude")
    assert (result.verdict, result.account, result.model, result.config_dir) == ("go", "work", "sonnet", "/dirs/claude-work")


def test_Advise_WorkAccountForecastPast95_SwitchesToPersonalClaude():
    doc = document(account("claude", "work", week=94.0, next_round=96.0), account("claude", "personal", week=22.0))
    result = ask(doc, agent="claude")
    assert (result.verdict, result.account, result.config_dir) == ("switch", "personal", "/dirs/claude-personal")
    assert "over its 95% ceiling" in result.reason


def test_Advise_WorkAccountExactlyAtItsCeiling_StillGoes():
    doc = document(account("claude", "work", week=94.0, next_round=95.0))
    assert ask(doc, agent="claude").verdict == "go"


def test_Advise_PersonalProject_NeverUsesTheWorkAccount():
    doc = document(account("claude", "work"), account("claude", "personal", week=79.5, next_round=85.0))
    result = ask(doc, agent="claude", repo=Path("/home/me/side"))
    assert (result.verdict, result.account) == ("wait", "personal")


def test_Advise_PersonalProjectOnWorkAccountLimits_IsNotMixedUp():
    doc = document(account("claude", "work", next_round=10.0), account("claude", "personal"))
    assert ask(doc, agent="claude", repo=Path("/home/me/side")).account == "personal"


def test_Advise_BuilderWithoutAgent_PrefersCodex():
    doc = document(account("claude", "work"), account("codex", "personal"))
    result = ask(doc)
    assert (result.verdict, result.agent, result.model, result.account) == ("go", "codex", "", "personal")


def test_Advise_CodexOverItsCeiling_SwitchesToClaude():
    doc = document(account("codex", "personal", week=74.0, next_round=80.0), account("claude", "work"))
    result = ask(doc)
    assert (result.verdict, result.agent, result.account) == ("switch", "claude", "work")


def test_Advise_PlannerWithoutAgent_UsesClaudeOpus():
    result = ask(document(account("codex", "personal"), account("claude", "work")), role="planner")
    assert (result.agent, result.model) == ("claude", "opus")


def test_Advise_AllBlocked_WaitsUntilTheEarliestReset():
    doc = document(account("claude", "work", next_round=99.0), account("claude", "personal", next_round=90.0, five=97.0))
    result = ask(doc, agent="claude")
    assert (result.verdict, result.retry_at) == ("wait", "2026-11-03T01:00:00+00:00")


def test_Advise_BlockedWithNoResetKnown_Denies():
    work = account("claude", "work", next_round=99.0)
    work["seven_day"]["resets_at"] = None
    result = ask(document(work), agent="claude", config=QuotaConfig(default=Allowed(("work",), None)), repo=Path("/x"))
    assert (result.verdict, result.retry_at) == ("deny", None)


def test_Advise_NoAllowedAccountOnTheAgent_Denies():
    result = ask(document(account("claude", "work")), agent="codex")
    assert result.verdict == "deny" and "project work" in result.reason


def test_Advise_NoReadingYet_IsAllowedAndSaysSo():
    blank = account("claude", "work")
    blank["seven_day"] = {"pct": None, "resets_at": None}
    blank["forecast"]["next_round_pct"] = None
    result = ask(document(blank), agent="claude")
    assert result.verdict == "go" and "no reading" in result.reason


def test_Advise_FullFiveHourWindow_BlocksEvenWithWeekLeft():
    doc = document(account("claude", "work", five=97.0))
    assert ask(doc, agent="claude").verdict == "wait"


def test_Advise_NoRules_AnyAccountServesAnyRepo():
    doc = document(account("claude", "work"))
    assert ask(doc, agent="claude", config=QuotaConfig(), repo=Path("/anywhere")).verdict == "go"


def test_Ceilings_AreOneHundredMinusTheRegistryReserve():
    mk = lambda i, r: Account("claude", i, i, Path("/d"), True, r, (), ())
    assert advise.ceilings(Registry((mk("a", 5.0), mk("b", None)), False)) == {("claude", "a"): 95.0, ("claude", "b"): 75.0}
