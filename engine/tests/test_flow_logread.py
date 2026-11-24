"""Reading a run's log back is bounded and only ever reads a regular file."""

import os
import signal

import pytest

from sakura.flow import logread


def test_a_small_log_is_returned_whole(tmp_path):
    log = tmp_path / "run.log"
    log.write_text("one\ntwo\n")
    assert logread.read_log(log) == "one\ntwo\n"


def test_a_log_larger_than_the_limit_returns_only_its_tail(tmp_path):
    log = tmp_path / "run.log"
    log.write_text("head " * 1000 + "SAKURA_RESULT {}\n")
    text = logread.read_log(log, limit=64)
    assert len(text.encode()) <= 64 and text.endswith("SAKURA_RESULT {}\n")


def test_a_missing_log_raises_file_not_found(tmp_path):
    with pytest.raises(FileNotFoundError):
        logread.read_log(tmp_path / "absent.log")


def test_a_symlinked_log_is_refused(tmp_path):
    target = tmp_path / "secret.txt"
    target.write_text("secret\n")
    log = tmp_path / "run.log"
    log.symlink_to(target)
    with pytest.raises(logread.UnreadableLog):
        logread.read_log(log)


def test_a_directory_in_place_of_a_log_is_refused(tmp_path):
    (tmp_path / "run.log").mkdir()
    with pytest.raises(logread.UnreadableLog):
        logread.read_log(tmp_path / "run.log")


def test_a_fifo_in_place_of_a_log_is_refused_without_blocking(tmp_path):
    log = tmp_path / "run.log"
    os.mkfifo(log)
    previous = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(AssertionError("the read blocked")))
    signal.alarm(5)
    try:
        with pytest.raises(logread.UnreadableLog):
            logread.read_log(log)
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
