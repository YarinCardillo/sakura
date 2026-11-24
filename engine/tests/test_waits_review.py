import json
import os
from datetime import UTC, datetime

import pytest
from flow_support import make_context
from sakura.actors import Actor
from sakura.cockpit import actions, api, review_waits, snapshot, wait_actions
from sakura.flow import delivery
from test_cockpit_actions import wait_target

PR = "https://github.com/example/project/pull/42"


@pytest.fixture
def pull_request(repo, tmp_path, monkeypatch):
    task = make_context(repo, state={"title": "Fix the inbox", "status": "report_ready"}).store
    delivery.record(task, delivery.PULL_REQUEST, "a" * 40, "feature/inbox", PR)
    status = tmp_path / "pr.json"
    status.write_text('{"state":"OPEN"}')
    gh = tmp_path / "gh"
    gh.write_text(f'#!/bin/sh\ncat "{status}"\n')
    gh.chmod(0o700)
    monkeypatch.setenv("PATH", f"{tmp_path}:{os.environ['PATH']}")
    return status


def _snapshot(repo):
    return snapshot.collect([repo], datetime.now(UTC))


def test_open_pr_yields_review(repo, pull_request):
    [review] = _snapshot(repo).waits
    row = api._wait(review)
    assert (row["kind"], row["subkind"], row["title"]) == ("review", "review", "Review Fix the inbox")
    assert row["links"] == [{"label": "Open pull request", "url": PR}]


@pytest.mark.parametrize("state", ["MERGED", "CLOSED"])
def test_merged_pr_resolves_review_with_actor_github(repo, pull_request, state):
    before = _snapshot(repo)
    pull_request.write_text(json.dumps({"state": state}))
    assert len(_snapshot(repo).waits) == 1
    review_waits.Refresher(lambda: [repo]).refresh()
    after = _snapshot(repo)
    assert after.waits == []
    assert [(entry["id"], entry["actor"]) for entry in after.decided] == [(before.waits[0].id, "github")]
    assert after.decided[0]["place"] == "github"
    assert _snapshot(repo).decided == after.decided


def test_mark_reviewed_resolves(repo, pull_request):
    [review] = _snapshot(repo).waits
    result = actions.run("markreviewed", wait_actions.target_of(review), {}, Actor("app", "owner"))
    assert result.exit_code == 0
    after = _snapshot(repo)
    assert after.waits == []
    assert [(entry["answer"], entry["actor"]) for entry in after.decided] == [("reviewed", "app:owner")]


@pytest.mark.parametrize("kind", ["owner_row", "automation", "proposal"])
def test_owner_row_and_automation_map_to_approval(kind):
    row = api._wait(wait_target(kind, "item").wait)
    assert (row["kind"], row["subkind"], row["key"][3]) == ("approval", kind, kind)


@pytest.mark.parametrize("kind", ["owner_row", "question"])
def test_digest_items_map_to_proposal(repo, kind):
    from test_cockpit_digest_reading import _published

    _published()
    row = api._wait(next(wait for wait in _snapshot(repo).waits if wait.kind == kind))
    assert (row["kind"], row["subkind"]) == ("proposal", kind)


@pytest.mark.parametrize("automatic", [False, True])
def test_resolving_review_is_accepted_by_live_driver(repo, pull_request, automatic):
    from sakura.flow import store

    driver = store.TaskStore(repo, "demo")
    driver.update_state(lambda state: state.update(status="running"))
    [review] = _snapshot(repo).waits
    if automatic:
        pull_request.write_text('{"state":"MERGED"}')
        review_waits.Refresher(lambda: [repo]).refresh()
        assert _snapshot(repo).waits == []
    else:
        assert actions.run("markreviewed", wait_actions.target_of(review), {}, Actor("app", "owner")).exit_code == 0
    assert driver.read_state()["status"] == "running"


def test_snapshot_reads_reviews_without_probing_github(repo, pull_request, monkeypatch):
    def unexpected(*args):
        pytest.fail("snapshot attempted a remote lookup")

    monkeypatch.setattr(review_waits, "_pr_state", unexpected)
    [review] = _snapshot(repo).waits
    assert review.kind == "review"


def test_unknown_github_state_leaves_the_review_open(repo, pull_request):
    pull_request.write_text("{}")
    review_waits.Refresher(lambda: [repo]).refresh()
    [review] = _snapshot(repo).waits
    assert review.kind == "review"



def test_background_probes_are_bounded_and_move_past_unknown_reviews(repo, pull_request, monkeypatch):
    for task_name in ("second", "third"):
        task = make_context(repo, task=task_name).store
        delivery.record(task, delivery.PULL_REQUEST, "a" * 40, "feature/inbox", PR.replace("42", task_name))
    probes = []
    monkeypatch.setattr(review_waits, "PROBES_PER_REFRESH", 1)
    monkeypatch.setattr(review_waits, "_pr_state", lambda url, _: probes.append(url) or "")
    refresh = review_waits.Refresher(lambda: [repo])
    for count in range(1, 4):
        refresh.refresh()
        assert len(probes) == count
    assert len(set(probes)) == 3
    assert len(_snapshot(repo).waits) == 3
