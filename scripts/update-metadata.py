#!/usr/bin/env python3
"""Apply optional public update metadata to a bundle's plist, without XML interpolation."""

import base64
import binascii
import os
from pathlib import Path
import plistlib
import sys
from urllib.parse import urlsplit


def update_metadata(path):
    feed = os.environ.get("SAKURA_UPDATE_FEED_URL", "")
    key = os.environ.get("SAKURA_UPDATE_PUBLIC_KEY", "")
    info = plistlib.loads(path.read_bytes())
    for name in ("SUFeedURL", "SUPublicEDKey"):
        info.pop(name, None)
    if feed or key:
        url = urlsplit(feed)
        if url.scheme != "https" or not url.hostname or url.username or url.password or url.fragment:
            sys.exit("Updates require an HTTPS feed URL without credentials or a fragment")
        try:
            valid_key = len(base64.b64decode(key, validate=True)) == 32
        except (ValueError, binascii.Error):
            valid_key = False
        if not valid_key:
            sys.exit("Updates require a base64-encoded 32-byte public EdDSA key")
        info.update(SUFeedURL=feed, SUPublicEDKey=key)
    # The app preference supplies both automatic flags before starting Sparkle.
    info.update(SUEnableSystemProfiling=False, SUAutomaticallyUpdate=False,
                SUAutomaticallyCheckForUpdates=False, SUEnableAutomaticChecks=True)
    path.write_bytes(plistlib.dumps(info))
    print("Bundle configured for updates" if feed else "Bundle built without updates")


if __name__ == "__main__":
    update_metadata(Path(sys.argv[1]))
