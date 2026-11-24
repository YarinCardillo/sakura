"""The names a person reads for an account and a plan: Leeway's label when it gave one, else words made from the ids."""

import pytest

from sakura.accounts import display


@pytest.mark.parametrize(("account_id", "label", "provider", "expected"), [
    ("claude-default", "claude-default", "claude", "Claude, personal"),
    ("claude-default", "", "", "Claude, personal"),
    ("default", "default", "claude", "Claude, personal"),
    ("default", "", "codex", "Codex, personal"),
    ("work", "work", "claude", "Claude, work"),
    ("codex-team-b", "", "", "Codex, team b"),
    ("team-b", "", "", "Team b"),
    ("work", "Work account", "claude", "Work account"),
    ("personal", "Personal", "claude", "Personal"),
])
def test_AccountLabel_LeewaysLabelWhenItIsOne_ElseAReadableProviderAndName(account_id, label, provider, expected):
    assert display.account_label(account_id, label, provider) == expected


@pytest.mark.parametrize("account_id", ["claude-default", "default", "codex-default"])
def test_AccountLabel_TheIdOfThePersonalAccount_NeverSaysDefault(account_id):
    assert "default" not in display.account_label(account_id, account_id).lower()


@pytest.mark.parametrize(("provider", "plan", "expected"), [
    ("codex", "prolite", "Codex Pro Lite"), ("codex", "plus", "Codex Plus"), ("codex", "pro", "Codex Pro"),
    ("claude", "pro", "Claude Pro"), ("claude", "max5x", "Claude Max 5x"), ("claude", "max20x", "Claude Max 20x"),
    ("codex", "team_plus", "Codex Team plus"), ("claude", None, ""), ("claude", "", "")])
def test_PlanLabel_NamesTheProviderAndThePlanInWords(provider, plan, expected):
    assert display.plan_label(provider, plan) == expected
