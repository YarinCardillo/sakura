#!/bin/bash
# Sign the embedded Sparkle.framework inside out, in the order Sparkle's documentation gives, with the
# hardened runtime and without --deep. The Downloader keeps its own entitlements. The app itself is
# left to the caller, which signs it last.
#
# Usage: scripts/sign-sparkle.sh <app> <identity>   (identity "-" signs ad hoc, without a timestamp)
set -euo pipefail
app=$1
identity=$2
framework="$app/Contents/Frameworks/Sparkle.framework"
base="$framework/Versions/B"
timestamp=(--timestamp)
if [[ "$identity" == - ]]; then timestamp=(--timestamp=none); fi

sign() {
  codesign --force --sign "$identity" --options runtime "${timestamp[@]}" "$@" < /dev/null
}

sign "$base/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$base/XPCServices/Downloader.xpc"
sign "$base/Autoupdate"
sign "$base/Updater.app"
sign "$framework"
