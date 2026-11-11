#!/usr/bin/env python3
"""Verified container. Prints which files of the checked commit differ from the task's base, as
one JSON object. The comparison runs in the verified store, where the workspace is not mounted: the base
and its trees come from the trusted source repository, the checked commit was imported with every object
re-hashed (boxgit.import_pack), and replace objects, grafts, refs, config, attributes and auxiliary files
of the workspace do not exist here. Both shas must be full ids of commit objects, the checked one a
verified commit; otherwise the script fails and the caller records the commit as a change.
usage: changes.py <base sha> <checked sha> <subdir> <protected patterns> <test file patterns> [<safe outside patterns>]
Empty pattern lists give null for their fields. Patterns are separated by whitespace: "*" stays inside one
directory name, "**" crosses directories. A protected or test file pattern is tried on the path inside
<subdir> and on the path from the root of the repository, so a pattern protects a file wherever it lies.
A commit that changes any path outside <subdir> counts as a change to the checks, unless every such path
matches a safe outside pattern (tried on the path from the root): the command of a check can call a file
of the repository that lies outside the folder of the project, and what is not looked at is not known
to be harmless. Only names are compared, no file content is read."""
import json
import re
import sys

import boxgit
from boxgit import require_trusted_commit, require_verified, verified_git

MAX_LISTED = 20


def pattern_to_regex(pattern):
    """Translates a glob (* ? **) to a regular expression for a whole path."""
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out) + "$")


def changed_files(base, sha):
    """[(status, path)] of the tree difference; a rename counts as a delete plus an add."""
    raw = verified_git("diff", "--name-status", "-z", "--no-renames", "--no-ext-diff", "--no-textconv", base, sha)
    fields = raw.split("\0")
    return [(fields[i], fields[i + 1]) for i in range(0, len(fields) - 1, 2)]


def relative_to(subdir, path):
    """The path inside the subdirectory, or None when it lies outside."""
    prefix = subdir.strip("/") + "/" if subdir.strip("/") else ""
    return path[len(prefix):] if path.startswith(prefix) else None


def matching(files, subdir, patterns):
    """The changed files a pattern names, by their path inside the subdirectory or from the root."""
    regexes = [pattern_to_regex(p) for p in patterns.split()]
    result = []
    for status, path in files:
        inside = relative_to(subdir, path)
        if any(r.match(path) or (inside is not None and r.match(inside)) for r in regexes):
            result.append((status, path))
    return result


def outside(files, subdir, safe):
    """The changed paths outside the subdirectory that no safe outside pattern names."""
    regexes = [pattern_to_regex(p) for p in safe.split()]
    return sorted(path for _status, path in files
                  if relative_to(subdir, path) is None and not any(r.match(path) for r in regexes))


OUTSIDE_REASON = "the commit changes files outside the folder of the project"


def checks_fields(files, subdir, protected, safe=""):
    if not protected.split():
        return {"changesChecks": None, "checksTouched": []}
    touched = sorted(path for _status, path in matching(files, subdir, protected))
    beyond = [path for path in outside(files, subdir, safe) if path not in touched]
    fields = {"changesChecks": bool(touched or beyond), "checksTouched": (touched + beyond)[:MAX_LISTED]}
    if beyond:
        fields["checksReason"] = OUTSIDE_REASON
    return fields


def tests_touched(files, subdir, test_patterns):
    """Existing test files the commit modified or deleted. A file that git reports as modified or
    deleted exists at the base, a new file is reported as added and does not count."""
    if not test_patterns.split():
        return None
    hits = [(status, path) for status, path in matching(files, subdir, test_patterns) if status[0] in "MTD"]
    deleted = sorted(path for status, path in hits if status[0] == "D")
    modified = sorted(path for status, path in hits if status[0] != "D")
    return {"modified": len(modified), "deleted": len(deleted), "files": sorted(deleted + modified)[:MAX_LISTED]}


def main():
    if len(sys.argv) not in (6, 7):
        boxgit.fail("usage: changes.py <base sha> <checked sha> <subdir> <protected patterns> <test file patterns> [<safe outside patterns>]", 2)
    base, sha, subdir, protected, test_patterns = sys.argv[1:6]
    safe = sys.argv[6] if len(sys.argv) == 7 else ""
    files = changed_files(require_trusted_commit(base), require_verified(sha))
    fields = checks_fields(files, subdir, protected, safe)
    fields["testsTouched"] = tests_touched(files, subdir, test_patterns)
    print(json.dumps(fields))


if __name__ == "__main__":
    main()
