#!/bin/bash
# Downloads the pinned restate-server for this machine, verifies its checksum and unpacks it.
# The engine of the service needs restate-server on the PATH; on a Mac `brew install
# restatedev/tap/restate-server` does the same. The addresses and checksums are those of Restate's
# own Homebrew formula for this version.
#
# Usage: scripts/install-restate-server.sh <folder>     (then add <folder> to the PATH)
set -euo pipefail

readonly VERSION="1.8.1"
readonly BASE_URL="https://restate.gateway.scarf.sh/v$VERSION"
readonly DOWNLOAD_DEADLINE_SECONDS=300

# The archive's checksum for a target triple; empty for a machine Restate publishes nothing for.
checksum_of() {
  case "$1" in
    aarch64-apple-darwin) echo "e2ea3dce01140981fe4d15a09e53718681574d3a290f5566b1025b4a2696a3eb" ;;
    x86_64-apple-darwin) echo "c6c079291b6171ff34358dd94b9315d5c374fc1e2ca96b62573caf8ac047e6c0" ;;
    aarch64-unknown-linux-musl) echo "7d30d4c73a6978f7b4e4a2c08163fbf4e5689c4534af013bb8fd37e003474857" ;;
    x86_64-unknown-linux-musl) echo "56f12ccc54ee227657d4d8b070622849977444425184ee242a33e3ccc28bf745" ;;
    *) echo "" ;;
  esac
}

target_triple() {
  local cpu system
  case "$(uname -m)" in
    arm64 | aarch64) cpu=aarch64 ;;
    *) cpu=x86_64 ;;
  esac
  case "$(uname -s)" in
    Darwin) system=apple-darwin ;;
    *) system=unknown-linux-musl ;;
  esac
  echo "$cpu-$system"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
}

main() {
  [ $# = 1 ] || { sed -n '2,7p' "$0" >&2; exit 64; }
  local folder="$1" triple expected archive actual
  triple="$(target_triple)"
  expected="$(checksum_of "$triple")"
  [ -n "$expected" ] || { echo "install-restate-server: no pinned archive for $triple" >&2; exit 1; }
  mkdir -p "$folder"
  archive="$folder/restate-server-$triple.tar.xz"
  curl -fsSL --max-time "$DOWNLOAD_DEADLINE_SECONDS" -o "$archive" "$BASE_URL/restate-server-$triple.tar.xz"
  actual="$(sha256_of "$archive")"
  if [ "$actual" != "$expected" ]; then
    rm -f "$archive"
    echo "install-restate-server: the checksum of the download is $actual, not $expected" >&2
    exit 1
  fi
  tar -xJf "$archive" -C "$folder" --strip-components 1
  rm -f "$archive"
  [ -x "$folder/restate-server" ] || { echo "install-restate-server: the archive holds no restate-server" >&2; exit 1; }
  echo "$folder/restate-server"
}

main "$@"
