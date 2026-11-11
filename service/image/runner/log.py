#!/usr/bin/env python3
"""Verified container. Prints the verified commits after the base, up to the head, as a JSON
array, newest first, at most 200.
usage: log.py <base sha> <head sha>"""
import json
import sys

import boxgit
from boxgit import verified_git

MAX_SUBJECT = 200


def describe(sha):
    subject, time = verified_git("show", "-s", "--format=%s%x1f%cI", sha).rstrip("\n").split("\x1f")
    files, insertions, deletions = boxgit.numstat(boxgit.parent_or_empty_tree(sha), sha, reader=verified_git)
    return {
        "sha": sha, "subject": subject[:MAX_SUBJECT], "time": boxgit.utc(time),
        "filesChanged": files, "insertions": insertions, "deletions": deletions,
    }


def main():
    if len(sys.argv) != 3:
        boxgit.fail("usage: log.py <base sha> <head sha>", 2)
    print(json.dumps([describe(sha) for sha in boxgit.commits_ahead(boxgit.full_sha(sys.argv[1]), sys.argv[2])]))


main()
