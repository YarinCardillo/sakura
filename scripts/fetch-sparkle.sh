#!/bin/bash
# Fetch the official, checksum-pinned Sparkle distribution, including its MIT license.
set -euo pipefail

readonly VERSION=2.10.1
# GitHub's release asset digest, independently checked against the downloaded archive.
readonly SHA256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
readonly URL="https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz"
root="$(cd "$(dirname "$0")/.." && pwd)"
destination="${SAKURA_BUILD_DIR:-$root/build}/sparkle/$VERSION"
mkdir -p "$destination"
archive="$destination/Sparkle-$VERSION.tar.xz"

if [[ ! -f "$archive" ]]; then
  curl -q --fail --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 15 --max-time 180 --output "$archive.part" "$URL"
  mv "$archive.part" "$archive"
fi
actual="$(shasum -a 256 "$archive" | cut -d ' ' -f 1)"
if [[ "$actual" != "$SHA256" ]]; then
  printf 'Sparkle SHA-256 mismatch; archive will not be extracted.\n' >&2
  rm -f "$archive"
  exit 1
fi
stage="$(mktemp -d "$destination/.extract.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
tar -xJf "$archive" -C "$stage" ./Sparkle.framework ./bin ./LICENSE
rm -rf "$destination/Sparkle.framework" "$destination/bin"
mv "$stage/Sparkle.framework" "$stage/bin" "$stage/LICENSE" "$destination/"
printf '%s\n' "$destination"
