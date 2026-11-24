"""The snapshot gives a name a person reads next to every id the app would otherwise print: preset, account and plan."""

from dataclasses import replace
from pathlib import Path

import pytest

from leeway_support import account, make_helper
from sakura.cockpit import api, lane_facts
from sakura.flow import presets
from sakura.usage import leeway
from test_cockpit_actions import flow_item
from test_cockpit_lane_facts import knobs_of, task_in
from test_cockpit_snapshot import make_snapshot


@pytest.fixture(autouse=True)
def fresh_helper_cache():
    leeway.forget()
    yield
    leeway.forget()


def _document(monkeypatch, tmp_path, rows, **facts):
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=rows)))
    item = replace(flow_item(Path("/work/repo"), "t1"), facts=facts)
    return api.snapshot_document(make_snapshot(items=[item]))


def _steps(**accounts):
    return [{"id": step, "name": step.capitalize(), "state": "next", "model": None, "account": accounts.get(step)}
            for step in ("plan", "build", "checks", "deliver")]


def test_LaneFacts_ThePresetOfALane_HasItsLabel(repo):
    task = task_in(repo)
    facts = lane_facts.lane_facts(task.root, task.read_state(), knobs_of("lean"))
    assert (facts["preset"], facts["preset_label"]) == ("lean", "Lean")


def test_Snapshot_StepAndReviewAccounts_CarryTheLabelLeewayGives(monkeypatch, tmp_path):
    rows = [account("work", label="Work account"), account("spare", label="Spare")]
    reviews = [{"id": "task_review", "on": True, "model": None, "account": "work", "state": "now"},
               {"id": "final_review", "on": False, "model": None, "account": None, "state": "skipped"}]

    document = _document(monkeypatch, tmp_path, rows, steps=_steps(build="spare", checks="work"), reviews=reviews)

    item = document["items"][0]
    assert [step["account_label"] for step in item["steps"]] == [None, "Spare", "Work account", None]
    assert [review["account_label"] for review in item["reviews"]] == ["Work account", None]


def test_Snapshot_AnAccountLeewayLabelsWithItsOwnId_ReadsAsProviderAndWord(monkeypatch, tmp_path):
    rows = [account("claude-default", label="claude-default")]

    document = _document(monkeypatch, tmp_path, rows, steps=_steps(build="claude-default"))

    assert [row["label"] for row in document["accounts"]] == ["Claude, personal"]
    assert document["items"][0]["steps"][1]["account_label"] == "Claude, personal"
    assert [row["label"] for row in document["items"][0]["pace_accounts"]] == ["Claude, personal"]


def test_Snapshot_AnAccountNoBookKnows_ReadsAsWordsToo(monkeypatch, tmp_path):
    document = _document(monkeypatch, tmp_path, [account("work")], steps=_steps(build="claude-spare"))

    assert document["items"][0]["steps"][1]["account_label"] == "Claude, spare"


def test_Snapshot_AccountPlans_AreNamedInWords(monkeypatch, tmp_path):
    row, book = account("main", provider="codex", label="Main")
    row["plan"] = "prolite"

    document = _document(monkeypatch, tmp_path, [(row, book)])

    (found,) = document["accounts"]
    assert found["plan"] == "prolite" and found["plan_label"] == "Codex Pro Lite"
    assert found["plan_choice_labels"] == {"plus": "Codex Plus", "prolite": "Codex Pro Lite", "pro": "Codex Pro"}


def test_Snapshot_UsageOfAnAccount_NamesTheAccount(monkeypatch, tmp_path):
    document = _document(monkeypatch, tmp_path, [account("claude-default", label="claude-default")])

    assert [row["account_label"] for row in document["usage"]["providers"]] == ["Claude, personal"]


def test_Snapshot_TheEnginePreset_HasItsLabel(monkeypatch, tmp_path):
    presets.choose("lean")

    document = _document(monkeypatch, tmp_path, [account("work")])

    assert (document["engine"]["preset"], document["engine"]["preset_label"]) == ("lean", "Lean")
