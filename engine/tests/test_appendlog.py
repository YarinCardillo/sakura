import json
import os
import stat
import subprocess
import sys
from pathlib import Path

from sakura import appendlog

LIB = str(Path(__file__).resolve().parents[1] / "lib")
WRITER = """
import sys
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from sakura import appendlog
for number in range(int(sys.argv[3])):
    appendlog.append(Path(sys.argv[2]), {"who": sys.argv[4], "n": number})
"""


def test_AppendLog_TwoProcessesAtOnce_SeqsContiguousNoLineLost(tmp_path):
    log = tmp_path / "log.jsonl"
    workers = [subprocess.Popen([sys.executable, "-c", WRITER, LIB, str(log), "200", name]) for name in ("a", "b")]
    assert [worker.wait(timeout=120) for worker in workers] == [0, 0]
    entries = appendlog.read(log)
    assert [entry["seq"] for entry in entries] == list(range(1, 401))
    assert sum(1 for entry in entries if entry["who"] == "a") == 200
    assert len(log.read_text().splitlines()) == 400


def test_AppendLog_TornLastLine_CutBeforeNextAppend(tmp_path):
    log = tmp_path / "log.jsonl"
    appendlog.append(log, {"x": 1})
    with open(log, "ab") as stream:
        stream.write(b'{"seq": 2, "x"')
    assert appendlog.read(log) == [{"seq": 1, "x": 1}]
    assert appendlog.append(log, {"x": 3}) == 2
    assert [entry["x"] for entry in appendlog.read(log)] == [1, 3]
    assert len(log.read_text().splitlines()) == 2


def test_AppendLog_ReadAfter_OnlyLaterEntriesOldestFirst(tmp_path):
    log = tmp_path / "log.jsonl"
    for number in range(5):
        appendlog.append(log, {"n": number})
    assert [entry["n"] for entry in appendlog.read(log, after=2)] == [2, 3, 4]
    assert appendlog.read(log, after=5) == []
    assert appendlog.last_seq(log) == 5


def test_AppendLog_FileMissing_ReadsEmptyAndCreatesNothing(tmp_path):
    folder = tmp_path / "empty"
    folder.mkdir()
    log = folder / "log.jsonl"
    assert appendlog.read(log) == [] and appendlog.last_seq(log) == 0
    assert list(folder.iterdir()) == []


def test_AppendLog_LineThatDoesNotParse_SkippedAndSeqContinues(tmp_path):
    log = tmp_path / "log.jsonl"
    appendlog.append(log, {"x": 1})
    with open(log, "ab") as stream:
        stream.write(b"not json\n")
    assert appendlog.append(log, {"x": 2}) == 2
    assert [entry["x"] for entry in appendlog.read(log)] == [1, 2]


def test_AppendLog_NewFile_Mode0600(tmp_path):
    log = tmp_path / "log.jsonl"
    appendlog.append(log, {"x": 1})
    assert stat.S_IMODE(os.stat(log).st_mode) == 0o600
    assert json.loads(log.read_text().splitlines()[0]) == {"seq": 1, "x": 1}


# -- an allowance per class of writer and reads that never shorten a history ------------------------------------------


def _is_agent(entry):
    return entry.get("who") == "agent"


def test_RequireRoom_ClassAtItsAllowance_RefusedWhileTheOtherClassStillFits(tmp_path):
    log = tmp_path / "log.jsonl"
    allowance = appendlog.Allowance(entries=2, bytes=10_000)
    for number in range(2):
        appendlog.require_room(log, {"who": "agent", "n": number}, _is_agent, allowance, 100_000)
        appendlog.append(log, {"who": "agent", "n": number})

    try:
        appendlog.require_room(log, {"who": "agent", "n": 2}, _is_agent, allowance, 100_000)
    except appendlog.LogFull as error:
        assert "2 entries" in str(error)
    else:
        raise AssertionError("a third agent line passed an allowance of two")
    appendlog.require_room(log, {"who": "person"}, lambda entry: not _is_agent(entry), allowance, 100_000)


def test_RequireRoom_CountsTheBytesTheLineTakesOnDisk(tmp_path):
    log = tmp_path / "log.jsonl"
    record = {"who": "agent", "text": "\U0001f600" * 10}  # each character is written as two `\u` escapes
    room = appendlog.line_bytes({"seq": 1, **record})
    assert room == len(json.dumps({"seq": 1, **record}, separators=(",", ":")).encode()) + 1
    appendlog.require_room(log, record, _is_agent, appendlog.Allowance(5, room), 100_000)
    try:
        appendlog.require_room(log, record, _is_agent, appendlog.Allowance(5, room - 1), 100_000)
    except appendlog.LogFull:
        pass
    else:
        raise AssertionError("a line one byte over the allowance passed")


def test_ReadWhole_LogOverTheReadBound_IsBrokenNeverShortened(tmp_path):
    log = tmp_path / "log.jsonl"
    appendlog.append(log, {"who": "person", "answer": "ok"})
    with open(log, "ab") as stream:  # grown before any cap
        stream.write(b'{"seq":2,"text":"' + b"x" * 500 + b'"}\n')
    appendlog.append(log, {"who": "person", "answer": "the tail"})

    for call in (lambda: appendlog.read_whole(log, 300),
                 lambda: appendlog.require_room(log, {"who": "agent"}, _is_agent, appendlog.Allowance(9, 9_999), 300)):
        try:
            call()
        except appendlog.LogBroken as error:
            assert "bytes" in str(error)
        else:
            raise AssertionError("a history over the read bound was read shortened")
    assert [entry["seq"] for entry in appendlog.read_whole(log, 10_000)] == [1, 2, 3]


def test_ReadWhole_MissingLog_Empty(tmp_path):
    assert appendlog.read_whole(tmp_path / "none.jsonl", 100) == []


def test_Append_RoomRefusesUnderTheLock_NothingWritten(tmp_path):
    log = tmp_path / "log.jsonl"
    appendlog.append(log, {"who": "agent"})
    allowance = appendlog.Allowance(entries=1, bytes=10_000)

    def room():
        appendlog.require_room(log, {"who": "agent"}, _is_agent, allowance, 100_000)

    try:
        appendlog.append(log, {"who": "agent"}, room=room)
    except appendlog.LogFull:
        pass
    else:
        raise AssertionError("an append past the allowance was written")
    assert [entry["seq"] for entry in appendlog.read(log)] == [1]
