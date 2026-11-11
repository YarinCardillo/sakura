#!/usr/bin/env python3
"""Drop container. Says what the entries of a box's drop directory are, for the names it is
given; the directory is mounted read-only at /drops and nothing else is. A request is an EMPTY
regular file whose name is a verb; this prints one line per name that looks like a verb and is
there: "<name> empty", "<name> nonempty" or "<name> other" (a link, a directory, a FIFO).
Nothing is opened, no link is followed and the directory is not listed: only lstat of the
names. The host picks the names, within its budget for one time round.
usage: drops.py <name>..."""
import os
import re
import stat
import sys

DROPS = os.environ.get("BOX_DROPS", "/drops")
VERB_PATTERN = re.compile(r"[a-z][a-z0-9-]*")
MAX_NAMES = 1000


def kind(name):
    status = os.lstat(os.path.join(DROPS, name))
    if not stat.S_ISREG(status.st_mode):
        return "other"
    return "empty" if status.st_size == 0 else "nonempty"


def main():
    for name in sys.argv[1 : MAX_NAMES + 1]:
        if not VERB_PATTERN.fullmatch(name):
            continue
        try:
            print(name, kind(name))
        except OSError:
            continue


main()
