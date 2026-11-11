#!/usr/bin/env python3
"""Prints a unified patch.
usage: diff.py <base sha>                       (workspace container)
       diff.py <base sha> <head sha> <commit>   (verified container)
Without a commit: merge base against the working tree, then every untracked file as a new file,
read from the workspace. With a commit: that verified commit against its first parent; it must be
one of the commits log.py lists for the head.
A file whose patch is over MAX_FILE_BYTES is replaced by its header and a one-line note, and the
whole output is capped at MAX_TOTAL_BYTES."""
import os
import stat
import subprocess
import sys

import boxgit

MAX_FILE_BYTES = 400 * 1024
MAX_TOTAL_BYTES = 8 * 1024 * 1024
MAX_UNTRACKED_FILES = 1000
READ_CHUNK = 1 << 16
DIFF_ARGS = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "-M"]
FILE_START = b"diff --git "
HUNK_START = b"@@"
EXIT_NOT_A_COMMIT = 3


class Output:
    """Writes patches to stdout within the total cap and drops oversized file patches."""

    def __init__(self):
        self.written = 0
        self.full = False
        self.header = []
        self.chunk = []
        self.size = 0
        self.in_header = True

    def feed(self, line, starts_line):
        if starts_line and line.startswith(FILE_START):
            self.flush()
        self.size += len(line)
        if self.in_header and starts_line and line.startswith(HUNK_START):
            self.in_header = False
        if self.in_header:
            self.header.append(line)
        if self.size <= MAX_FILE_BYTES:
            self.chunk.append(line)

    def flush(self):
        if self.size > MAX_FILE_BYTES:
            body = b"".join(self.header[:20]) + b"[patch omitted: %d kB]\n" % (self.size // 1024)
        else:
            body = b"".join(self.chunk)
        self.emit(body)
        self.header, self.chunk, self.size, self.in_header = [], [], 0, True

    def emit(self, body):
        if self.full:
            return
        if self.written + len(body) > MAX_TOTAL_BYTES:
            sys.stdout.buffer.write(b"[output truncated at 8 MB]\n")
            self.full = True
            return
        sys.stdout.buffer.write(body)
        self.written += len(body)


def stream(output, git_args, command=None):
    """Feeds the stdout of one git command into the output, line by line. The command is the
    workspace (working tree questions) or the verified repository (commits)."""
    proc = subprocess.Popen((command or boxgit.GIT) + git_args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            env=boxgit.hardened_env())
    starts_line = True
    for line in iter(lambda: proc.stdout.readline(READ_CHUNK), b""):
        output.feed(line, starts_line)
        starts_line = line.endswith(b"\n")
        if output.full:
            proc.kill()
            break
    proc.wait()
    output.flush()


def untracked_files():
    listing = boxgit.git("ls-files", "--others", "--exclude-standard", "-z")
    paths = [p for p in listing.split("\0") if p and not p.endswith("/")]
    return paths[:MAX_UNTRACKED_FILES]


def is_plain_file_or_link(path):
    """Regular files and symlinks only: a FIFO or device would block the diff forever."""
    mode = os.lstat(os.path.join(boxgit.SRC, path)).st_mode
    return stat.S_ISREG(mode) or stat.S_ISLNK(mode)


def working_tree_patch(output, base):
    stream(output, DIFF_ARGS + [boxgit.workspace_merge_base(base)])
    for path in untracked_files():
        if output.full:
            break
        if is_plain_file_or_link(path):
            stream(output, ["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color",
                            "--", "/dev/null", path])


def main():
    if len(sys.argv) not in (2, 4):
        boxgit.fail("usage: diff.py <base sha> [<head sha> <commit sha>]", 2)
    base = boxgit.full_sha(sys.argv[1])
    output = Output()
    if len(sys.argv) == 4:
        commit = sys.argv[3]
        if commit not in boxgit.commits_ahead(base, sys.argv[2]):
            boxgit.fail("commit is not one of the commits log lists", EXIT_NOT_A_COMMIT)
        stream(output, DIFF_ARGS + [boxgit.parent_or_empty_tree(commit), commit], boxgit.VERIFIED_GIT)
    else:
        working_tree_patch(output, base)
    sys.stdout.flush()


main()
