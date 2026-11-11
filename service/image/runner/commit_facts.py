#!/usr/bin/env python3
"""Verified container. Prints what the verified store knows about the head of a task, as one
JSON object: its subject, its time and how many commits it is ahead of the base.
usage: commit_facts.py <base sha> <head sha>"""
import json
import sys

import boxgit
from boxgit import verified_git

MAX_SUBJECT = 200


def main():
    if len(sys.argv) != 3:
        boxgit.fail("usage: commit_facts.py <base sha> <head sha>", 2)
    base, head = boxgit.full_sha(sys.argv[1]), boxgit.require_verified(sys.argv[2])
    subject, time = verified_git("log", "-1", "--format=%s%x1f%cI", head).rstrip("\n").split("\x1f")
    print(json.dumps({
        "headSubject": subject[:MAX_SUBJECT],
        "headTime": boxgit.utc(time),
        "ahead": int(verified_git("rev-list", "--count", base + ".." + head).strip()),
    }))


main()
