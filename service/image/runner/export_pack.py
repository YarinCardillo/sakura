#!/usr/bin/env python3
"""Workspace container. Writes a pack of the commits after the base, up to a commit, to standard
output. The pack is untrusted bytes: the import container re-hashes every object in it.
usage: export_pack.py <base sha> <commit sha>"""
import subprocess
import sys

import boxgit


def main():
    if len(sys.argv) != 3:
        boxgit.fail("usage: export_pack.py <base sha> <commit sha>", 2)
    base, head = boxgit.full_sha(sys.argv[1]), boxgit.full_sha(sys.argv[2])
    pack = subprocess.run(boxgit.GIT + ["pack-objects", "--revs", "--stdout", "--quiet"], input=(head + "\n^" + base + "\n").encode(),
                          stdout=sys.stdout.buffer, stderr=subprocess.DEVNULL, timeout=boxgit.GIT_TIMEOUT_SECONDS, env=boxgit.hardened_env())
    if pack.returncode != 0:
        boxgit.fail("the workspace could not pack its commits")


main()
