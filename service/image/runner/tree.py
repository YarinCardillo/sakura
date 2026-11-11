#!/usr/bin/env python3
"""Prints one directory level of a lavoro as a JSON object: for each entry its name, its kind
(file, dir, link, other, or deleted for a file of the base that is gone), whether it differs
from the base and, for a file, the lines added and removed. Files git ignores are left out
unless the third argument is 1.
usage: tree.py <base sha> <directory inside the lavoro, "" for its root> <all: 0|1>"""
import json
import os
import stat
import sys

import boxgit
import lavoro

MAX_ENTRIES = 2000


def kind_of(mode):
    if stat.S_ISLNK(mode):
        return "link"
    if stat.S_ISDIR(mode):
        return "dir"
    return "file" if stat.S_ISREG(mode) else "other"


def listed(directory, at_root):
    """{name: kind} of what is in the open directory, without the git metadata of the lavoro."""
    entries = {}
    with os.scandir(directory) as scan:
        for entry in scan:
            if at_root and entry.name == ".git":
                continue
            entries[entry.name] = kind_of(entry.stat(follow_symlinks=False).st_mode)
    return entries


def ignored(prefix, entries):
    """The names of this level that git ignores (a tracked file is never ignored)."""
    if not entries:
        return set()
    names = sorted(entries)
    paths = "\0".join(prefix + name + ("/" if entries[name] == "dir" else "") for name in names)
    found = lavoro.git("check-ignore", "-z", "--stdin", check=False, stdin=paths.encode("utf-8", "surrogateescape"))
    hits = [lavoro.text(hit).rstrip("/") for hit in found.split(b"\0") if hit]
    return {hit[len(prefix):] for hit in hits}


def first_part(path, prefix):
    """(the name at this level, whether the path is that entry itself) of a path under prefix."""
    rest = path[len(prefix):].rstrip("/")
    name, _, deeper = rest.partition("/")
    return name, not deeper and not path.endswith("/")


def committed_changes(base, prefix):
    """{name: [added, removed, is the entry itself]} from the difference to the base."""
    changes = {}
    pathspec = ["--", prefix] if prefix else []
    raw = lavoro.git("diff", "--numstat", "-z", "--no-renames", "--no-ext-diff", "--no-textconv", lavoro.merge_base(base), *pathspec)
    for record in raw.split(b"\0"):
        fields = lavoro.text(record).split("\t", 2)
        if len(fields) != 3 or not fields[2].startswith(prefix):
            continue
        name, itself = first_part(fields[2], prefix)
        added = int(fields[0]) if fields[0].isdigit() else None
        removed = int(fields[1]) if fields[1].isdigit() else None
        changes[name] = [added, removed, itself] if itself else [None, None, False]
    return changes


def untracked(prefix):
    """The names of this level that hold something git does not know yet."""
    pathspec = ["--", prefix] if prefix else []
    raw = lavoro.git("ls-files", "-z", "--others", "--exclude-standard", "--directory", *pathspec)
    return {first_part(lavoro.text(path), prefix) for path in raw.split(b"\0") if path}


def describe(name, kind, change, new, directory):
    entry = {"name": name, "kind": kind, "changed": bool(change) or new is not None, "added": None, "removed": None}
    if change and change[2]:
        entry["added"], entry["removed"] = change[0], change[1]
    elif new and kind == "file":
        entry["added"], entry["removed"] = lavoro.count_lines(directory, name), 0
    return entry


def main():
    if len(sys.argv) != 4 or sys.argv[3] not in ("0", "1"):
        boxgit.fail("usage: tree.py <base sha> <directory> <all: 0|1>", 2)
    base = boxgit.full_sha(sys.argv[1])
    path = "/".join(lavoro.plain_parts(sys.argv[2]))
    directory = lavoro.open_inside(path, directory=True)
    prefix = path + "/" if path else ""
    entries = listed(directory, not path)
    if sys.argv[3] == "0":
        for name in ignored(prefix, entries):
            entries.pop(name, None)
    changes = committed_changes(base, prefix)
    news = {name: itself for name, itself in untracked(prefix)}
    for name, change in changes.items():
        if name not in entries and change[2]:
            entries[name] = "deleted"
    names = sorted(entries)
    result = [describe(name, entries[name], changes.get(name), news.get(name), directory) for name in names[:MAX_ENTRIES]]
    print(json.dumps({"path": path, "entries": result, "truncated": len(names) > MAX_ENTRIES}))


main()
