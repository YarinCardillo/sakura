"""The run log reader: what an agent's log says it wrote and which tools it used."""

import json

import pytest

from sakura.flow import roles, run_log

CLAUDE = roles.parse_role("claude")
CODEX = roles.parse_role("codex")


def _assistant(*blocks):
    return json.dumps({"type": "assistant", "message": {"role": "assistant", "content": list(blocks)}}) + "\n"


def _codex(kind, item_type, **fields):
    return json.dumps({"type": kind, "item": {"id": fields.pop("id", "i1"), "type": item_type, **fields}}) + "\n"


def test_RunLog_ClaudeStream_TextAndToolCallsWithPaths():
    log = ("$ claude -p\n"
           + _assistant({"type": "thinking", "thinking": "private"}, {"type": "text", "text": "Reading the file."})
           + _assistant({"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/work/a.py", "offset": 12}},
                        {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "ls"}})
           + json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "content": "file bytes"}]}}) + "\n"
           + json.dumps({"type": "result", "result": "Done."}) + "\n")
    assert run_log.entries(CLAUDE, log) == [
        run_log.LogEntry("text", "Reading the file.", "", "", None),
        run_log.LogEntry("tool", "", "Read", "/work/a.py", 12),
        run_log.LogEntry("tool", "", "Bash", "", None, command="ls")]


def test_RunLog_CodexStream_TextOnlyNoPaths():
    log = (_codex("item.completed", "reasoning", text="private")
           + _codex("item.completed", "agent_message", id="i2", text="Looking.")
           + _codex("item.started", "command_execution", id="i3", command="cat /etc/hosts")
           + _codex("item.completed", "command_execution", id="i3", command="cat /etc/hosts", exit_code=0))
    found = run_log.entries(CODEX, log)
    assert found == [run_log.LogEntry("text", "Looking.", "", "", None),
                     run_log.LogEntry("tool", "", "command_execution", "", None, command="cat /etc/hosts")]


def test_RunLog_CodexError_IsTextFromItsMessage():
    log = _codex("item.completed", "error", message="hooks refused")
    assert run_log.entries(CODEX, log) == [run_log.LogEntry("text", "hooks refused", "", "", None)]


def test_RunLog_UnknownBlockOrItem_CountsAsATool():
    assert run_log.entries(CLAUDE, _assistant({"type": "server_tool_use"}, "stray")) == [
        run_log.LogEntry("tool", "", "server_tool_use", "", None), run_log.LogEntry("tool", "", "unknown", "", None)]
    assert run_log.entries(CODEX, '{"item":{"type":"mcp_tool_call","id":"x"}}\n')[0].kind == "tool"


def test_RunLog_BrokenLines_Skipped():
    log = "not json\n[1]\n" + "[" * 100000 + "\n" + _assistant({"type": "text", "text": "ok"})
    assert [entry.text for entry in run_log.entries(CLAUDE, log)] == ["ok"]


def test_RunLog_OffsetNotAWholeNumber_NoLine():
    log = _assistant({"type": "tool_use", "name": "Read", "input": {"path": "a", "offset": True}},
                     {"type": "tool_use", "name": "Read", "input": {"path": "b", "offset": -3}})
    assert [entry.line for entry in run_log.entries(CLAUDE, log)] == [None, None]


def test_RunLog_EventTime_IsKeptAsUtcOnEveryEntryOfThatEventAndEmptyWithoutOne():
    stamped = json.dumps({"type": "assistant", "timestamp": "2026-11-16T11:05:00+01:00", "message": {"content": [
        {"type": "text", "text": "hi"}, {"type": "tool_use", "name": "Bash", "input": {}}]}}) + "\n"
    assert [entry.at for entry in run_log.entries(CLAUDE, stamped + _assistant({"type": "text", "text": "later"}))] == [
        "2026-11-16T10:05:00+00:00", "2026-11-16T10:05:00+00:00", ""]


def test_RunLog_CodexEventTime_IsKept():
    log = json.dumps({"type": "item.completed", "timestamp": "2026-11-16T10:05:00Z",
                      "item": {"id": "i", "type": "agent_message", "text": "done"}}) + "\n"
    assert [entry.at for entry in run_log.entries(CODEX, log)] == ["2026-11-16T10:05:00+00:00"]


@pytest.mark.parametrize("stamp", ["yesterday", True, 1e30, [], None])
def test_RunLog_EventTimeThatIsNotATime_IsEmpty(stamp):
    log = json.dumps({"type": "assistant", "timestamp": stamp, "message": {"content": [{"type": "text", "text": "x"}]}}) + "\n"
    assert [entry.at for entry in run_log.entries(CLAUDE, log)] == [""]


def _user_result(tool_id):
    return json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tool_id,
                                                                 "content": "ok"}]}}) + "\n"


def test_Scan_ClaudeToolResult_ClosesTheToolByItsId():
    first = _assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
                       {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/work/a"}})
    log = first + _user_result("t1")

    scanned = run_log.scan(CLAUDE, log.encode())

    assert [entry.tool_id for entry in scanned.entries] == ["t1", "t2"]
    assert scanned.closed == {"t1": len(first.encode()) + len(_user_result("t1").encode())}


def test_Scan_EntriesCarryTheOffsetsOfTheirEventLine():
    skipped = "$ claude -p\n"
    first = _assistant({"type": "text", "text": "one"})
    second = _assistant({"type": "text", "text": "two"}, {"type": "tool_use", "id": "t", "name": "Bash", "input": {}})

    scanned = run_log.scan(CLAUDE, (skipped + first + second).encode(), base=1000)

    start = 1000 + len(skipped) + len(first)
    assert [(entry.offset, entry.end, entry.block) for entry in scanned.entries] == [
        (1000 + len(skipped), start, 0), (start, start + len(second), 0), (start, start + len(second), 1)]
    assert scanned.end == start + len(second.encode())


def test_Scan_HalfWrittenLastLine_IsLeftForTheNextRead():
    whole = _assistant({"type": "text", "text": "done"})
    half = _assistant({"type": "text", "text": "being written"})[:-9]

    scanned = run_log.scan(CLAUDE, (whole + half).encode())

    assert [entry.text for entry in scanned.entries] == ["done"]
    assert scanned.end == len(whole.encode()), "the read stops where the last whole line ends"


def test_Scan_CodexItemCompleted_ClosesTheToolStartedBefore():
    started = _codex("item.started", "command_execution", id="i3", command="make")
    log = started + _codex("item.completed", "command_execution", id="i3", command="make", exit_code=0)

    scanned = run_log.scan(CODEX, log.encode())

    assert [entry.tool_id for entry in scanned.entries] == ["i3"]
    assert scanned.closed == {"i3": len(log.encode())}


def test_Scan_CodexToolSeenOnlyWhenCompleted_IsClosedAtOnce():
    log = _codex("item.completed", "command_execution", id="i9", command="make")

    scanned = run_log.scan(CODEX, log.encode())

    assert scanned.closed == {"i9": len(log.encode())}


# -- partial messages: the text being written, and what the person sent

def _delta(text, parent=None, kind="text_delta"):
    field = "text" if kind == "text_delta" else "thinking"
    return json.dumps({"type": "stream_event", "parent_tool_use_id": parent,
                       "event": {"type": "content_block_delta", "index": 0, "delta": {"type": kind, field: text}}}) + "\n"


def _message_start():
    return json.dumps({"type": "stream_event", "parent_tool_use_id": None,
                       "event": {"type": "message_start", "message": {"role": "assistant"}}}) + "\n"


def _you(text):
    return json.dumps({"type": "user", "message": {"role": "user", "content": text}}) + "\n"


def test_Scan_TextDeltasAfterTheLastCompletedMessage_FormTheLiveTail():
    done = _assistant({"type": "text", "text": "First."})
    log = done + _message_start() + _delta("Sec") + _delta("ond, ") + _delta("so far")

    scanned = run_log.scan(CLAUDE, log.encode())

    assert scanned.live == "Second, so far"
    assert [entry.text for entry in scanned.entries] == ["First."], "a delta is never an entry"


def test_Scan_TheLiveTailEndsWhenTheMessageCompletes():
    log = _message_start() + _delta("Hello") + _assistant({"type": "text", "text": "Hello there."})

    assert run_log.scan(CLAUDE, log.encode()).live == ""


def test_Scan_ANewMessageStartsANewLiveTail():
    log = _message_start() + _delta("old") + _message_start() + _delta("new")

    assert run_log.scan(CLAUDE, log.encode()).live == "new"


def test_Scan_ThinkingDeltasAndASubagentsDeltasAreNotTheLiveTail():
    log = _message_start() + _delta("secret", kind="thinking_delta") + _delta("sub", parent="toolu_1") + _delta("mine")

    assert run_log.scan(CLAUDE, log.encode()).live == "mine"


def test_Scan_ToolResultEndsTheLiveTail():
    log = _message_start() + _delta("before the tool") + _user_result("t1")

    assert run_log.scan(CLAUDE, log.encode()).live == ""


def test_Scan_SettledStopsBeforeTheDeltasOfTheMessageBeingWritten():
    done = _assistant({"type": "text", "text": "Done."})
    log = _message_start() + done + _message_start() + _delta("half")

    scanned = run_log.scan(CLAUDE, log.encode())

    assert scanned.settled == len(_message_start().encode() + done.encode()), "the next read starts at the last message start"
    assert scanned.end == len(log.encode())


def test_Scan_AllSettled_WhenNoDeltaIsPending():
    log = _assistant({"type": "text", "text": "Done."})

    assert run_log.scan(CLAUDE, log.encode()).settled == len(log.encode())


def test_Scan_UserTextLines_AreYouEntriesWithTheTextAsWritten():
    log = _you("Operator steer:\nuse short names") + _user_result("t1")

    scanned = run_log.scan(CLAUDE, log.encode(), keep_messages=True)

    assert [(entry.kind, entry.text) for entry in scanned.entries] == [("you", "Operator steer:\nuse short names")]


def test_Scan_UserTextAsBlocks_IsOneYouEntry():
    log = json.dumps({"type": "user", "message": {"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}}) + "\n"

    assert [entry.text for entry in run_log.scan(CLAUDE, log.encode(), keep_messages=True).entries] == ["ab"]


def test_Scan_AMessageTheEngineLoggedForACodexResume_IsAYouEntry():
    log = json.dumps({"type": "sakura_message", "text": "check the edge case"}) + "\n" + _codex(
        "item.completed", "agent_message", text="On it.")

    assert [(entry.kind, entry.text) for entry in run_log.scan(CODEX, log.encode(), keep_messages=True).entries] == [
        ("you", "check the edge case"), ("text", "On it.")]


def test_Entries_WithoutAskingForMessages_TheEnginesPromptNeverShows():
    log = _you("the whole role prompt") + json.dumps({"type": "sakura_message", "text": "x"}) + "\n"

    assert run_log.entries(CLAUDE, log) == [] and run_log.entries(CODEX, log) == []


def test_Scan_AUserLineOfASubagent_IsNotAMessageTheEngineOrThePersonSent():
    line = json.dumps({"type": "user", "parent_tool_use_id": "toolu_1", "message": {"role": "user", "content": "agent wrote this"}})

    assert run_log.scan(CLAUDE, (line + "\n").encode(), keep_messages=True).entries == []


# -- the commits an agent's Git refused, and the check's own words

def _bash(tool_id, command):
    return _assistant({"type": "tool_use", "id": tool_id, "name": "Bash", "input": {"command": command}})


def _result(tool_id, text):
    return json.dumps({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tool_id,
                                                                 "content": text}]}}) + "\n"


HINTS = "hint: Support for <GIT_DIR>/info/grafts is deprecated\nhint: and will be removed in a future Git version.\nhint:\n"


def test_RefusedCommits_AFailedGitCommitGivesTheLastLineThatIsNotAGitHint():
    log = (_bash("t1", "git add -A && git commit -m work")
           + _result("t1", "Exit code 1\n" + HINTS + "sakura pre-commit: commits on protected branch 'main' are refused"))

    assert run_log.refused_commits(CLAUDE, log.encode()) == ["commits on protected branch 'main' are refused"]


def test_RefusedCommits_ABrakeOfTheEngineThatRefusedTheCommandGivesItsReason():
    log = _bash("t1", "git commit -am x") + _result("t1", "PreToolUse:Bash hook error: unsupported shell syntax is refused")

    assert run_log.refused_commits(CLAUDE, log.encode()) == ["unsupported shell syntax is refused"]


def test_RefusedCommits_AHookThatCouldNotStart_GivesWhatPythonSaid():
    log = _bash("t1", "git commit -m x") + _result("t1", "Exit code 1\n" + HINTS + "Python: can't open file '/x/y': [Errno 1] Operation not permitted")

    assert run_log.refused_commits(CLAUDE, log.encode()) == ["Python: can't open file '/x/y': [Errno 1] Operation not permitted"]


def test_RefusedCommits_ACommitThatWorkedAndOtherCommandsThatFailedAreNotRefusedCommits():
    log = (_bash("t1", "git commit -m ok") + _result("t1", "[lane abc123] ok\n 1 file changed")
           + _bash("t2", "pytest -q") + _result("t2", "Exit code 1\n1 failed")
           + _bash("t3", "git status") + _result("t3", "Exit code 128\nfatal: not a git repository"))

    assert run_log.refused_commits(CLAUDE, log.encode()) == []


def test_RefusedCommits_OnlyAClaudeLogHasThem():
    assert run_log.refused_commits(CODEX, b'{"item": {"type": "command_execution"}}\n') == []
