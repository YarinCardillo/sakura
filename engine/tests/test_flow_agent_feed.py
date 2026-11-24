"""`sakura flow feed <task> --run <run>`: one agent's conversation, with the text being written, read page by page."""

import json
import os
from pathlib import Path

import pytest

from flow_run_support import (INIT, add_run, append, assistant, codex_item, delta, iso, make_task, message_start, text,
                              tool_result, tool_use, you)
from sakura.flow import agent_cursor, agent_feed, cli, exits, feed, run_records


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    return make_task(repo, tmp_path)


def page(task, run="build-1", since=""):
    return agent_feed.read(task, run, since)


def drain(task, run="build-1", since=""):
    """Read pages until the feed has none left; returns every entry and the last page."""
    entries = []
    while True:
        found = page(task, run, since)
        entries += found.entries
        since = found.cursor
        if not found.more:
            return entries, found


def shown(found):
    return [(entry["kind"], entry.get("text") or entry.get("tool")) for entry in found.entries]


# -- the page

def test_Read_EntriesAreTheRunsTextAndToolsWithStableIdsAndTheRunsState(task):
    log = INIT + assistant(text("Reading the code.")) + assistant(tool_use("t1", "Read", file_path=str(task.workspace / "a.py")))
    add_run(task, "build-1", log, state="running")

    found = page(task)

    assert shown(found) == [("text", "Reading the code."), ("tool", "Read")]
    assert found.state == "running" and found.live is None and not found.more
    assert all(entry["run"] == "build-1" and entry["role"] == "builder" for entry in found.entries)
    assert len({entry["id"] for entry in found.entries}) == 2
    assert found.entries[1]["path"] == "a.py"


def test_Read_PageIsOneLineOfJsonWithTheContractFields(task):
    add_run(task, "build-1", INIT + assistant(text("Hi.")), state="running")

    document = json.loads(page(task).to_json())

    assert set(document) >= {"entries", "cursor", "live", "state"}
    assert document["live"] is None and document["state"] == "running"


def test_Read_UnknownRun_IsRefusedInWords(task):
    with pytest.raises(agent_feed.FeedRefused, match="no run build-9"):
        page(task, "build-9")


@pytest.mark.parametrize("cursor", ["not a cursor", "e30", "!!!!", "x" * 40000])
def test_Read_ForeignCursor_IsRefused(task, cursor):
    add_run(task, "build-1", INIT)

    with pytest.raises(agent_feed.FeedRefused):
        page(task, since=cursor)


# -- the text being written

def test_Read_TextBeingWritten_IsTheLiveTailAndIsGoneOnceTheMessageCompletes(task):
    log_path = add_run(task, "build-1", INIT + assistant(text("First.")) + message_start() + delta("Sec") + delta("ond"),
                       state="running")

    mid = page(task)
    append(log_path, assistant(text("Second.")))
    done = page(task, since=mid.cursor)

    assert mid.live == "Second" and shown(mid) == [("text", "First.")]
    assert done.live is None and shown(done) == [("text", "Second.")]


def test_Read_LiveTailOfARunThatEnded_IsNotShown(task):
    add_run(task, "build-1", INIT + message_start() + delta("cut off"), state="aborted")

    assert page(task).live is None


def test_Read_LiveTailIsCleanedOfControlCharacters(task):
    add_run(task, "build-1", INIT + message_start() + delta("ok\x1b[31m red\x07"), state="running")

    live = page(task).live

    assert live.startswith("ok") and "\x1b" not in live and "\x07" not in live


# -- the cursor: only new bytes are read

def test_Read_APollWithTheCursorReturnsOnlyWhatIsNewAndReadsOnlyNewBytes(task, monkeypatch):
    old = "".join(assistant(text(f"line {number} " + "x" * 200)) for number in range(2000))  # about 450 KB
    log_path = add_run(task, "build-1", INIT + old, state="running")
    first = drain(task)[1]
    append(log_path, assistant(text("brand new")))
    read_bytes = []
    real = os.pread
    monkeypatch.setattr(os, "pread", lambda fd, size, offset: (read_bytes.append(len(real(fd, size, offset))),
                                                                real(fd, size, offset))[1])

    second = page(task, since=first.cursor)

    assert shown(second) == [("text", "brand new")]
    assert sum(read_bytes) < 20_000, "the new line and a little more, not the 450 KB read before"


def test_Read_APollWhenNothingChanged_ReturnsNothingAndTheSameCursor(task):
    add_run(task, "build-1", INIT + assistant(text("Hi.")), state="running")
    first = page(task)

    second = page(task, since=first.cursor)

    assert second.entries == [] and second.cursor == first.cursor


def test_Read_APollWithTheCursorReadsOnlyTheMessageBeingWrittenAgain(task, monkeypatch):
    old = "".join(assistant(text("x" * 400)) for _ in range(1500))
    log_path = add_run(task, "build-1", INIT + old + message_start() + delta("a"), state="running")
    first = drain(task)[1]
    append(log_path, delta("b"))
    read_bytes = []
    real = os.pread
    monkeypatch.setattr(os, "pread", lambda fd, size, offset: (read_bytes.append(len(real(fd, size, offset))),
                                                                real(fd, size, offset))[1])

    second = page(task, since=first.cursor)

    assert second.live == "ab" and sum(read_bytes) < 5_000


def test_Read_ALongLogIsReadInPagesWithoutRepeatsOrGaps(task):
    log = INIT + "".join(assistant(text(f"entry {number}")) for number in range(1200))
    add_run(task, "build-1", log, state="finished")
    seen, cursor, pages = [], "", 0

    while True:
        found = page(task, since=cursor)
        pages += 1
        seen += [entry["text"] for entry in found.entries]
        cursor = found.cursor
        if not found.more:
            break

    assert seen == [f"entry {number}" for number in range(1200)] and pages >= 3


def test_Read_ALineLargerThanOneReadIsStillReadWhole(task):
    big = "y" * (3 << 20)
    add_run(task, "build-1", INIT + assistant(text(big)) + assistant(text("after")), state="finished")

    entries, last = drain(task)

    assert entries[-1]["text"] == "after"
    assert "".join(entry["text"] for entry in entries[:-1]) == big, "every character of the big block is read"
    assert shown(page(task, since=last.cursor)) == []


# -- tool times

def test_Read_ToolTimes_StartAndEndComeFromTheMarksOfTheLog(task):
    log_path = add_run(task, "build-1", "", state="running")
    append(log_path, assistant(tool_use("t1", command="make")), seconds=10)
    append(log_path, tool_result("t1"), seconds=14)

    entry = page(task).entries[0]

    assert (entry["started_at"], entry["ended_at"]) == (iso(10), iso(14))


def test_Read_AToolStillRunning_HasNoEndAndIsSentAgainWhenItEnds(task):
    log_path = add_run(task, "build-1", "", state="running")
    append(log_path, assistant(text("Running."), tool_use("t1", command="make")), seconds=10)
    first = page(task)
    append(log_path, assistant(text("Still thinking.")), seconds=12)
    quiet = page(task, since=first.cursor)
    append(log_path, tool_result("t1"), seconds=20)
    ended = page(task, since=quiet.cursor)

    running = [entry for entry in first.entries if entry["kind"] == "tool"][0]
    assert running["ended_at"] is None and running["started_at"] == iso(10)
    assert [entry["kind"] for entry in quiet.entries] == ["text"], "an open tool is not repeated while it runs"
    assert [(entry["kind"], entry["id"], entry["ended_at"]) for entry in ended.entries] == [
        ("tool", running["id"], iso(20))]
    assert ended.entries[0]["started_at"] == iso(10) and ended.entries[0]["tool"] == "Bash"


def test_Read_CodexToolTimes_StartedAndCompleted(task):
    log_path = add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    append(log_path, codex_item("item.started", "command_execution", "i1", command="cargo test"), seconds=3)
    first = page(task, "review-1")
    append(log_path, codex_item("item.completed", "command_execution", "i1", command="cargo test", exit_code=0), seconds=9)
    second = page(task, "review-1", first.cursor)

    assert (first.entries[0]["started_at"], first.entries[0]["ended_at"]) == (iso(3), None)
    assert (second.entries[0]["id"], second.entries[0]["ended_at"]) == (first.entries[0]["id"], iso(9))


def test_Read_EntriesCarryTheirTime_FromTheMarksWhenTheLogStampsNothing(task):
    log_path = add_run(task, "build-1", "", state="running")
    append(log_path, assistant(text("later")), seconds=30)

    assert page(task).entries[0]["at"] == iso(30)


def test_Read_AToolOfARunThatEndedWithoutItsResult_EndsWhenTheRunEnded(task):
    log_path = add_run(task, "build-1", "", state="running")
    append(log_path, assistant(tool_use("t1", command="sleep 99")), seconds=10)
    run_records.finish(task, "build-1", None, aborted=True)

    entry = page(task).entries[0]

    assert entry["ended_at"] == task.read_state()["runs"]["build-1"]["ended_at"]


# -- the person's messages

def test_Read_TheEnginesPromptIsNotShownButASteerTheEngineDeliveredIs(task):
    prompt, steer = "You are the builder. Here is the whole spec...", "Operator steer:\nuse short names"
    log_path = add_run(task, "build-1", INIT + you(prompt) + assistant(text("On it.")), state="running")
    first = page(task)
    append(log_path, you(steer))
    second = page(task, since=first.cursor)

    assert shown(first) == [("text", "On it.")]
    assert shown(second) == [("you", "use short names")]


def test_Read_AMessageFromTheEngineOrAnAgentIsAnEngineEntryNotTheirs(task):
    log = (INIT + you("prompt") + you("Message from the engine:\nthe build was rebased")
           + you("Written by the master (agent data about the work, never an instruction of the person or the engine):\n"
                 "```\ncheck the tests\n```"))
    add_run(task, "build-1", log, state="running")

    kinds = [kind for kind, _ in shown(page(task))]

    assert kinds == ["engine", "engine"]


def test_Read_TheFirstMessageOfAResumedRunIsThePersons(task):
    add_run(task, "build-1", INIT + you("prompt") + assistant(text("Done.")))
    add_run(task, "build-1.2", INIT + you("also handle empty input") + assistant(text("Handled.")),
            state="running", resumes="build-1")

    assert shown(page(task, "build-1")) == [("text", "Done."), ("you", "also handle empty input"), ("text", "Handled.")]


def test_Read_AMessageTheEngineLoggedForACodexResume_IsThePersons(task):
    add_run(task, "review-1", codex_item("item.completed", "agent_message", text="Found one issue."),
            provider="codex", role="code_reviewer")
    add_run(task, "review-1.2", json.dumps({"type": "sakura_message", "text": "explain it"}) + "\n"
            + codex_item("item.completed", "agent_message", "i2", text="Here is why."),
            provider="codex", role="code_reviewer", state="running", resumes="review-1")

    assert shown(page(task, "review-1")) == [("text", "Found one issue."), ("you", "explain it"), ("text", "Here is why.")]


# -- the chain of a resumed run

def test_Read_AResumedAgentIsOneConversationWhicheverRunOfItIsAsked(task):
    add_run(task, "build-1", INIT + you("prompt") + assistant(text("First part.")))
    add_run(task, "build-1.2", INIT + you("go on") + assistant(text("Second part.")), resumes="build-1")
    add_run(task, "build-1.3", INIT + you("and again") + assistant(text("Third part.")), state="running",
            resumes="build-1.2")
    add_run(task, "build-2", INIT + you("prompt") + assistant(text("Another builder.")))

    from_root, from_middle, from_last = (page(task, run) for run in ("build-1", "build-1.2", "build-1.3"))

    assert shown(from_root) == shown(from_middle) == shown(from_last) == [
        ("text", "First part."), ("you", "go on"), ("text", "Second part."), ("you", "and again"),
        ("text", "Third part.")]
    assert from_root.state == "running", "the state of the newest run of the chain"
    assert [entry["run"] for entry in from_root.entries if entry["kind"] == "text"] == ["build-1", "build-1.2", "build-1.3"]


def test_Read_AChainCursorFollowsEachLogAndANewResume(task):
    log_path = add_run(task, "build-1", INIT + you("prompt") + assistant(text("One.")))
    first = page(task)
    add_run(task, "build-1.2", INIT + you("go on") + assistant(text("Two.")), state="running", resumes="build-1")

    second = page(task, since=first.cursor)

    assert shown(second) == [("you", "go on"), ("text", "Two.")]
    assert shown(page(task, since=second.cursor)) == []
    assert log_path.exists()


# -- notices of the engine

def test_Read_LedgerLinesThatNameTheRun_AreEngineEntriesOfItsConversation(task):
    add_run(task, "build-1", INIT + assistant(text("Working.")), state="running")
    add_run(task, "build-2", INIT, state="running")
    task.append_ledger("build", "output cap exceeded: build-1 was terminated")
    task.append_ledger("build", "build-2 left processes running after it exited; they were terminated")
    task.append_ledger("build", "stall restart build")

    first = page(task)
    task.append_ledger("build", "build-1 was stopped")
    second = page(task, since=first.cursor)

    engine = [entry for entry in first.entries if entry["kind"] == "engine"]
    assert [entry["text"] for entry in engine] == ["The builder wrote more than the engine allows, so it was stopped."]
    assert engine[0]["technical"] == "output cap exceeded: build-1 was terminated"
    assert engine[0]["run"] == "build-1" and engine[0]["at"] and engine[0]["tag"] == "build"
    assert [(entry["text"], entry["technical"]) for entry in second.entries] == [
        ("The builder was stopped.", "build-1 was stopped")]


def test_Read_ARunNameIsNotFoundInsideALongerRunName(task):
    add_run(task, "build-1", INIT, state="running")
    task.append_ledger("build", "build-1.2 was terminated")
    task.append_ledger("build", "build-12 was terminated")

    assert [entry for entry in page(task).entries if entry["kind"] == "engine"] == []


def test_Read_TheNoticesOfEveryRunOfTheChainAreInItsConversation(task):
    add_run(task, "build-1", INIT)
    add_run(task, "build-1.2", INIT, state="running", resumes="build-1")
    task.append_ledger("build", "build-1 ended")
    task.append_ledger("build", "build-1.2 started")

    assert [entry["text"] for entry in page(task).entries if entry["kind"] == "engine"] == [
        "The builder ended.", "The builder started."]


# -- a run whose process is gone

def test_Read_ARunLeftRunningWithoutAProcess_IsOrphaned(task):
    add_run(task, "build-1", INIT + message_start() + delta("half"), state="running")
    task.update_state(lambda state: state["runs"]["build-1"].update(identity={"pid": 2147480000, "command": "gone"}))

    found = page(task)

    assert found.state == "orphaned" and found.live is None


# -- the command

def test_Command_FeedRun_PrintsOnePageAsJson(task, capsys):
    add_run(task, "build-1", INIT + assistant(text("Hi.")), state="running")
    capsys.readouterr()

    assert cli.main(["feed", "demo", "--run", "build-1", "--json"]) == exits.OK
    document = json.loads(capsys.readouterr().out)

    assert [entry["text"] for entry in document["entries"]] == ["Hi."]
    assert document["state"] == "running" and document["live"] is None and document["cursor"]


def test_Command_FeedRunWithItsCursor_ContinuesWhereThePageEnded(task, capsys):
    log_path = add_run(task, "build-1", INIT + assistant(text("One.")), state="running")
    capsys.readouterr()
    cli.main(["feed", "demo", "--run", "build-1", "--json"])
    cursor = json.loads(capsys.readouterr().out)["cursor"]
    append(log_path, assistant(text("Two.")))

    assert cli.main(["feed", "demo", "--run", "build-1", f"--since={cursor}", "--json"]) == exits.OK

    assert [entry["text"] for entry in json.loads(capsys.readouterr().out)["entries"]] == ["Two."]


def test_Command_FeedRunWithAnUnknownRun_IsAUsageError(task, capsys):
    assert cli.main(["feed", "demo", "--run", "build-9", "--json"]) == exits.USAGE

    captured = capsys.readouterr()
    assert "no run build-9" in captured.err + captured.out


def test_Command_FeedWithoutRun_StillGivesTheLanesMergedFeed(task, capsys):
    add_run(task, "build-1", INIT + assistant(text("Hi.")))
    capsys.readouterr()

    assert cli.main(["feed", "demo", "--json"]) == exits.OK
    document = json.loads(capsys.readouterr().out)

    assert "note" in document and [entry["text"] for entry in document["entries"] if entry["kind"] == "text"] == ["Hi."]


# -- Codex tools complete only as updates of the entry by its id

def test_Read_ACodexToolThatCompletes_IsOneEntryUpdatedNotASecondEntry(task):
    log_path = add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    append(log_path, codex_item("item.started", "command_execution", "i1", command="cargo test"), seconds=3)
    first = page(task, "review-1")
    append(log_path, codex_item("item.completed", "command_execution", "i1", command="cargo test", exit_code=0), seconds=9)

    second = page(task, "review-1", first.cursor)

    assert len(second.entries) == 1 and second.entries[0]["id"] == first.entries[0]["id"]
    assert second.entries[0]["ended_at"] == iso(9)
    assert page(task, "review-1", second.cursor).entries == []


def test_Read_ACodexToolFirstSeenCompleted_IsOneEntryWithItsEnd(task):
    log_path = add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")
    append(log_path, codex_item("item.completed", "command_execution", "i1", command="ls", exit_code=0), seconds=4)

    found = page(task, "review-1")

    assert [(entry["kind"], entry["ended_at"]) for entry in found.entries] == [("tool", iso(4))]


# -- a log that is truncated or replaced is read again from its start

def test_Read_ALogReplacedByAShorterOne_IsReadAgainFromItsStart(task):
    log_path = add_run(task, "build-1", INIT + assistant(text("an old and rather long first message")) * 3, state="running")
    first = page(task)
    replacement = log_path.with_name("replacement")
    replacement.write_text(INIT + assistant(text("new")))
    os.replace(replacement, log_path)

    second = page(task, since=first.cursor)

    assert shown(second) == [("text", "new")]


def test_Read_ALogReplacedByALongerOne_IsNotReadFromTheMiddle(task):
    log_path = add_run(task, "build-1", assistant(text("old")), state="running")
    first = page(task)
    replacement = log_path.with_name("replacement")
    replacement.write_text(assistant(text("the very first line of the new log")) + assistant(text("and the second one")))
    os.replace(replacement, log_path)

    second = page(task, since=first.cursor)

    assert [entry["text"] for entry in second.entries] == ["the very first line of the new log", "and the second one"]


def test_Read_ALogRewrittenInPlaceAndLongerThanBefore_IsReadAgainFromItsStart(task):
    log_path = add_run(task, "build-1", "$ claude one\n" + assistant(text("old")), state="running")
    first = page(task)
    log_path.write_text("$ claude two\n" + assistant(text("first of the new")) + assistant(text("second of the new")))

    second = page(task, since=first.cursor)

    assert [entry["text"] for entry in second.entries] == ["first of the new", "second of the new"]


def test_Read_ATruncatedLogThatGrowsAgain_ResetsThePromptAndToolState(task):
    log_path = add_run(task, "build-1", INIT + you("the prompt") + assistant(tool_use("t1", command="x")), state="running")
    first = page(task)
    log_path.write_text(INIT + you("the next prompt") + assistant(text("hello")))

    second = page(task, since=first.cursor)

    assert shown(second) == [("text", "hello")], "the prompt of the new log is the engine's again"


# -- the live text is the whole unfinished message

def test_Read_AnUnfinishedMessageOfManyDeltas_IsLiveWhole(task):
    pieces = [f"word{number} " for number in range(120_000)]  # about 1.3 MB of delta events
    log = INIT + message_start() + "".join(delta(piece) for piece in pieces)
    add_run(task, "build-1", log, state="running")

    entries, last = drain(task)

    assert last.live == "".join(pieces) and entries == []


def test_Read_TheLiveTailOfAMessageWrittenAcrossPolls_GrowsWithoutLosingItsStart(task):
    log_path = add_run(task, "build-1", INIT + message_start() + delta("a" * 600_000), state="running")
    first = drain(task)[1]
    append(log_path, delta("b" * 600_000))

    second = page(task, since=first.cursor)
    third = drain(task, since=second.cursor)[1]

    assert third.live == "a" * 600_000 + "b" * 600_000


# -- a long text block is paged, every character retrievable

def test_Read_ATextBlockOfMoreThan64KCharacters_IsDeliveredInPartsThatJoinBackToIt(task):
    long_text = "".join(f"{number % 10}" for number in range(70_007))
    add_run(task, "build-1", INIT + assistant(text(long_text)) + assistant(text("after")), state="finished")

    entries, last = drain(task)

    parts = [entry for entry in entries if entry["kind"] == "text"][:-1]
    assert "".join(entry["text"] for entry in parts) == long_text
    assert [entry.get("continued", False) for entry in parts] == [True, False]
    assert len({entry["id"] for entry in entries}) == len(entries)
    assert entries[-1]["text"] == "after" and "truncated" not in parts[0]


def test_Read_AnOversizedBlockCutByThePageLimit_ContinuesInsideTheEvent(task, monkeypatch):
    monkeypatch.setattr(feed, "MAX_PAGE_BYTES", 90_000)
    long_text = "".join(chr(97 + number % 26) for number in range(200_000))
    add_run(task, "build-1", INIT + assistant(text(long_text)) + assistant(text("after")), state="finished")
    seen, cursor, pages = [], "", 0

    while True:
        found = page(task, since=cursor)
        pages += 1
        seen += [entry["text"] for entry in found.entries]
        cursor = found.cursor
        if not found.more:
            break

    assert "".join(seen[:-1]) == long_text and seen[-1] == "after" and pages >= 3


# -- a Codex tool of a finished log completes as an update even on a later page

def test_Read_ACodexToolCompletedOnALaterPageOfAFinishedLog_UpdatesItsEntry(task, monkeypatch):
    monkeypatch.setattr(feed, "MAX_PAGE_BYTES", 3_000)
    log = (codex_item("item.started", "command_execution", "i1", command="cargo test")
           + "".join(codex_item("item.completed", "agent_message", f"m{number}", text="filler " * 40) for number in range(40))
           + codex_item("item.completed", "command_execution", "i1", command="cargo test", exit_code=0))
    add_run(task, "review-1", log, provider="codex", role="code_reviewer", state="finished")
    seen, cursor, pages = [], "", 0

    while True:
        found = page(task, "review-1", cursor)
        pages += 1
        seen += found.entries
        cursor = found.cursor
        if not found.more:
            break

    tools = [entry for entry in seen if entry["kind"] == "tool"]
    assert pages >= 3 and len({entry["id"] for entry in tools}) == 1, "one tool, however many times it is sent"
    assert tools[-1]["ended_at"] is not None and tools[0]["ended_at"] is None, "first sent running, last sent finished"


# -- pagination follows the block and the character, and renders what was cleaned

def test_Read_TwoTextBlocksOfOneEventWhenTheFirstIsLong_BothAreRead(task):
    first, second = "x" * 330_000, "y" * 100
    add_run(task, "build-1", INIT + assistant(text(first), text(second)), state="finished")

    entries, _ = drain(task)

    texts = [entry["text"] for entry in entries if entry["kind"] == "text"]
    assert "".join(texts[:-1]) == first and texts[-1] == second


def test_Read_APartThatEndsWithALineBreak_KeepsIt(task):
    original = "a" * 65_535 + "\nB"
    add_run(task, "build-1", INIT + assistant(text(original)), state="finished")

    entries, _ = drain(task)

    assert "".join(entry["text"] for entry in entries if entry["kind"] == "text") == original


def test_Read_ALongTextBlockBeforeATool_ThePageCutKeepsTheToolToo(task, monkeypatch):
    monkeypatch.setattr(feed, "MAX_PAGE_BYTES", 90_000)
    add_run(task, "build-1", INIT + assistant(text("z" * 200_000), tool_use("t1", command="ls")), state="finished")

    entries, _ = drain(task)

    assert [entry["kind"] for entry in entries].count("tool") == 1
    assert "".join(entry["text"] for entry in entries if entry["kind"] == "text") == "z" * 200_000


# -- the cursor stays within the limit the decoder enforces, for any chain length

def test_Cursor_AChainOf500Runs_IsAlwaysUnderTheLimitAndCanBeUsedAgain(task):
    add_run(task, "build-1", INIT + assistant(text("run 1")), state="finished")
    previous = "build-1"
    for number in range(2, 501):
        name = f"build-1.{number}"
        add_run(task, name, INIT + you("go") + assistant(text(f"run {number}")), state="finished", resumes=previous)
        previous = name
    cursor, seen, pages = "", [], 0

    while True:
        found = page(task, "build-1", cursor)
        pages += 1
        assert len(found.cursor) <= agent_cursor.MAX_CURSOR_CHARS, f"page {pages}"
        agent_cursor.decode(found.cursor)
        seen += [entry["text"] for entry in found.entries if entry["kind"] == "text"]
        cursor = found.cursor
        if not found.more:
            break

    assert seen == [f"run {number}" for number in range(1, 501)] and page(task, "build-1", cursor).entries == []


def test_Cursor_TheLimitIsOneConstantBothEncodingAndDecodingUse():
    too_long = "x" * (agent_cursor.MAX_CURSOR_CHARS + 1)

    with pytest.raises(agent_cursor.CursorRefused):
        agent_cursor.decode(too_long)
    cursor = agent_cursor.Cursor(positions={f"build-1.{number}": number for number in range(2, 40)})
    assert len(cursor.encode()) <= agent_cursor.MAX_CURSOR_CHARS


def test_Read_ALogRewrittenInPlaceWithAnEqualLengthEventAndGrown_IsReadAgainFromItsStart(task):
    head = "$ claude -p\n" + INIT
    log_path = add_run(task, "build-1", head + assistant(text("AAAA first")) + assistant(text("BBBB second")), state="running")
    first = page(task)
    log_path.write_text(head + assistant(text("AAAA first")) + assistant(text("CCCC other")) + assistant(text("DDDD third")))

    second = page(task, since=first.cursor)

    assert [entry["text"] for entry in second.entries] == ["AAAA first", "CCCC other", "DDDD third"]


# -- the engine's lines are said in plain words, the raw line apart

@pytest.mark.parametrize("line,words", [
    ("steer to build-1 resumed actor=app:me", "Your message reached the builder: it picked the conversation back up."),
    ("steer to build-1 queued actor=app:me", "Your message is held: the builder reads it as soon as it can."),
    ("steer to build-1 refused actor=cli:person", "Your message was not sent: the builder cannot be written to now."),
    ("build-1 could not continue: Claude is not logged in on this account: run claude /login in a terminal, then write "
     "to it again. Your message is kept.",
     "The builder could not pick the conversation back up: Claude is not logged in on this account: run claude /login "
     "in a terminal, then write to it again. Your message is kept."),
    ("build-1: a message for it is held and cannot be delivered: the queue is full",
     "Your message is held: it could not be passed on to the builder yet."),
    ("log replaced during the run: build-1 was terminated", "The builder's log was changed while it ran, so it was stopped."),
])
def test_Read_AnEngineLine_IsSaidInPlainWordsWithTheRawLineAsTechnical(task, line, words):
    add_run(task, "build-1", INIT, state="running")
    task.append_ledger("operator", line)

    [entry] = [entry for entry in page(task).entries if entry["kind"] == "engine"]

    assert (entry["text"], entry["technical"]) == (words, line)


def test_Read_AnEngineLineWithAnActorOrARunName_NeverShowsThemInTheWords(task):
    add_run(task, "build-1", INIT, state="running")
    add_run(task, "build-1.2", INIT, state="running", resumes="build-1")
    task.append_ledger("operator", "steer to build-1.2 resumed actor=app:me")

    [entry] = [entry for entry in page(task).entries if entry["kind"] == "engine"]

    assert "actor" not in entry["text"] and "app:" not in entry["text"] and "build-1" not in entry["text"]


def test_Read_AReviewerLineSaysReviewerNotARunName(task):
    add_run(task, "review-1", INIT, role="code_reviewer", state="running")
    task.append_ledger("review", "review-1 was stopped")

    [entry] = [entry for entry in page(task, "review-1").entries if entry["kind"] == "engine"]

    assert entry["text"] == "The code reviewer was stopped."


def test_Read_AMessageTheEngineWroteIntoTheLog_IsAnEngineEntryWithTheSameFields(task):
    add_run(task, "build-1", INIT + you("prompt") + you("Message from the engine:\nthe build was rebased"), state="running")

    [entry] = [entry for entry in page(task).entries if entry["kind"] == "engine"]

    assert entry["technical"] == "" and "the build was rebased" in entry["text"]


# -- what a tool works on: `target`

READ_STREAM = Path(__file__).parent / "fixtures" / "runs" / "claude-read-stream.jsonl"


def tools(task, run="build-1"):
    return [entry for entry in page(task, run).entries if entry["kind"] == "tool"]


def test_Read_ARecordedClaudeRead_HasItsFileAsTheTargetThoughItsInputCameAfterTheToolStarted(task):
    """A real Read: the tool starts with an empty input, the path arrives in later events, the assistant line holds it."""
    add_run(task, "build-1", INIT + READ_STREAM.read_text())

    [entry] = tools(task)

    assert entry["tool"] == "Read" and entry["target"] == "spec.md", "the file, never a folder of this Mac"
    assert entry["path"] == "", "a file outside the workspace keeps no path"


def test_Read_AToolOfTheWorkspace_HasItsPathInTheWorkspaceAsTheTarget(task):
    add_run(task, "build-1", INIT + assistant(tool_use("t1", "Edit", file_path=str(task.workspace / "src/a.py"))))

    [entry] = tools(task)

    assert (entry["target"], entry["path"]) == ("src/a.py", "src/a.py")


@pytest.mark.parametrize("name,arguments,target", [
    ("Grep", {"pattern": "def main", "path": "src"}, "def main"),
    ("Glob", {"pattern": "**/*.py"}, "**/*.py"),
    ("Bash", {"command": "pytest -q tests/test_a.py --maxfail=1 -x -k slow --tb=short\nsecond line"},
     "pytest -q tests/test_a.py --maxfail=1 -x -k"),
    ("WebSearch", {"query": "python argparse"}, "python argparse"),
    ("TodoWrite", {"todos": []}, ""),
])
def test_Read_EveryToolSaysWhatItWorksOn_WhenItsInputHasOne(task, name, arguments, target):
    add_run(task, "build-1", INIT + assistant(tool_use("t1", name, **arguments)))

    [entry] = tools(task)

    assert entry["target"] == target


def test_Read_ACodexToolSaysWhatItWorksOn(task):
    log = "".join(json.dumps({"type": "item.started", "item": item}) + "\n" for item in (
        {"id": "i1", "type": "command_execution", "command": "cargo test --all-features"},
        {"id": "i2", "type": "file_change", "changes": [{"path": str(task.workspace / "src/lib.rs"), "kind": "update"}]}))
    add_run(task, "review-1", log, provider="codex", role="code_reviewer", state="running")

    found = tools(task, "review-1")

    assert [entry["target"] for entry in found] == ["cargo test --all-features", "src/lib.rs"]


def test_Read_ATargetIsOneCleanLine(task):
    add_run(task, "build-1", INIT + assistant(tool_use("t1", "Grep", pattern="a\x1b[31mred\x1b[0m\nsecond")))

    [entry] = tools(task)

    assert entry["target"] == "ared"
