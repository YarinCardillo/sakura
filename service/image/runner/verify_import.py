#!/usr/bin/env python3
"""Import container. Reads a pack from standard input and puts the commit into the verified
store, re-hashing every object; fails (exit 1) when anything is wrong. The workspace is not
mounted here: nothing the agent configures runs beside the store.
usage: verify_import.py <base sha> <commit sha>"""
import sys

import boxgit


def main():
    if len(sys.argv) != 3:
        boxgit.fail("usage: verify_import.py <base sha> <commit sha>", 2)
    boxgit.import_pack(sys.argv[1], sys.argv[2], sys.stdin.buffer)
    boxgit.require_verified(sys.argv[2])


main()
