#!/usr/bin/env python3
"""Workspace container. Prints what only the workspace knows, as one JSON object: the commit
and the branch HEAD names and the figures of the working tree. All of it is the agent's word
(the workspace's git configuration can run a program during status and diff): it is shown, and
the head is used only after the import container has verified that commit.

usage: facts.py <base sha> <task> [<cached key> <moment of the cached read, in nanoseconds>]

With a cached key it first asks, without running git, whether the workspace is as it was at the
cached read (the same key, nothing in the working tree changed after that moment). Then it
prints {"unchanged": true} and stops. Otherwise it prints the facts with "key": the key for the
cache, or null when the workspace has none or the key moved while the facts were read."""
import json
import sys

import boxgit
import workspace_key
from boxgit import git

UNCHANGED = {"unchanged": True}


def is_unchanged(key, arguments):
    """True when the caller's cached read still describes the workspace."""
    if len(arguments) != 2 or key is None or key != arguments[0] or not arguments[1].isdigit():
        return False
    return not workspace_key.changed_since(boxgit.SRC, int(arguments[1]))


def main():
    if len(sys.argv) not in (3, 5):
        boxgit.fail("usage: facts.py <base sha> <task> [<cached key> <moment in nanoseconds>]", 2)
    base, task = boxgit.full_sha(sys.argv[1]), sys.argv[2]
    key = workspace_key.workspace_key(boxgit.SRC, task)
    if is_unchanged(key, sys.argv[3:]):
        print(json.dumps(UNCHANGED))
        return
    head = boxgit.workspace_head()
    files, insertions, deletions = boxgit.numstat(boxgit.workspace_merge_base(base), reader=git)
    facts = {
        "head": head,
        "branch": git("rev-parse", "--abbrev-ref", "HEAD").strip(),
        "dirtyFiles": len(git("status", "--porcelain", "-z").split("\0")) - 1,
        "filesChanged": files,
        "insertions": insertions,
        "deletions": deletions,
    }
    facts["key"] = key if key == workspace_key.workspace_key(boxgit.SRC, task) else None
    print(json.dumps(facts))


main()
