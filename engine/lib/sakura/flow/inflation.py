"""Detects raised timeouts and retry counts, and added retry wrappers, between two versions of a text."""

from __future__ import annotations

import re

LIMIT_NUMBER = re.compile(r"(?:timeout|retries|retry)\w*[\"']?\s*[=:(,]?\s*(\d+(?:\.\d+)?)", re.IGNORECASE)
RETRY_WRAPPERS = ("@pytest.mark.flaky", "--reruns", "retryTimes", "test.retry", "this.retries")


def inflates(old: str, new: str) -> bool:
    """Whether a timeout or retry number grew, or a retry wrapper appeared, going from `old` to `new`."""
    return _number_grew(old, new) or _wrapper_added(old, new)


def timeout_only_increase(old: str, new: str) -> bool:
    """Whether the only source changes are increases to existing timeout or retry numbers."""
    before = list(LIMIT_NUMBER.finditer(old))
    after = list(LIMIT_NUMBER.finditer(new))
    if not before or len(before) != len(after):
        return False

    increased = False
    old_spans = []
    new_spans = []
    for earlier, latest in zip(before, after):
        old_span, old_value = _number_span(earlier)
        new_span, new_value = _number_span(latest)
        if new_value < old_value:
            return False
        increased |= new_value > old_value
        old_spans.append(old_span)
        new_spans.append(new_span)

    return increased and _without_numbers(old, old_spans) == _without_numbers(new, new_spans)


def _number_span(match: re.Match[str]) -> tuple[tuple[int, int], float]:
    number = re.search(r"\d+(?:\.\d+)?$", match.group())
    assert number is not None
    start = match.start() + number.start()
    return (start, start + len(number.group())), float(number.group())


def _without_numbers(text: str, spans: list[tuple[int, int]]) -> str:
    for start, end in reversed(spans):
        text = text[:start] + "<limit>" + text[end:]
    return text


def _number_grew(old: str, new: str) -> bool:
    before = [float(value) for value in LIMIT_NUMBER.findall(old)]
    after = [float(value) for value in LIMIT_NUMBER.findall(new)]
    return any(latest > earlier for earlier, latest in zip(before, after))


def _wrapper_added(old: str, new: str) -> bool:
    return any(new.count(marker) > old.count(marker) for marker in RETRY_WRAPPERS)
