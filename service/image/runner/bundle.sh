#!/bin/bash
# Writes a git bundle of the task branch at one verified commit (the commits after the recorded
# base) into /out. Runs in a verified container: the workspace is not mounted, the commit is the
# one the host names, and the bundle is made from the verified store and the trusted source.
# The store is read-only, so the branch name lives in a throwaway repository that borrows their
# objects.
# usage: bundle.sh <task> <base sha> <commit sha>
set -euo pipefail

readonly OUT="${BOX_OUT:-/out}"
readonly TASK="${1:?usage: bundle.sh <task> <base sha> <commit sha>}"
readonly BASE_SHA="${2:?usage: bundle.sh <task> <base sha> <commit sha>}"
readonly TIP="${3:?usage: bundle.sh <task> <base sha> <commit sha>}"
readonly VERIFIED="${BOX_VERIFIED:-/verified}"
readonly TRUSTED="${BOX_TRUSTED:-/trusted}"
. "$(dirname "${BASH_SOURCE[0]}")/gitenv.sh"
box_harden_git

box_require_verified "$TIP"
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/box-bundle.XXXXXX")"
git init --quiet --bare --template= "$SCRATCH"
{ echo "$VERIFIED/objects"; if [ -d "$TRUSTED/.git/objects" ]; then echo "$TRUSTED/.git/objects"; else echo "$TRUSTED/objects"; fi; } \
  >"$SCRATCH/objects/info/alternates"
git "${BOX_GIT_OPTIONS[@]}" --git-dir="$SCRATCH" update-ref "refs/heads/box/$TASK" "$TIP"
git "${BOX_GIT_OPTIONS[@]}" --git-dir="$SCRATCH" bundle create "$OUT/$TASK.bundle" "refs/heads/box/$TASK" "^$BASE_SHA"
rm -rf "$SCRATCH"
