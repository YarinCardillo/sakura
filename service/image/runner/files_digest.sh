#!/bin/bash
# files_digest.sh <file>...: one SHA-256 over the files, whole: 64 hex digits. Each file goes
# in with its length in bytes before it, so the same bytes split another way between the files
# never give the same digest, and nothing of the digest is cut off: an agent that can try about
# 2^24 variants of a file finds two with the same first 12 digits, not with the same 64.
set -euo pipefail
for file in "$@"; do
  printf '%s\n' "$(wc -c <"$file" | tr -d ' ')"
  cat "$file"
done | sha256sum | cut -d' ' -f1
