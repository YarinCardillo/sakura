"""Test-pattern matching with `**`."""

from sakura.flow import globs

PATTERNS = ["tests/**", "e2e/**", "**/test_*.py", "**/*.spec.*"]


def test_matches_any_covers_nested_and_top_level_files():
    for name in ("tests/a/b.py", "e2e/x.py", "test_top.py", "pkg/sub/test_x.py", "ui/app.spec.ts"):
        assert globs.matches_any(name, PATTERNS), name


def test_matches_any_rejects_other_files():
    for name in ("app.py", "src/testing.py", "docs/tests.md", "xe2e/x.py"):
        assert not globs.matches_any(name, PATTERNS), name
