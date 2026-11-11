#!/usr/bin/env python3
"""Writes the bytes of one file of a lavoro to standard output: the file of the working tree,
or with a commit the file as that commit has it. Nothing is written unless all of it can be:
the exit code says why (3 no such file, 4 the path leaves the lavoro or is a link that does,
82 the file is larger than MAX_FILE_BYTES).
usage: file.py <path inside the lavoro> [<commit sha>]"""
import os
import stat
import subprocess
import sys

import boxgit
import lavoro

REGULAR_MODES = ("100644", "100755")


def from_working_tree(path):
    """The bytes of a regular file of the working tree, opened through directory descriptors
    (a link is followed only inside the lavoro) and examined on its descriptor."""
    descriptor = lavoro.open_inside(path)
    status = os.fstat(descriptor)
    if not stat.S_ISREG(status.st_mode):
        boxgit.fail("not a file", lavoro.EXIT_NOT_FOUND)
    if status.st_size > lavoro.MAX_FILE_BYTES:
        boxgit.fail("the file is too big", lavoro.EXIT_TOO_BIG)
    return lavoro.read_bounded(descriptor)


def from_commit(sha, path):
    """The bytes of the blob a commit has at the path; a link or a directory there is refused."""
    try:
        listing = lavoro.git("ls-tree", "-z", boxgit.full_sha(sha), "--", path)
    except subprocess.CalledProcessError:
        boxgit.fail("no such commit in the lavoro", lavoro.EXIT_NOT_FOUND)
    records = [record for record in listing.split(b"\0") if record]
    if len(records) != 1:
        boxgit.fail("no such file at that commit", lavoro.EXIT_NOT_FOUND)
    mode, kind, object_id = records[0].split(b"\t", 1)[0].decode().split(" ")
    if kind != "blob" or mode not in REGULAR_MODES:
        boxgit.fail("not a regular file at that commit", lavoro.EXIT_OUTSIDE)
    if int(lavoro.git("cat-file", "-s", object_id).decode().strip()) > lavoro.MAX_FILE_BYTES:
        boxgit.fail("the file is too big", lavoro.EXIT_TOO_BIG)
    return lavoro.git("cat-file", "blob", object_id)


def main():
    if len(sys.argv) not in (2, 3) or not sys.argv[1]:
        boxgit.fail("usage: file.py <path> [<commit sha>]", 2)
    path = "/".join(lavoro.plain_parts(sys.argv[1]))
    if not path:
        boxgit.fail("not a file", lavoro.EXIT_NOT_FOUND)
    data = from_commit(sys.argv[2], path) if len(sys.argv) == 3 else from_working_tree(path)
    if len(data) > lavoro.MAX_FILE_BYTES:
        boxgit.fail("the file is too big", lavoro.EXIT_TOO_BIG)
    sys.stdout.buffer.write(data)
    sys.stdout.flush()


main()
