#!/bin/bash
# Copies package.json and the lockfile of one verified commit out of the verified store into /out
# and prints the hash of the two files (the whole SHA-256, see files_digest.sh). Runs in a verified container (no network, no workspace).
# usage: extract.sh <sha> <subdir, empty for the root>
set -euo pipefail

readonly OUT="${BOX_OUT:-/out}"
[ "$#" -eq 2 ] || { echo "usage: extract.sh <sha> <subdir, empty for the root>" >&2; exit 2; }
readonly SHA="$1"
readonly SUBDIR="$2"
. "$(dirname "${BASH_SOURCE[0]}")/gitenv.sh"
box_harden_git

box_require_verified "$SHA"
for file in package.json package-lock.json; do
  box_verified_git show "$SHA:${SUBDIR:+$SUBDIR/}$file" >"$OUT/$file"
done
echo "files_hash=$("$(dirname "${BASH_SOURCE[0]}")/files_digest.sh" "$OUT/package.json" "$OUT/package-lock.json")"
