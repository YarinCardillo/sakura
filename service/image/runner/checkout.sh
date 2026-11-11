#!/bin/bash
# Runs in the runner's checkout container: a clean checkout of one commit into the workspace
# directory. The objects come from the verified repository (mounted read-only; the commit was
# imported with every object re-hashed by the extract step) and, through its alternates, from the
# trusted source repository. The workspace is not mounted at all. The checkout must end exactly on
# the requested commit, or the script fails.
# With BOX_SPARSE=1 only the BOX_SUBDIR cone (plus the top-level files) is checked out.
# usage: checkout.sh <sha>
set -euo pipefail

readonly SHA="${1:?usage: checkout.sh <sha>}"
readonly VERIFIED="${BOX_VERIFIED:-/verified}"
readonly TRUSTED="${BOX_TRUSTED:-/trusted}"
readonly WS="${BOX_WS:-/ws}"
. "$(dirname "${BASH_SOURCE[0]}")/gitenv.sh"
box_harden_git

[[ "$SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "not a full commit sha: $SHA" >&2; exit 1; }
git init --quiet --template= "$WS"
{ echo "$VERIFIED/objects"; if [ -d "$TRUSTED/.git/objects" ]; then echo "$TRUSTED/.git/objects"; else echo "$TRUSTED/objects"; fi; } \
  >"$WS/.git/objects/info/alternates"
git "${BOX_GIT_OPTIONS[@]}" -C "$WS" cat-file -e "$SHA^{commit}"
if [ "${BOX_SPARSE:-0}" = "1" ]; then
  git "${BOX_GIT_OPTIONS[@]}" -C "$WS" sparse-checkout init --cone
  git "${BOX_GIT_OPTIONS[@]}" -C "$WS" sparse-checkout set -- "${BOX_SUBDIR:?BOX_SUBDIR is required with BOX_SPARSE=1}"
fi
git "${BOX_GIT_OPTIONS[@]}" -C "$WS" checkout --quiet --detach "$SHA"
[ "$(git "${BOX_GIT_OPTIONS[@]}" -C "$WS" rev-parse HEAD)" = "$SHA" ] || { echo "the checkout is not at $SHA" >&2; exit 1; }
echo "[box] checked out $SHA"
