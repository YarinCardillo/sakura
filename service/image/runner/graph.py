#!/usr/bin/env python3
"""Verified container. Prints the verified commits of a task after its base, up to the head, each
with its parents, as a JSON array, newest first, at most 200: what box graph draws for a task in
a box.
usage: graph.py <base sha> <head sha>"""
import json
import sys

import boxgit
from boxgit import verified_git

MAX_SUBJECT = 200


def main():
    if len(sys.argv) != 3:
        boxgit.fail("usage: graph.py <base sha> <head sha>", 2)
    base, head = boxgit.full_sha(sys.argv[1]), boxgit.require_verified(sys.argv[2])
    raw = verified_git("log", "--max-count=%d" % boxgit.MAX_LOG_COMMITS, "--format=%H%x1f%P%x1f%ct%x1f%s", base + ".." + head)
    commits = []
    for line in raw.splitlines():
        sha, parents, time, subject = line.split("\x1f", 3)
        commits.append({"sha": sha, "parents": parents.split(), "time": int(time), "subject": subject[:MAX_SUBJECT]})
    print(json.dumps(commits))


main()
