#!/bin/bash
# SPARSE=1 (the default when SUBDIR is set) limits the working tree to the SUBDIR cone plus the
# top-level files; history, log, diff, take and the runner's checkout stay complete.
. "$(dirname "$0")/lib.sh"
setup_sandbox
mkdir -p "$SOURCE/other" "$SOURCE/sub/deep"
echo outside >"$SOURCE/other/file.txt"
echo top >"$SOURCE/root.txt"
echo inside >"$SOURCE/sub/deep/file.txt"
git -C "$SOURCE" add -A
git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m "more files"
{ cat "$PROJECTS_DIR/sandbox.env"; echo "SPARSE=0"; } >"$PROJECTS_DIR/full.env"

echo " default: sparse"
"$BOX" new sp sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/sp"
assert_file "the SUBDIR is checked out" "$WORKSPACE/sub/deep/file.txt"
assert_file "top-level files are checked out" "$WORKSPACE/root.txt"
assert_no_file "other directories are not" "$WORKSPACE/other/file.txt"
assert_contains "history still has them" "other/file.txt" "$(git -C "$WORKSPACE" ls-tree -r HEAD --name-only)"
assert_eq "no dirty files from the sparse tree" "0" "$("$BOX" facts sp --json | jq -r .dirtyFiles)"

echo " SPARSE=0 keeps the full tree"
"$BOX" new fl full >/dev/null
assert_file "other directories are there" "$AGENTBOX_HOME/work/fl/other/file.txt"

echo " log, diff, take and the bundle are unaffected"
commit_in_task sp work
assert_eq "log has the commit" "1" "$("$BOX" log sp --json | jq length)"
assert_contains "diff of the commit" "+work" "$("$BOX" diff sp --commit "$(workspace_head sp)")"
assert_status "take" 0 "$BOX" take sp
assert_contains "the branch holds the whole tree" "other/file.txt" "$(git -C "$SOURCE" ls-tree -r box/sp --name-only)"

echo " the runner's checkout is sparse too"
SHA="$(workspace_head sp)"
BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$AGENTBOX_HOME/verified/sp.git" BOX_WS="$TMP/ws-sparse" BOX_SPARSE=1 BOX_SUBDIR=sub "$BOX_ROOT/image/runner/checkout.sh" "$SHA" >/dev/null
assert_file "SUBDIR checked out" "$TMP/ws-sparse/sub/deep/file.txt"
assert_file "top-level file checked out" "$TMP/ws-sparse/root.txt"
assert_no_file "other directories absent" "$TMP/ws-sparse/other/file.txt"
BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$AGENTBOX_HOME/verified/sp.git" BOX_WS="$TMP/ws-full" BOX_SPARSE=0 "$BOX_ROOT/image/runner/checkout.sh" "$SHA" >/dev/null
assert_file "SPARSE=0 checks everything out" "$TMP/ws-full/other/file.txt"
finish
