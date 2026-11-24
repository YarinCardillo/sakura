"""What a lane delivered: one record for every kind of delivery, read the same way everywhere."""

import pytest

from flow_support import make_context
from sakura.flow import delivery

HEAD = "a" * 40
BASE = "b" * 40


@pytest.mark.parametrize(("url", "expected"), [("/srv/remote.git", "local"), ("file:///srv/remote.git", "local"),
                                                ("https://example.test/o/r.git", "push"), ("git@example.test:o/r.git", "push")])
def test_KindOfPush_LocalFolderOrFileUrlIsLocalElseAPush(url, expected):
    assert delivery.kind_of_push(url) == expected


def test_Record_WritesDeliveryAndThePrRecordTogether(repo):
    ctx = make_context(repo)
    delivery.record(ctx.store, delivery.PUSH, HEAD, "feat/x", note="branch feat/x pushed to a host")
    state = ctx.store.read_state()
    moment = state[delivery.KEY].pop("at")  # requirement changed (round 6, N20): a delivery says when it first happened
    assert moment and state[delivery.KEY] == {"kind": "push", "head": HEAD, "branch": "feat/x", "url": ""}
    assert state["pr"] == {"url": "", "head": HEAD, "note": "branch feat/x pushed to a host"}


def test_Record_UnknownKind_Refused(repo):
    with pytest.raises(ValueError):
        delivery.record(make_context(repo).store, "carrier-pigeon", HEAD, "feat/x")


@pytest.mark.parametrize(("state", "expected"), [
    ({"delivery": {"kind": "local", "head": HEAD, "branch": "feat/x", "url": ""}}, "local"),
    ({"pr": {"url": "https://example.test/pull/3", "head": HEAD}, "branch": "feat/x"}, "pull_request"),
    ({"pr": {"url": "", "head": HEAD, "note": "pushed"}, "branch": "feat/x"}, "push"),
])
def test_Of_ReadsTheNewRecordAndTheOlderPrRecord(state, expected):
    assert delivery.of(state)["kind"] == expected and delivery.head_of(state) == HEAD


@pytest.mark.parametrize("state", [{}, {"pr": {}}, {"pr": {"url": "", "head": BASE, "note": "nothing to change"}, "base_commit": BASE},
                                   {"pr": {"head": HEAD}}])
def test_Of_NothingDelivered_IsNone(state):
    assert delivery.of(state) is None and delivery.head_of(state) == ""


def test_Describe_NamesThePullRequestOrTheBranchAndHead():
    assert delivery.describe({"kind": "pull_request", "url": "https://example.test/pull/3"}) == "pull request https://example.test/pull/3"
    line = delivery.describe({"kind": "local", "url": "", "branch": "feat/x", "head": HEAD})
    assert "feat/x" in line and HEAD[:12] in line and "this machine" in line
