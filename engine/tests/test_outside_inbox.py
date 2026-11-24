import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import actors, appendlog, questions
from sakura.outside import inbox, records, store

LIB = str(Path(__file__).resolve().parents[1] / "lib")
NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
CLAUDE = records.Agent("claude", Path("/work/work"))
PERSON = actors.Actor("overlay", "me", "iphone")
OUTSIDE = actors.Actor("outside", "claude")
LANE = "measure-help"
STEPS = ("Run measure.py --help",)
ADD_ROWS = """
import sys
sys.path.insert(0, sys.argv[1])
from datetime import datetime, timezone
from pathlib import Path
from sakura.outside import records, store
agent = records.Agent("claude", Path("/work/work"))
for number in range(30):
    store.put_row(Path(sys.argv[2]), agent, "measure-help", f"extra-{number}", title="Extra", steps=("s",),
                  expected="e", now=datetime(2026, 11, 16, tzinfo=timezone.utc))
"""


@pytest.fixture
def root(tmp_path):
    folder = tmp_path / "outside"
    store.open_lane(folder, CLAUDE, LANE, title="Add --help", now=NOW)
    return folder


def row(root, row_id="help-prints", **fields):
    given = {"title": "Help prints", "steps": STEPS, "expected": "usage", **fields}
    return store.put_row(root, CLAUDE, LANE, row_id, **given, now=NOW).lane.rows[-1]


def question(root, question_id="flag-short", choices=("yes", "no")):
    asked = questions.make(question_id, "Also accept -h?", choices, NOW)
    store.ask(root, CLAUDE, LANE, asked, NOW)
    return asked


def test_Mark_ActorOutside_Refused(root):
    current = row(root)
    with pytest.raises(store.Refused, match="only the person"):
        inbox.mark(root, OUTSIDE, LANE, current.id, current.digest, "ok", "", NOW)
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "row"]


def test_Mark_PersonThroughGateOrCli_AppendsLineWithVerdictAndTitle(root):
    current = row(root)
    line = inbox.mark(root, PERSON, LANE, current.id, current.digest, "ko", "usage line is empty", NOW)
    assert (line["kind"], line["verdict"], line["note"], line["row"], line["title"]) == (
        "mark", "ko", "usage line is empty", "help-prints", "Help prints")
    assert line["by"] == "overlay:me:iphone" and line["seq"] == 3
    cli = inbox.mark(root, actors.Actor("cli", "me"), LANE, row(root, "second").id, row(root, "second").digest, "ok",
                     "", NOW)
    assert cli["by"] == "cli:me"


def test_Mark_StaleDigest_RefusedNoLongerWaiting(root):
    old = row(root)
    row(root, note="changed by the agent")
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.mark(root, PERSON, LANE, old.id, old.digest, "ok", "", NOW)
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "row", "row"]


def test_Mark_Twice_SecondRefused(root):
    current = row(root)
    inbox.mark(root, PERSON, LANE, current.id, current.digest, "ok", "", NOW)
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.mark(root, PERSON, LANE, current.id, current.digest, "ko", "", NOW)
    assert len([line for line in store.log(root, LANE) if line["kind"] == "mark"]) == 1


@pytest.mark.parametrize("fields", [dict(verdict="maybe"), dict(note="x" * 501), dict(note="bad \x1b")])
def test_Mark_BadVerdictOrNote_Refused(root, fields):
    current = row(root)
    given = {"verdict": "ok", "note": "", **fields}
    with pytest.raises(store.Refused):
        inbox.mark(root, PERSON, LANE, current.id, current.digest, given["verdict"], given["note"], NOW)


def test_Mark_RowGoneOrLaneClosed_Refused(root):
    current = row(root)
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.mark(root, PERSON, LANE, "nope", current.digest, "ok", "", NOW)
    store.close(root, PERSON, LANE, outcome="dropped", now=NOW)
    with pytest.raises(store.Refused, match="closed"):
        inbox.mark(root, PERSON, LANE, current.id, current.digest, "ok", "", NOW)
    with pytest.raises(store.Refused, match="closed"):
        inbox.steer(root, PERSON, LANE, "hello", NOW)


def test_Reply_ChoiceOrText_AppendsLine_OtherInputsRefused(root):
    asked = question(root)
    line = inbox.reply(root, PERSON, LANE, asked.id, asked.digest, "yes", "", NOW)
    assert (line["kind"], line["choice"], line["text"], line["question"]) == ("reply", "yes", "", "flag-short")
    other = question(root, "why", choices=())
    with pytest.raises(store.Refused):
        inbox.reply(root, PERSON, LANE, other.id, other.digest, "yes", "", NOW)
    with pytest.raises(store.Refused):
        inbox.reply(root, PERSON, LANE, other.id, other.digest, "", "", NOW)
    assert inbox.reply(root, PERSON, LANE, other.id, other.digest, "", "for scripts", NOW)["text"] == "for scripts"


def test_Reply_StaleOrTwiceOrOutsideActor_Refused(root):
    asked = question(root)
    with pytest.raises(store.Refused, match="only the person"):
        inbox.reply(root, OUTSIDE, LANE, asked.id, asked.digest, "yes", "", NOW)
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.reply(root, PERSON, LANE, asked.id, "0" * 12, "yes", "", NOW)
    inbox.reply(root, PERSON, LANE, asked.id, asked.digest, "yes", "", NOW)
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.reply(root, PERSON, LANE, asked.id, asked.digest, "no", "", NOW)


def test_Steer_ActorOutsideRefused_PersonAppends(root):
    with pytest.raises(store.Refused, match="only the person"):
        inbox.steer(root, OUTSIDE, LANE, "do it", NOW)
    assert inbox.steer(root, PERSON, LANE, "use the tag, not the date", NOW)["text"] == "use the tag, not the date"
    with pytest.raises(store.Refused):
        inbox.steer(root, PERSON, LANE, "x" * 8001, NOW)


def test_Inbox_UnacknowledgedRead_SameEntriesAgain(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    inbox.steer(root, PERSON, LANE, "two", NOW)
    first = inbox.read(root, "claude", LANE, None)
    again = inbox.read(root, "claude", LANE, None)
    assert [entry["text"] for entry in first.entries] == ["one", "two"] == [e["text"] for e in again.entries]
    assert first.cursor == again.cursor == 3 and store.read(root, LANE).read_up_to == 0


def test_Inbox_AfterCursor_NeverAgain(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    cursor = inbox.read(root, "claude", LANE, None).cursor
    assert inbox.read(root, "claude", LANE, cursor).entries == []
    assert inbox.read(root, "claude", LANE, None).entries == []
    assert store.read(root, LANE).read_up_to == cursor
    inbox.steer(root, PERSON, LANE, "two", NOW)
    assert [entry["text"] for entry in inbox.read(root, "claude", LANE, cursor).entries] == ["two"]


def lane_and_log(root):
    return store.read(root, LANE), store.log(root, LANE)


def test_Inbox_DeliveredButNotAcknowledged_CountsAsReadAndIsRedelivered(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    assert inbox.unread(*lane_and_log(root)) == 1

    page = inbox.read(root, "claude", LANE, None)

    lane, log = lane_and_log(root)
    assert inbox.unread(lane, log) == 0
    assert (lane.delivered_up_to, lane.read_up_to) == (page.cursor, 0)
    assert [entry["text"] for entry in inbox.read(root, "claude", LANE, None).entries] == ["one"]


def test_Inbox_EntryAfterTheDelivery_StaysUnread(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    inbox.read(root, "claude", LANE, None)
    inbox.steer(root, PERSON, LANE, "two", NOW)

    assert inbox.unread(*lane_and_log(root)) == 1


def test_Inbox_PagedRead_DeliversOnlyWhatItReturned(root):
    for number in range(inbox.PAGE_SIZE + 1):
        inbox.steer(root, PERSON, LANE, f"m{number}", NOW)

    page = inbox.read(root, "claude", LANE, None)

    assert len(page.entries) == inbox.PAGE_SIZE
    assert inbox.unread(*lane_and_log(root)) == 1 and store.read(root, LANE).delivered_up_to == page.cursor


def test_Inbox_AcknowledgedPastDelivery_DeliveryFollows(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    inbox.read(root, "claude", LANE, 2)

    lane, log = lane_and_log(root)
    assert (lane.read_up_to, lane.delivered_up_to, inbox.unread(lane, log)) == (2, 2, 0)


def test_Inbox_AfterPastLastEntry_Refused(root):
    with pytest.raises(store.Refused, match="past"):
        inbox.read(root, "claude", LANE, 99)
    assert store.read(root, LANE).read_up_to == 0


def test_Inbox_AfterLowerThanAcknowledged_KeepsTheHigherCursor(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    inbox.read(root, "claude", LANE, 2)
    inbox.read(root, "claude", LANE, 1)
    assert store.read(root, LANE).read_up_to == 2


def test_Inbox_OtherAgentName_Refused(root):
    with pytest.raises(store.Refused, match="claude"):
        inbox.read(root, "script", LANE, None)


def test_Inbox_MoreThanFiftyEntries_PagedWithCursorOfLastReturned(root):
    for number in range(60):
        inbox.steer(root, PERSON, LANE, f"m{number}", NOW)
    page = inbox.read(root, "claude", LANE, None)
    assert len(page.entries) == 50 and page.cursor == page.entries[-1]["seq"]
    rest = inbox.read(root, "claude", LANE, page.cursor)
    assert [entry["text"] for entry in rest.entries] == [f"m{number}" for number in range(50, 60)]


def test_Inbox_EntriesOnlyThePersonsLines_AgentLinesNeverReturned(root):
    row(root)
    inbox.steer(root, PERSON, LANE, "hi", NOW)
    assert [entry["kind"] for entry in inbox.read(root, "claude", LANE, None).entries] == ["steer"]


class Clock:
    """A fake clock whose sleep moves it and can run an action at a tick."""

    def __init__(self, on_tick=None):
        self.now, self.slept, self.on_tick = 0.0, 0, on_tick

    def sleep(self, seconds):
        self.now += seconds
        self.slept += 1
        if self.on_tick:
            self.on_tick(self.slept)

    def time(self):
        return self.now


def test_Inbox_WaitEntryArrives_ReturnsBeforeDeadline(root):
    clock = Clock(lambda tick: inbox.steer(root, PERSON, LANE, "now", NOW) if tick == 3 else None)
    page = inbox.wait(root, "claude", LANE, None, 50, sleep=clock.sleep, clock=clock.time)
    assert [entry["text"] for entry in page.entries] == ["now"] and clock.now == 3


def test_Inbox_WaitDeadline_EmptyPageCursorKept(root):
    clock = Clock()
    page = inbox.wait(root, "claude", LANE, 1, 5, sleep=clock.sleep, clock=clock.time)
    assert page.entries == [] and page.cursor == 1 and 5 <= clock.now <= 6


def test_Inbox_WaitOnClosedLane_ReturnsAtOnce(root):
    store.close(root, PERSON, LANE, outcome="dropped", now=NOW)
    clock = Clock()
    page = inbox.wait(root, "claude", LANE, None, 50, sleep=clock.sleep, clock=clock.time)
    assert page.is_closed and clock.now == 0


def test_Inbox_PersonMarksWhileAgentAddsRow_BothKept(root):
    current = row(root)
    worker = subprocess.Popen([sys.executable, "-c", ADD_ROWS, LIB, str(root)])
    inbox.mark(root, PERSON, LANE, current.id, current.digest, "ok", "", NOW)
    for number in range(30):
        inbox.steer(root, PERSON, LANE, f"m{number}", NOW)
    assert worker.wait(timeout=60) == 0
    lines = appendlog.read(root / LANE / "log.jsonl")
    assert [line["seq"] for line in lines] == list(range(1, len(lines) + 1)) and len(lines) == 1 + 31 + 31
    assert len(store.read(root, LANE).rows) == 31


def test_Ask_ChangedAfterReply_RefusedNewIdNamed(root):
    asked = question(root)
    inbox.reply(root, PERSON, LANE, asked.id, asked.digest, "yes", "", NOW)
    changed = questions.make("flag-short", "Also accept --short?", ("yes", "no"), NOW)
    with pytest.raises(store.Refused, match="flag-short-2"):
        store.ask(root, CLAUDE, LANE, changed, NOW)
    assert store.ask(root, CLAUDE, LANE, asked, NOW).result == "unchanged"
    assert store.ask(root, CLAUDE, LANE, questions.make("flag-short-2", "Also --short?", (), NOW), NOW).result == "opened"


def test_Ask_ChangedBeforeReply_StillUpdates(root):
    question(root)
    changed = questions.make("flag-short", "Also accept --short?", ("yes", "no"), NOW)
    assert store.ask(root, CLAUDE, LANE, changed, NOW).result == "updated"


def test_Derived_KoRow_NotOpenNotDone(root):
    current = row(root)
    other = row(root, "second")
    asked = question(root)
    lane, log = store.read(root, LANE), store.log(root, LANE)
    assert [item.id for item in inbox.open_rows(lane, log)] == ["help-prints", "second"]
    assert [item.id for item in inbox.open_questions(lane, log)] == ["flag-short"]
    inbox.mark(root, PERSON, LANE, current.id, current.digest, "ko", "", NOW)
    inbox.mark(root, PERSON, LANE, other.id, other.digest, "ok", "", NOW)
    inbox.reply(root, PERSON, LANE, asked.id, asked.digest, "yes", "", NOW)
    lane, log = store.read(root, LANE), store.log(root, LANE)
    assert inbox.open_rows(lane, log) == [] and inbox.open_questions(lane, log) == []
    assert inbox.done_rows(lane, log) == 1


def test_Derived_RowChangedAfterMark_OpenAgainAndNotDone(root):
    current = row(root)
    inbox.mark(root, PERSON, LANE, current.id, current.digest, "ok", "", NOW)
    row(root, note="new note")
    lane, log = store.read(root, LANE), store.log(root, LANE)
    assert [item.id for item in inbox.open_rows(lane, log)] == ["help-prints"] and inbox.done_rows(lane, log) == 0


def test_Derived_Unread_CountsPersonLinesNoReadReturnedYet(root):
    inbox.steer(root, PERSON, LANE, "one", NOW)
    inbox.steer(root, PERSON, LANE, "two", NOW)
    assert inbox.unread(store.read(root, LANE), store.log(root, LANE)) == 2
    inbox.read(root, "claude", LANE, 2)
    assert inbox.unread(store.read(root, LANE), store.log(root, LANE)) == 0


def test_Derived_ClosedLane_NoOpenWaits(root):
    row(root)
    store.close(root, PERSON, LANE, outcome="dropped", now=NOW)
    lane, log = store.read(root, LANE), store.log(root, LANE)
    assert inbox.open_rows(lane, log) == [] and inbox.open_questions(lane, log) == []


def test_Revision_RowRestoredToAnEarlierContent_WaitsAgainAndTheOldOkDoesNotCount(root):
    # Review 17, finding 6: a mark binds the row's revision, which grows on every content change, so restoring an
    # earlier content through the agent API cannot bring back an Ok the person gave it.
    first = row(root)
    inbox.mark(root, PERSON, LANE, first.id, first.digest, "ok", "", NOW)
    row(root, note="changed by the agent")
    restored = row(root)

    lane, log = store.read(root, LANE), store.log(root, LANE)

    assert [item.id for item in inbox.open_rows(lane, log)] == ["help-prints"]
    assert inbox.done_rows(lane, log) == 0 and restored.digest != first.digest
    assert (first.revision, restored.revision) == (1, 3)
    with pytest.raises(store.Refused, match="no longer waiting"):
        inbox.mark(root, PERSON, LANE, first.id, first.digest, "ok", "", NOW)


def test_Revision_AnImmediateIdenticalRetry_LeavesTheRevisionAndTheOkUnchanged(root):
    first = row(root)
    inbox.mark(root, PERSON, LANE, first.id, first.digest, "ok", "", NOW)

    again = row(root)

    assert (again.revision, again.digest) == (first.revision, first.digest)
    assert inbox.open_rows(store.read(root, LANE), store.log(root, LANE)) == []
