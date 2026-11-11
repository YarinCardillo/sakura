#!/usr/bin/env python3
"""Verified container. Says whether every path of the tree of a verified commit can be checked
out as it is on the filesystems people use: the checks of box run on Linux, where names differ
by a single bit, and the person checks the branch out on macOS (or Windows), where two names
that differ only in letter case, or only in how an accented letter is composed, are one file,
and where some names are not what they look like. A tree with such paths is checked as one
thing and checked out as another, so it is refused before a check and before a take.
Prints "ok", or "refused: <why>" for the first problem found. Names only; no file is read.
usage: tree_check.py <sha>"""
import sys
import unicodedata

import boxgit
from boxgit import require_verified, verified_git

# Characters HFS+ ignores in a name: with one of them inside, ".g<200c>it" is ".git" there.
IGNORED = dict.fromkeys(list(range(0x200C, 0x2010)) + list(range(0x202A, 0x202F)) + list(range(0x206A, 0x2070)) + [0xFEFF])
MAX_SHOWN = 120


def fold(name):
    """What a filesystem that ignores case and composition takes the name for."""
    return unicodedata.normalize("NFC", unicodedata.normalize("NFC", name).casefold())


def shown(path):
    return "".join(c if c.isprintable() else "?" for c in path)[:MAX_SHOWN]


def name_problem(name):
    """Why a single name is not safe to check out, or None."""
    plain = fold(name.translate(IGNORED)).rstrip(". ")
    if plain == ".git" or plain.startswith("git~"):
        return "is a name some filesystems take for .git"
    if any(ord(character) in IGNORED for character in name):
        return "holds a code point ignored by HFS+ filesystems"
    if name != name.rstrip(". "):
        return "ends with a dot or a space, which some filesystems drop"
    if ":" in name:
        return "holds a colon, which names a stream of another file on some filesystems"
    return None


def problem(paths):
    seen = {}
    for path in paths:
        parts = path.split("/")
        for depth, name in enumerate(parts):
            why = name_problem(name)
            if why:
                return "the path '%s' %s" % (shown(path), why)
            prefix = "/".join(parts[: depth + 1])
            key = "/".join(fold(part) for part in parts[: depth + 1])
            other = seen.setdefault(key, prefix)
            if other != prefix:
                return ("the paths '%s' and '%s' differ only in letter case or in how a letter is composed: "
                        "they are one file on the filesystem of a Mac" % (shown(other), shown(prefix)))
    return None


def main():
    if len(sys.argv) != 2:
        boxgit.fail("usage: tree_check.py <sha>", 2)
    sha = require_verified(sys.argv[1])
    raw = verified_git("ls-tree", "-r", "-z", "--name-only", "--full-tree", sha)
    why = problem([path for path in raw.split("\0") if path])
    print("refused: " + why if why else "ok")


if __name__ == "__main__":
    main()
