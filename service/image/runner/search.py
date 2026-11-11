#!/usr/bin/env python3
"""Prints the files of a lavoro whose path contains a text, for quick open, as a JSON object:
the files git tracks and the ones it does not know yet, without the ignored ones; letter case
does not count; a match in the file's own name comes before one in a folder's, shorter paths
first; at most MAX_MATCHES.
usage: search.py <text>"""
import json
import sys

import boxgit
import lavoro

MAX_MATCHES = 200
MAX_QUERY = 200


def rank(path, wanted):
    name = path.rsplit("/", 1)[-1].lower()
    return (0 if wanted in name else 1, len(path), path)


def main():
    if len(sys.argv) != 2 or not sys.argv[1].strip() or len(sys.argv[1]) > MAX_QUERY:
        boxgit.fail("usage: search.py <text of at most %d characters>" % MAX_QUERY, 2)
    wanted = sys.argv[1].lower()
    raw = lavoro.git("ls-files", "-z", "--cached", "--others", "--exclude-standard")
    paths = {lavoro.text(path) for path in raw.split(b"\0") if path}
    matches = sorted((path for path in paths if wanted in path.lower()), key=lambda path: rank(path, wanted))
    print(json.dumps({"query": sys.argv[1], "matches": matches[:MAX_MATCHES], "truncated": len(matches) > MAX_MATCHES}))


main()
