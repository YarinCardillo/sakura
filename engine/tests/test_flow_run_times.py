"""When each part of a run's log was written: the marks the runner keeps next to the log."""

from datetime import datetime, timedelta, timezone

from sakura.flow import run_times

START = datetime(2026, 11, 19, 10, 0, 0, tzinfo=timezone.utc)


def _iso(seconds: float) -> str:
    return (START + timedelta(seconds=seconds)).isoformat(timespec="seconds")


def test_TimeAt_FirstMarkAtOrPastTheOffset_ReturnsItsTime(tmp_path):
    log = tmp_path / "build-1.log"
    for size, seconds in ((100, 1), (250, 2), (900, 5)):
        run_times.record(log, size, START + timedelta(seconds=seconds))

    assert run_times.time_at(log, 0) == _iso(1)
    assert run_times.time_at(log, 100) == _iso(1)
    assert run_times.time_at(log, 101) == _iso(2)
    assert run_times.time_at(log, 900) == _iso(5)


def test_TimeAt_OffsetPastTheLastMark_ReturnsNothing(tmp_path):
    log = tmp_path / "build-1.log"
    run_times.record(log, 100, START)

    assert run_times.time_at(log, 101) == ""


def test_TimeAt_NoMarksFile_ReturnsNothing(tmp_path):
    assert run_times.time_at(tmp_path / "build-1.log", 5) == ""


def test_TimeAt_ManyMarks_ReadsFewBytes(tmp_path):
    log = tmp_path / "build-1.log"
    for index in range(1, 5001):
        run_times.record(log, index * 10, START + timedelta(seconds=index))
    reads = []
    original_read = run_times._read_mark

    def counting_read(stream, index):
        reads.append(index)
        return original_read(stream, index)

    run_times._read_mark = counting_read
    try:
        assert run_times.time_at(log, 25001) == _iso(2501)
    finally:
        run_times._read_mark = original_read
    assert len(reads) <= 20, "a binary search, not a scan"


def test_TimeAt_TornOrForeignLine_IsIgnored(tmp_path):
    log = tmp_path / "build-1.log"
    run_times.record(log, 100, START)
    with run_times.times_path(log).open("ab") as stream:
        stream.write(b"not a mark at all\n")

    assert run_times.time_at(log, 50) == _iso(0)
    assert run_times.time_at(log, 150) == ""


def test_Marker_GrowthIsMarkedAndQuietIsNot(tmp_path):
    log = tmp_path / "build-1.log"
    log.write_bytes(b"x" * 10)
    clock = [START]
    marker = run_times.Marker(log, now=lambda: clock[0])

    marker.tick()
    assert run_times.time_at(log, 10) == _iso(0)
    clock[0] = START + timedelta(seconds=3)
    marker.tick()
    assert run_times.times_path(log).stat().st_size == run_times.MARK_BYTES, "no growth, no mark"
    log.write_bytes(b"x" * 30)
    marker.tick()
    assert run_times.time_at(log, 11) == _iso(3)


def test_Marker_GrowthInsideTheMinimumGap_IsMarkedOnALaterTickWithoutNewGrowth(tmp_path):
    log = tmp_path / "build-1.log"
    log.write_bytes(b"x" * 10)
    clock = [START]
    marker = run_times.Marker(log, now=lambda: clock[0])
    marker.tick()
    log.write_bytes(b"x" * 40)
    clock[0] = START + timedelta(seconds=0.1)
    marker.tick()
    assert run_times.time_at(log, 40) == "", "too soon after the last mark"

    clock[0] = START + timedelta(seconds=1)
    marker.tick()

    assert run_times.time_at(log, 40) == _iso(1), "a tool that then stays silent still gets its time"


def test_Marker_Flush_MarksTheLastGrowthWhateverTheGap(tmp_path):
    log = tmp_path / "build-1.log"
    log.write_bytes(b"x" * 10)
    clock = [START]
    marker = run_times.Marker(log, now=lambda: clock[0])
    marker.tick()
    log.write_bytes(b"x" * 40)
    clock[0] = START + timedelta(seconds=0.1)

    marker.flush()

    assert run_times.time_at(log, 40) == _iso(0), "the second only, but marked"
    assert run_times.times_path(log).stat().st_size == 2 * run_times.MARK_BYTES
