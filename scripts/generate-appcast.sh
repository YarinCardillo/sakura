#!/bin/bash
# Sign the final ZIP with an existing Sparkle account and write appcast.xml beside it, keeping every
# item of the previous feed. The feed of the last release is kept beside the release folders
# (<release-dir>/../appcast.xml) unless SAKURA_PREVIOUS_APPCAST names another; the very first update
# release says so with SAKURA_FIRST_UPDATE_RELEASE=1. Without SAKURA_SPARKLE_ACCOUNT it does nothing.
# It never creates a key.
#
# Usage: scripts/generate-appcast.sh <version> <sparkle-bin-dir> <release-dir> <repository-url>
#   <release-dir> holds Sakura-<version>.zip, already notarized and stapled.
set -euo pipefail
version=$1
tools=$2
out=$3
repository=$4
root="$(cd "$(dirname "$0")/.." && pwd)"
account="${SAKURA_SPARKLE_ACCOUNT:-}"
if [[ -z "$account" ]]; then
  printf 'Skipping update feed: SAKURA_SPARKLE_ACCOUNT is not set.\n'
  exit 0
fi
cache="$(dirname "$out")/appcast.xml"
previous="${SAKURA_PREVIOUS_APPCAST:-$cache}"
if [[ -f "$previous" ]]; then
  python3 "$root/scripts/appcast-history.py" check "$previous" "$version"
  cp "$previous" "$out/.previous-appcast.xml"
  cp "$previous" "$out/appcast.xml"
elif [[ -n ${SAKURA_PREVIOUS_APPCAST:-} || ${SAKURA_FIRST_UPDATE_RELEASE:-0} != 1 ]]; then
  printf 'A previous appcast is required after the first update release. Set SAKURA_PREVIOUS_APPCAST.\n' >&2
  exit 1
fi
zip="$out/Sakura-$version.zip"
signature="$("$tools/sign_update" --account "$account" -p "$zip")"
printf '%s\n' "$signature" > "$out/zip-signature.txt"
"$tools/sign_update" --account "$account" --verify "$zip" "$signature"
args=(--account "$account" --versions "$version" --maximum-versions 0 --maximum-deltas 0
      --download-url-prefix "$repository/releases/download/v$version/")
if [[ "$version" == *-* ]]; then args+=(--channel beta); fi
"$tools/generate_appcast" "${args[@]}" "$out"
if [[ -f "$out/.previous-appcast.xml" ]]; then
  python3 "$root/scripts/appcast-history.py" merge "$out/.previous-appcast.xml" "$out/appcast.xml"
  rm "$out/.previous-appcast.xml"
fi
cp "$out/appcast.xml" "$cache"
