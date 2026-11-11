#!/usr/bin/env python3
"""Which files of a checked commit differ from the base of a lavoro on the Mac, as the JSON
object a result carries (changesChecks, checksTouched, testsTouched). The rules are the ones of
image/runner/changes.py, which a task in a box uses; only the source of the list differs: box
runs "git diff --name-status -z --no-renames <base> <commit>" on the worktree and pipes it in.
This script runs no git and reads no file of the worktree.
usage: native_changes.py <subdir> <protected patterns> <test file patterns> [<safe outside patterns>]
(the list on stdin)"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "image", "runner"))
import changes  # noqa: E402


def main():
    if len(sys.argv) not in (4, 5):
        sys.exit("usage: native_changes.py <subdir> <protected patterns> <test file patterns> [<safe outside patterns>]")
    subdir, protected, test_patterns = sys.argv[1:4]
    safe = sys.argv[4] if len(sys.argv) == 5 else ""
    fields = sys.stdin.buffer.read().decode("utf-8", "replace").split("\0")
    files = [(fields[i], fields[i + 1]) for i in range(0, len(fields) - 1, 2)]
    result = changes.checks_fields(files, subdir, protected, safe)
    result["testsTouched"] = changes.tests_touched(files, subdir, test_patterns)
    print(json.dumps(result))


main()
