#!/usr/bin/env python3
"""Workspace container. What the facts cache needs to know about a workspace without running
git: a key that changes when the commits or the index change, and whether anything in the
working tree changed after a moment.

usage: workspace_key.py <workspace directory> <task>      (prints the key)

It runs inside the sandbox, never on the host: the host reads no path the agent writes. Even
here no path string of the workspace is handed to the kernel whole. The path is walked one
component at a time: every component is opened relative to the descriptor of its parent
(openat) with O_NOFOLLOW, directories with O_DIRECTORY, and fstat on the descriptor says whether
it is what it must be. Bytes are read only from a descriptor that was checked, and at most
MAX_BYTES of them. A symbolic link, a FIFO, a device or an oversized file on the way means
there is no key, and the caller then does not use the cache.

The key is head=<sha> and the change time and size of .git/HEAD, .git/index, .git/packed-refs
and the branch ref ("-" for a file that does not exist)."""
import os
import re
import stat
import sys

MAX_BYTES = 16384
READ_CHUNK_BYTES = 4096
SHA_PATTERN = re.compile(rb"^[0-9a-f]{40}$")
TASK_PATTERN = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
# O_NONBLOCK: opening a FIFO the agent planted must not wait for a writer.
FILE_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
ABSENT = "-"
GIT_DIRECTORY = ".git"


class Refused(Exception):
    """Something on the way is not what git writes."""


def open_directory(name, parent=None):
    """Opens one component as a real directory, relative to the descriptor of its parent."""
    descriptor = os.open(name, DIRECTORY_FLAGS, dir_fd=parent)
    if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
        raise Refused(name)
    return descriptor


def open_plain_file(name, parent):
    """Opens one component as a small regular file. Returns (descriptor, stat), or None when absent."""
    try:
        descriptor = os.open(name, FILE_FLAGS, dir_fd=parent)
    except FileNotFoundError:
        return None
    status = os.fstat(descriptor)
    if not stat.S_ISREG(status.st_mode):
        raise Refused(name)
    return descriptor, status


def read_bounded(descriptor):
    """Reads the whole file from a checked descriptor; refuses more than MAX_BYTES."""
    data = b""
    while len(data) <= MAX_BYTES:
        chunk = os.read(descriptor, READ_CHUNK_BYTES)
        if not chunk:
            return data
        data += chunk
    raise Refused("oversized file")


def describe(opened):
    """Change time and size of an opened file: the agent cannot set the change time back."""
    if opened is None:
        return ABSENT
    return "%d:%d" % (opened[1].st_ctime_ns, opened[1].st_size)


def packed_ref(packed, ref_name):
    """The sha of a ref in an opened packed-refs file, None when it is not there."""
    if packed is None:
        return None
    for line in read_bounded(packed[0]).split(b"\n"):
        fields = line.split(b" ")
        if len(fields) == 2 and fields[1] == ref_name:
            return fields[0]
    return None


def branch_head(git, task, head, packed):
    """The commit of box/<task>, from the loose ref or else packed-refs; also returns the opened ref."""
    if head is None or read_bounded(head[0]).rstrip(b"\n") != b"ref: refs/heads/box/" + task.encode():
        raise Refused("HEAD")
    box = open_directory("box", open_directory("heads", open_directory("refs", git)))
    ref = open_plain_file(task, box)
    sha = read_bounded(ref[0]).rstrip(b"\n") if ref else packed_ref(packed, b"refs/heads/box/" + task.encode())
    if sha is None or not SHA_PATTERN.match(sha):
        raise Refused("branch ref")
    return sha.decode(), ref


def workspace_key(workspace, task):
    """The key, or None when something on the way is not what git writes."""
    if not TASK_PATTERN.match(task):
        return None
    try:
        git = open_directory(GIT_DIRECTORY, open_directory(workspace))
        head = open_plain_file("HEAD", git)
        index = open_plain_file("index", git)
        packed = open_plain_file("packed-refs", git)
        sha, ref = branch_head(git, task, head, packed)
    except (OSError, Refused):
        return None
    return "head=%s %s" % (sha, " ".join(describe(opened) for opened in (head, index, packed, ref)))


def changed_since(workspace, reference_ns):
    """True when a file or directory of the working tree (outside .git) had its inode changed
    after the moment: an edit, a new or a deleted file. Change time, not modification time,
    which the agent can set. Nothing is opened and no link is followed."""
    try:
        if os.lstat(workspace).st_ctime_ns > reference_ns:
            return True
    except OSError:
        return True
    pending = [workspace]
    while pending:
        directory = pending.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError:
            return True
        for entry in entries:
            if directory == workspace and entry.name == GIT_DIRECTORY:
                continue
            try:
                status = entry.stat(follow_symlinks=False)
            except OSError:
                return True
            if status.st_ctime_ns > reference_ns:
                return True
            if stat.S_ISDIR(status.st_mode):
                pending.append(entry.path)
    return False


def main(arguments):
    if len(arguments) != 2:
        print("usage: workspace_key.py <workspace directory> <task>", file=sys.stderr)
        return 2
    key = workspace_key(arguments[0], arguments[1])
    if key is None:
        print("workspace_key: refused", file=sys.stderr)
        return 1
    print(key)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
