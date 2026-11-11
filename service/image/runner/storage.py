#!/usr/bin/env python3
"""Measure allocated bytes and distinct filesystem entries without reading file contents."""
import json
import os
import sys


def fail_scan(error):
    """Make an unreadable directory fail closed."""
    raise error


def measure(paths, entry_limit):
    """Count allocated blocks and inodes in read-only mounts without following symlinks."""
    seen = set()
    total = 0
    for path in paths:
        for root, directories, files, descriptor in os.fwalk(path, follow_symlinks=False, onerror=fail_scan):
            for name in ["."] + directories + files:
                info = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
                identity = (info.st_dev, info.st_ino)
                if identity in seen:
                    continue
                seen.add(identity)
                total += info.st_blocks * 512
                if len(seen) > entry_limit:
                    return {"bytes": total, "entries": len(seen)}
    return {"bytes": total, "entries": len(seen)}


if __name__ == "__main__":
    try:
        print(json.dumps(measure(sys.argv[2:], int(sys.argv[1]))))
    except (OSError, ValueError):
        sys.exit(1)
