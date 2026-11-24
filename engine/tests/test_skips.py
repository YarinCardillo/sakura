from __future__ import annotations

from sakura import skips


def test_parse_pytest_skip_lines():
    output = (
        "SKIPPED [1] tests/test_db.py:4: needs DEMO_DB\n"
        "SKIPPED [2] tests/test_web.py:9: no browser\n"
        "3 skipped, 1 passed in 0.02s\n"
    )
    report = skips.parse(output)
    assert report.count == 3
    assert report.entries == [
        skips.SkipEntry("tests/test_db.py", "needs DEMO_DB"),
        skips.SkipEntry("tests/test_web.py", "no browser"),
    ]


def test_parse_vitest_and_jest_counts():
    assert skips.parse("Tests  2 skipped | 5 passed (7)\n").count == 2
    assert skips.parse("Tests:       4 skipped, 9 passed, 13 total\n").count == 4
    assert skips.parse("5 passed in 0.1s\n").count == 0


def test_unreported_reasons_fail_closed():
    report = skips.parse("Tests:       2 skipped, 9 passed, 11 total\n")
    assert report.entries == [skips.SkipEntry("", "reason not reported")]
