"""The snapshot's events carry a `kind` the app renders by and plain `words` for what an outside lane's log says."""

import pytest

from sakura.cockpit import events
from sakura.outside import inbox
from test_cockpit_outside import LANE, NOW, PHONE, add_row, ask, open_lane, root  # noqa: F401  (root is a fixture)


def _lane_events(root):
    return [event for event in events.latest([], None, outside_root=root) if event["source"] == "outside"]


def test_Events_OpeningALane_IsKindOpenedWithItsTitle(root):
    open_lane(root)
    (event,) = _lane_events(root)
    assert (event["kind"], event["words"]) == ("opened", "Opened: Add --help")
    assert event["text"] == "opened Add --help by outside:claude"


def test_Events_ARowTheAgentAdded_SaysAddedWithItsTitleAndNotWhoAddedIt(root):
    open_lane(root)
    add_row(root)
    row = _lane_events(root)[-1]
    assert (row["kind"], row["words"]) == ("row", "Added: Help prints")


def test_Events_TheAgentAskingAQuestion_IsKindAsked(root):
    open_lane(root)
    ask(root)
    last = _lane_events(root)[-1]
    assert (last["kind"], last["words"]) == ("asked", "Asked a question")


def test_Events_ThePersonsOwnMessageToTheLane_IsASteerInHisWordsNotAnEngineEvent(root):
    open_lane(root)
    inbox.steer(root, PHONE, LANE, "use the tag", NOW)
    last = _lane_events(root)[-1]
    assert (last["kind"], last["words"]) == ("steer", "You wrote to the lane")
    assert last["by"] == "overlay:owner@example.com:nPHONE1"


@pytest.mark.parametrize("verdict, words", [("ok", "You marked it done: Help prints"), ("ko", "You marked it not done: Help prints")])
def test_Events_ThePersonsMark_SaysWhatHeDecided(root, verdict, words):
    open_lane(root)
    row = add_row(root)
    inbox.mark(root, PHONE, LANE, row.id, row.digest, verdict, "", NOW)
    last = _lane_events(root)[-1]
    assert (last["kind"], last["words"]) == ("mark", words)


def test_Events_ThePersonsReply_SaysYouReplied(root):
    open_lane(root)
    asked = ask(root)
    inbox.reply(root, PHONE, LANE, asked.id, asked.digest, "yes", "", NOW)
    last = _lane_events(root)[-1]
    assert (last["kind"], last["words"]) == ("reply", "You replied: Also accept -h?")


def test_Events_AFlowLedgerLine_HasItsTagAsKindAndNoWords(tmp_path, repo):
    from flow_support import make_context

    ctx = make_context(repo)
    ctx.store.append_ledger("pace", "an account moves")
    flow = [event for event in events.latest([ctx.store]) if event["source"] == "flow"]
    assert (flow[-1]["kind"], flow[-1]["words"]) == ("pace", None)


def test_Events_ALaneCommand_HasItsKindAndNoWords(tmp_path):
    from sakura.lanes import events as lane_events

    log = tmp_path / "events.jsonl"
    lane_events.append("say", "work", "alpha", "cli:me", "done", path=log)
    (event,) = events.latest([], log)
    assert (event["kind"], event["words"]) == ("say", None)
