import json
import os
import stat
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import actors, appendlog, questions
from sakura.outside import records, store

LIB = str(Path(__file__).resolve().parents[1] / "lib")
NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
CLAUDE = records.Agent("claude", Path("/work/work"))
SCRIPT = records.Agent("script", Path("/work/other"))
STEPS = ("Run measure.py --help", "Read the usage line")
OPEN_TWICE = """
import sys
sys.path.insert(0, sys.argv[1])
from datetime import datetime, timezone
from pathlib import Path
from sakura.outside import records, store
try:
    change = store.open_lane(Path(sys.argv[2]), records.Agent(sys.argv[3], Path("/w")), sys.argv[3], title="T",
                             now=datetime(2026, 11, 16, tzinfo=timezone.utc))
    print(change.result)
except store.Refused as error:
    print("refused", error)
"""


@pytest.fixture
def root(tmp_path):
    return tmp_path / "outside"


def open_lane(root, agent=CLAUDE, lane_id="measure-help", now=NOW, **fields):
    return store.open_lane(root, agent, lane_id, **{"title": "Add --help", **fields}, now=now)


def put_row(root, agent=CLAUDE, lane_id="measure-help", row_id="help-prints", now=NOW, **fields):
    given = {"title": "Help prints", "steps": STEPS, "expected": "usage: measure", **fields}
    return store.put_row(root, agent, lane_id, row_id, **given, now=now)


def test_Open_SameAgentSameFields_UnchangedOneLane(root):
    assert open_lane(root).result == "opened"
    again = open_lane(root, now=NOW + timedelta(minutes=5))
    assert again.result == "unchanged" and not again.is_new
    assert [lane.id for lane in store.lanes(root)] == ["measure-help"]
    assert [line["kind"] for line in store.log(root, "measure-help")] == ["opened"]


def test_Open_ChangedTitle_UpdatedWithLogLine(root):
    open_lane(root)
    assert open_lane(root, title="Add help").result == "updated"
    assert store.read(root, "measure-help").title == "Add help"
    assert [line["kind"] for line in store.log(root, "measure-help")] == ["opened", "changed"]


def test_Open_IdOfAnotherAgent_RefusedNamingIt(root):
    open_lane(root)
    with pytest.raises(store.Refused, match="claude"):
        open_lane(root, agent=SCRIPT)


def test_Open_TwoProcessesForLastFreeLane_ExactlyOneOpened(root):
    for number in range(store.MAX_OPEN_LANES - 1):
        open_lane(root, lane_id=f"lane-{number}")
    workers = [subprocess.Popen([sys.executable, "-c", OPEN_TWICE, LIB, str(root), name], stdout=subprocess.PIPE,
                                text=True) for name in ("one", "two")]
    answers = sorted(worker.communicate(timeout=60)[0].split()[0] for worker in workers)
    assert answers == ["opened", "refused"]
    assert len(store.lanes(root)) == store.MAX_OPEN_LANES


def test_Open_OverTheCap_RefusedNamingCapAndCloseVerb(root):
    for number in range(store.MAX_OPEN_LANES):
        open_lane(root, lane_id=f"lane-{number}")
    with pytest.raises(store.Refused, match="50.*sakura outside close"):
        open_lane(root, lane_id="one-more")


def test_Open_ClosedNinetyOneDaysAgo_Removed(root):
    open_lane(root, lane_id="old")
    store.close(root, actors.Actor("outside", "claude"), "old", outcome="done", now=NOW)
    open_lane(root, lane_id="fresh", now=NOW + timedelta(days=91))
    assert store.lanes(root, closed=True) == [] and not (root / "old").exists()


def test_Open_ClosedEightyNineDaysAgo_Kept(root):
    open_lane(root, lane_id="old")
    store.close(root, actors.Actor("outside", "claude"), "old", outcome="done", now=NOW)
    open_lane(root, lane_id="fresh", now=NOW + timedelta(days=89))
    assert [lane.id for lane in store.lanes(root, closed=True)] == ["old"]


def test_Open_IdOfClosedLane_RefusedNotReused(root):
    open_lane(root)
    store.close(root, actors.Actor("outside", "claude"), "measure-help", outcome="done", now=NOW)
    with pytest.raises(store.Refused, match="closed"):
        open_lane(root)


@pytest.mark.parametrize("fields", [dict(title=""), dict(title="x" * 201), dict(title="bad \x1b"),
                                    dict(about="x" * 2001), dict(phase="merged"),
                                    dict(links=(("a", "https://x"),) * 5)])
def test_Open_FieldOverLimit_Refused(root, fields):
    with pytest.raises(store.Refused):
        open_lane(root, **fields)
    assert not root.exists()


@pytest.mark.parametrize("lane_id", ["Bad", "-x", "a" * 41, "a/b", "..", ""])
def test_Open_BadLaneId_Refused(root, lane_id):
    with pytest.raises(store.Refused):
        open_lane(root, lane_id=lane_id)


def test_Row_SameContent_UnchangedNoLogLine(root):
    open_lane(root)
    assert put_row(root).result == "opened"
    assert put_row(root, now=NOW + timedelta(hours=1)).result == "unchanged"
    assert [line["kind"] for line in store.log(root, "measure-help")] == ["opened", "row"]


def test_Row_ChangedNote_NewDigestAndSince(root):
    open_lane(root)
    first = put_row(root).lane.rows[0]
    later = NOW + timedelta(hours=1)
    changed = put_row(root, note="check on a clean clone", now=later)
    row = changed.lane.rows[0]
    assert changed.result == "updated" and row.digest != first.digest and row.since > first.since
    assert len(changed.lane.rows) == 1 and len(first.digest) == 12


def test_Row_FromAnotherAgent_Refused(root):
    open_lane(root)
    with pytest.raises(store.Refused, match="claude"):
        put_row(root, agent=SCRIPT)


def test_Row_StepsOutOfRange_Refused(root):
    open_lane(root)
    for steps in ((), tuple(f"s{number}" for number in range(21)), ("x" * 301,)):
        with pytest.raises(store.Refused):
            put_row(root, steps=steps)


def test_Row_FiftyOneDistinct_RefusedNamingFifty(root):
    open_lane(root)
    for number in range(store.MAX_ROWS):
        put_row(root, row_id=f"row-{number}")
    with pytest.raises(store.Refused, match="50"):
        put_row(root, row_id="one-more")


def test_Row_OnClosedLane_Refused(root):
    open_lane(root)
    store.close(root, actors.Actor("outside", "claude"), "measure-help", outcome="done", now=NOW)
    with pytest.raises(store.Refused, match="closed"):
        put_row(root)


@pytest.mark.parametrize("url", ["javascript:alert(1)", "http://example.com", "https://", "https://a b.com",
                                 "https://x.com/" + "a" * 500, "//x.com", "HTTPS://x.com\n"])
def test_Link_NotHttps_Refused(root, url):
    with pytest.raises(store.Refused):
        open_lane(root, links=(("PR", url),))


def test_Link_Https_StoredWithLabel(root):
    lane = open_lane(root, links=(("PR 12", "https://github.com/x/y/pull/12"),)).lane
    assert lane.links == (records.Link("PR 12", "https://github.com/x/y/pull/12"),)


def test_Ask_SameQuestionTwice_UnchangedChangedUpdated(root):
    open_lane(root)
    question = questions.make("flag-short", "Also accept -h?", ("yes", "no"), NOW)
    assert store.ask(root, CLAUDE, "measure-help", question, NOW).result == "opened"
    assert store.ask(root, CLAUDE, "measure-help", question, NOW).result == "unchanged"
    changed = questions.make("flag-short", "Accept -h too?", ("yes", "no"), NOW)
    assert store.ask(root, CLAUDE, "measure-help", changed, NOW).result == "updated"
    assert len(store.read(root, "measure-help").questions) == 1


def test_Ask_TwentyOneQuestions_RefusedNamingTwenty(root):
    open_lane(root)
    for number in range(store.MAX_QUESTIONS):
        store.ask(root, CLAUDE, "measure-help", questions.make(f"q-{number}", "Why?", (), NOW), NOW)
    with pytest.raises(store.Refused, match="20"):
        store.ask(root, CLAUDE, "measure-help", questions.make("q-more", "Why?", (), NOW), NOW)


def test_Close_PersonActor_OutcomeDropped(root):
    open_lane(root)
    person = actors.Actor("overlay", "me", "iphone")
    closed = store.close(root, person, "measure-help", outcome="dropped", now=NOW)
    assert closed.result == "closed" and closed.lane.outcome == "dropped" and closed.lane.closed_at
    open_lane(root, lane_id="second")
    with pytest.raises(store.Refused):
        store.close(root, person, "second", outcome="done", now=NOW)


def test_Close_OutsideActorOfAnotherAgent_Refused(root):
    open_lane(root)
    with pytest.raises(store.Refused, match="claude"):
        store.close(root, actors.Actor("outside", "script"), "measure-help", outcome="done", now=NOW)


def test_Close_Twice_UnchangedOneLogLine(root):
    open_lane(root)
    agent = actors.Actor("outside", "claude")
    store.close(root, agent, "measure-help", outcome="done", now=NOW)
    assert store.close(root, agent, "measure-help", outcome="done", now=NOW).result == "unchanged"
    assert [line["kind"] for line in store.log(root, "measure-help")] == ["opened", "closed"]


def test_Close_SourceThatIsNeitherPersonNorOutside_Refused(root):
    open_lane(root)
    with pytest.raises(store.Refused):
        store.close(root, actors.Actor("flow", "engine"), "measure-help", outcome="dropped", now=NOW)


def test_Store_FoldersAndFiles_0700And0600(root):
    open_lane(root)
    put_row(root)
    modes = {path: stat.S_IMODE(os.stat(path).st_mode) for path in [root, *root.rglob("*")]}
    assert modes[root] == modes[root / "measure-help"] == 0o700
    assert modes[root / "measure-help" / "lane.json"] == modes[root / "measure-help" / "log.jsonl"] == 0o600
    assert all(mode in (0o700, 0o600) for mode in modes.values())


def test_Store_ReadsNeverCreateTheFolder(root):
    assert store.lanes(root) == [] and store.lanes(root, closed=True) == []
    with pytest.raises(store.Refused, match="no outside lane"):
        store.read(root, "measure-help")
    assert not root.exists()


def test_Store_BrokenRecord_ReadRaisesStoreErrorAndScanReportsIt(root):
    open_lane(root)
    open_lane(root, lane_id="second")
    (root / "measure-help" / "lane.json").write_text("{not json")
    with pytest.raises(store.StoreError, match="measure-help"):
        store.read(root, "measure-help")
    found, problems = store.scan(root)
    assert [lane.id for lane in found] == ["second"] and len(problems) == 1


def test_Store_RecordIsJsonWithAgentAndWhere(root):
    open_lane(root)
    data = json.loads((root / "measure-help" / "lane.json").read_text())
    assert data["agent"] == {"name": "claude", "where": "/work/work"}
    assert data["phase"] == "draft" and data["read_up_to"] == 0 and data["closed_at"] is None
    line = appendlog.read(root / "measure-help" / "log.jsonl")[0]
    assert line["by"] == "outside:claude" and line["where"] == "/work/work"


def test_Store_NotePushed_RecordsTime(root):
    open_lane(root)
    store.note_pushed(root, "measure-help", NOW)
    assert store.read(root, "measure-help").pushed_at == "2026-11-16T10:00:00+00:00"


def test_AgentHere_InRepository_WhereIsMainWorktree(repo, tmp_path):
    (repo / "sub").mkdir()
    assert records.agent_here("claude", repo / "sub").where == repo
    linked = tmp_path / "linked"
    subprocess.run(["git", "-C", str(repo), "worktree", "add", "-q", str(linked), "-b", "side"], check=True)
    assert records.agent_here("claude", linked).where == repo


def test_AgentHere_NotARepository_WhereIsTheFolder(tmp_path):
    folder = tmp_path / "plain"
    folder.mkdir()
    assert records.agent_here("claude", folder).where == folder


def test_AgentHere_BadName_Refused(tmp_path):
    with pytest.raises(records.Refused):
        records.agent_here("Not Valid", tmp_path)


# -- review 17, finding 8: the history of a lane is bounded --------------------------------------------------


def _alternate(root, times, start=0):
    for number in range(start, start + times):
        store.put_row(root, CLAUDE, "measure-help", "r1", title="Help", steps=STEPS,
                      expected="usage", note=f"variant {number % 2}", now=NOW)


def test_LogCap_AlternatingOneRowUntilTheCap_RefusesTheNextAppendAndLeavesTheRecordAsItWas(root, monkeypatch):
    monkeypatch.setattr(store, "MAX_LOG_ENTRIES", 12)
    open_lane(root)
    with pytest.raises(store.Refused, match="close the lane and open a new one"):
        _alternate(root, 50)

    log = store.log(root, "measure-help")
    assert len(log) == 12
    lane = store.read(root, "measure-help")
    assert lane.rows[0].revision == 11, "the refused change is not in the record: 10 changes after the first put"


def test_LogCap_ByteCap_RefusesAnAppendThatWouldPassIt(root, monkeypatch):
    open_lane(root)
    size = (root / "measure-help" / store.LOG_FILE).stat().st_size
    monkeypatch.setattr(store, "MAX_LOG_BYTES", size + 100)
    with pytest.raises(store.Refused, match="close the lane and open a new one"):
        _alternate(root, 5)
    assert (root / "measure-help" / store.LOG_FILE).stat().st_size <= size + 100


def test_LogCap_AtTheCap_ThePersonsAnswerAndTheAgentsCloseStillGoThrough(root, monkeypatch):
    from sakura.outside import inbox

    monkeypatch.setattr(store, "MAX_LOG_ENTRIES", 6)
    open_lane(root)
    with pytest.raises(store.Refused):
        _alternate(root, 20)
    row = store.read(root, "measure-help").rows[0]
    person = actors.Actor("overlay", "me", "iphone")

    inbox.mark(root, person, "measure-help", row.id, row.digest, "ok", "", NOW)
    closed = store.close(root, actors.Actor("outside", "claude"), "measure-help", outcome="done", now=NOW)

    assert closed.result == "closed"
    assert len(store.log(root, "measure-help")) == 8


def test_Log_ReadsParseNoMoreThanTheCap(root):
    open_lane(root)
    path = root / "measure-help" / store.LOG_FILE
    for number in range(40):
        appendlog.append(path, {"kind": "steer", "text": "x" * 50})
    cap = 600

    entries = appendlog.read(path, max_bytes=cap)

    assert 1 <= len(entries) < 41
    assert sum(len(json.dumps(entry, separators=(",", ":"))) + 1 for entry in entries) <= cap


def test_Scan_AClosedLane_IsNotReadAgainByTheOpenScan(root, monkeypatch):
    open_lane(root)
    open_lane(root, lane_id="second-lane", agent=CLAUDE)
    store.close(root, actors.Actor("outside", "claude"), "measure-help", outcome="done", now=NOW)
    opened = []
    real = store.read
    monkeypatch.setattr(store, "read", lambda folder, lane_id: opened.append(lane_id) or real(folder, lane_id))

    lanes, _ = store.scan(root)

    assert [lane.id for lane in lanes] == ["second-lane"] and opened == ["second-lane"]


# -- review 18, findings 5 and 6 ------------------------------------------------------------------------------------

WORST_REPLY = "\U0001F600" * 8000  # 8000 characters, the longest valid reply; each serializes to twelve bytes


def _ask_twenty(root):
    for number in range(store.MAX_QUESTIONS):
        asked = questions.make(f"q-{number}", "Also accept -h?", ("yes", "no"), NOW)
        store.ask(root, CLAUDE, "measure-help", asked, NOW)


def test_LogCap_TwentyQuestionsAndAFullAgentHistory_EveryWorstCaseReplyIsStillAccepted(root):
    from sakura.outside import inbox

    open_lane(root)
    _ask_twenty(root)
    with pytest.raises(store.Refused, match="close the lane and open a new one"):
        _alternate(root, 5000)
    person = actors.Actor("overlay", "me", "iphone")

    for asked in store.read(root, "measure-help").questions:
        inbox.reply(root, person, "measure-help", asked.id, asked.digest, "", WORST_REPLY, NOW)

    lane, log = store.read(root, "measure-help"), store.log(root, "measure-help")
    assert inbox.open_questions(lane, log) == []


def test_LogCap_AnswersDoNotUseUpTheAgentsEntries(root, monkeypatch):
    from sakura.outside import inbox

    monkeypatch.setattr(store, "MAX_LOG_ENTRIES", 10)
    open_lane(root)
    person = actors.Actor("overlay", "me", "iphone")
    for number in range(5):
        row = store.put_row(root, CLAUDE, "measure-help", f"r-{number}", title="Help", steps=STEPS, expected="usage",
                            now=NOW).lane.rows[-1]
        inbox.mark(root, person, "measure-help", row.id, row.digest, "ok", "", NOW)

    # five agent lines and five answers: the agent still has its own five entries left
    _alternate(root, 4)
    assert len([line for line in store.log(root, "measure-help") if line["kind"] != "mark"]) == 10
def test_Log_AHistoryOverTheReadBound_IsBrokenNotSilentlyShortened(root, monkeypatch):
    from sakura.outside import inbox

    open_lane(root)
    row = store.put_row(root, CLAUDE, "measure-help", "r1", title="Help", steps=STEPS, expected="usage",
                        now=NOW).lane.rows[0]
    person = actors.Actor("overlay", "me", "iphone")
    inbox.mark(root, person, "measure-help", row.id, row.digest, "ok", "", NOW)
    path = root / "measure-help" / store.LOG_FILE
    with open(path, "ab") as stream:  # a history from before the caps, grown past what a read parses
        stream.write(b'{"seq":9,"kind":"steer","text":"' + b"x" * (store.LOG_READ_BYTES + 10) + b'"}\n')

    lanes, problems = store.scan(root)

    assert lanes == [] and len(problems) == 1 and "history" in problems[0] and "measure-help" in problems[0]
    with pytest.raises(store.StoreError, match="history"):
        store.log(root, "measure-help")
    with pytest.raises(store.Refused, match="history"):
        store.put_row(root, CLAUDE, "measure-help", "r2", title="Help", steps=STEPS, expected="usage", now=NOW)


def test_Log_ABrokenHistory_CanStillBeClosed_AndThenIsNoLongerReported(root):
    open_lane(root)
    path = root / "measure-help" / store.LOG_FILE
    with open(path, "ab") as stream:
        stream.write(b'{"seq":9,"kind":"steer","text":"' + b"x" * (store.LOG_READ_BYTES + 10) + b'"}\n')

    closed = store.close(root, actors.Actor("outside", "claude"), "measure-help", outcome="dropped", now=NOW)

    assert closed.result == "closed"
    assert store.scan(root) == ([], [])
