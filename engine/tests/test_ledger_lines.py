"""The list of ledger line kinds the interface reads (`tests/fixtures/ledger-lines.txt`) is the engine's own."""

import ledger_lines


def test_LedgerLines_Fixture_MatchesTheKindsTheEngineSourceWrites():
    assert ledger_lines.FIXTURE.read_text().splitlines() == ledger_lines.kinds(), (
        "a ledger line kind changed: run python3 tests/ledger_lines.py and copy the file to the interface's fixtures")


def test_LedgerLines_Kinds_CoverEveryPhaseOfALaneRun():
    tags = {line.split("\t")[0] for line in ledger_lines.kinds()}
    assert {"intake", "contract", "build", "evaluate", "report", "review"} <= tags


def test_LedgerLines_Kinds_CoverTheNotifyLinesOfALane():
    notify = {line.split("\t")[1] for line in ledger_lines.kinds() if line.startswith("notify\t")}
    assert {"notify {}: {}", "notify failed: {}", "notify defer failed: {}"} <= notify
    assert any(words.startswith("notify deferred (night): {}") for words in notify)
