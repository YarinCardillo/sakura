#!/bin/bash
# The host does not trust work/<task>: unreadable or hostile workspaces are reported, not run.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
"$BOX" new t2 sandbox >/dev/null

echo " status survives a corrupted workspace"
echo "garbage" >"$AGENTBOX_HOME/work/t2/.git/HEAD"
assert_status "status exits 0" 0 "$BOX" status
assert_contains "the readable task is listed" "t1" "$OUTPUT"
assert_contains "the corrupted one is reported" "unreadable workspace" "$OUTPUT"

echo " the host runs git only through the sandbox after new"
: >"$FAKE_DOCKER_LOG"
"$BOX" status >/dev/null 2>&1
assert_contains "status reads facts through a docker run" "/box/facts.py" "$(cat "$FAKE_DOCKER_LOG")"
assert_contains "the workspace mount is read-only" ":/src:ro" "$(cat "$FAKE_DOCKER_LOG")"
assert_contains "the sandbox has no network" "--network none" "$(cat "$FAKE_DOCKER_LOG")"

echo " take goes through a bundle, not the workspace path"
commit_in_task t1 work
: >"$FAKE_DOCKER_LOG"
assert_status "take works" 0 "$BOX" take t1
assert_contains "a bundle was written by a sandbox" "/box/bundle.sh t1" "$(cat "$FAKE_DOCKER_LOG")"
assert_no_file "the operation's private bundle is removed after import" "$AGENTBOX_HOME/bundles/t1.bundle"
assert_eq "the branch arrived" "box/t1" "$(git -C "$SOURCE" branch --list 'box/t1' --format '%(refname:short)')"
assert_eq "the workspace is not a remote of the source" "" "$(git -C "$SOURCE" remote)"

echo " new clones without hardlinks or alternates"
assert_no_file "no alternates" "$AGENTBOX_HOME/work/t1/.git/objects/info/alternates"
assert_eq "no origin remote" "" "$(git -C "$AGENTBOX_HOME/work/t1" remote)"
OBJECT="$(find "$AGENTBOX_HOME/work/t1/.git/objects" -type f -path '*/??/*' | head -1)"
assert_eq "objects are not hardlinked to the source" "1" "$(stat_of %l %h "$OBJECT")"

echo " what a commit changes is computed without trusting the workspace's repository"
W="$AGENTBOX_HOME/work/t1"
BASE="$(sed -n 's/^BASE_SHA=//p' "$AGENTBOX_HOME/tasks/t1.env")"
echo '{"changed":true}' >"$W/sub/package.json"
echo changed >>"$W/sub/existing.spec.ts"
git -C "$W" add -A && git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m "change the checks"
CHANGED="$(workspace_head t1)"
MARKER="$TMP/changes-marker"
# import_into <store> <base> <sha>: what box does before anything reads a commit: a workspace
# container packs it, an import container (no workspace) re-hashes it into the verified store.
import_into() {
  BOX_SRC="$W" "$BOX_ROOT/image/runner/export_pack.py" "$2" "$3" 2>/dev/null \
    | BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$1" "$BOX_ROOT/image/runner/verify_import.py" "$2" "$3" 2>/dev/null
}
# changes_report <base> <sha>: the JSON changes.py prints for the commit, read in a verified
# container (no workspace) after the import, or nothing when either fails.
changes_report() {
  LAST_VERIFIED="$(mktemp -d "$TMP/verified.XXXXXX")"
  import_into "$LAST_VERIFIED" "$1" "$2" || return 0
  BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$LAST_VERIFIED" \
    "$BOX_ROOT/image/runner/changes.py" "$1" "$2" sub "package.json" "**/*.spec.ts" "change.txt" 2>/dev/null
}
assert_reported() {
  local name="$1" report
  report="$(changes_report "$BASE" "$CHANGED")"
  assert_eq "$name: the protected file is reported" '["sub/package.json"]' "$(jq -c .checksTouched <<<"$report")"
  assert_eq "$name: the edited spec is reported" "1" "$(jq .testsTouched.modified <<<"$report")"
}
assert_reported "plain workspace"

git -C "$W" replace "$CHANGED" "$BASE"
assert_eq "(the attack works on plain git: the diff shows nothing)" "" "$(git -C "$W" diff --name-only "$BASE" "$CHANGED")"
assert_reported "a replace object that hides the change"
git -C "$W" replace -d "$CHANGED" >/dev/null

printf '%s %s\n' "$CHANGED" "$BASE" >"$W/.git/info/grafts"
assert_reported "a graft file"
rm -f "$W/.git/info/grafts"

git -C "$W" config diff.external "touch $MARKER"
git -C "$W" config diff.evil.textconv "touch $MARKER"
git -C "$W" config core.fsmonitor "touch $MARKER"
git -C "$W" config core.hooksPath "$W/evil-hooks"
git -C "$W" config core.pager "touch $MARKER"
echo '* diff=evil' >"$W/.git/info/attributes"
assert_reported "a config with an external diff, a textconv and an fsmonitor"
assert_no_file "nothing from the workspace's config ran" "$MARKER"
git -C "$W" config --remove-section diff && git -C "$W" config --remove-section "diff.evil" && git -C "$W" config --unset core.fsmonitor
git -C "$W" config --unset core.hooksPath && git -C "$W" config --unset core.pager
rm -f "$W/.git/info/attributes"

git -C "$W" branch "$BASE" "$CHANGED" 2>/dev/null
assert_reported "a branch named like the base sha that points elsewhere"
git -C "$W" branch -D "$BASE" >/dev/null 2>&1

BLOB="$(git -C "$W" rev-parse "$CHANGED:sub/package.json")"
assert_eq "a base that is not a commit gives no report" "" "$(changes_report "$BLOB" "$CHANGED")"
assert_eq "a checked sha that is not a commit gives no report" "" "$(changes_report "$BASE" "$BLOB")"
assert_eq "a base that is not in the repository gives no report" "" "$(changes_report "$(printf 'b%.0s' {1..40})" "$CHANGED")"

echo " the job checks out exactly the sha it was requested"
CHECKOUT="$TMP/checkout"
git -C "$W" replace "$CHANGED" "$BASE"
mkdir -p "$CHECKOUT"
changes_report "$BASE" "$CHANGED" >/dev/null
BOX_WS="$CHECKOUT" BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$LAST_VERIFIED" "$BOX_ROOT/image/runner/checkout.sh" "$CHANGED" >/dev/null 2>&1
assert_eq "the checked out commit is the requested one" "$CHANGED" "$(git -C "$CHECKOUT" rev-parse HEAD)"
assert_eq "its files are the real ones, not the replaced ones" '{"changed":true}' "$(cat "$CHECKOUT/sub/package.json")"
mkdir -p "$TMP/extract-out"
EXTRACT_VERIFIED="$(mktemp -d "$TMP/verified.XXXXXX")"
import_into "$EXTRACT_VERIFIED" "$BASE" "$CHANGED"
BOX_OUT="$TMP/extract-out" BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$EXTRACT_VERIFIED" \
  "$BOX_ROOT/image/runner/extract.sh" "$CHANGED" sub >/dev/null 2>&1
assert_eq "the package files read for the dependencies are the real ones" '{"changed":true}' "$(cat "$TMP/extract-out/package.json")"
assert_status "a sha that is not a full commit id is refused" 1 env BOX_OUT="$TMP/extract-out" BOX_TRUSTED="$SOURCE" BOX_VERIFIED="$EXTRACT_VERIFIED" "$BOX_ROOT/image/runner/extract.sh" main sub
git -C "$W" replace -d "$CHANGED" >/dev/null

echo " objects the agent forged are never trusted"
# forge_object <repo> <sha of the object to damage> <sha of the object whose content replaces it>
forge_object() {
  local dir="$1/.git/objects" victim="$2" donor="$3"
  chmod u+w "$dir/${victim:0:2}/${victim:2}" 2>/dev/null
  cp "$dir/${donor:0:2}/${donor:2}" "$dir/${victim:0:2}/${victim:2}"
}
BASE_TREE="$(git -C "$W" rev-parse "$BASE^{tree}")"
CHANGED_TREE="$(git -C "$W" rev-parse "$CHANGED^{tree}")"
cp -R "$W" "$TMP/w-intact"

forge_object "$W" "$BASE_TREE" "$CHANGED_TREE"
assert_eq "(plain git reads the forged base tree without noticing: no diff of the protected file)" "0" "$(git -C "$W" diff --name-only "$BASE" "$CHANGED" 2>/dev/null | grep -c 'sub/package.json')"
assert_eq "a base tree forged inside the workspace fails closed (no report)" "" "$(changes_report "$BASE" "$CHANGED")"
rm -rf "$W" && cp -R "$TMP/w-intact" "$W"

forge_object "$W" "$CHANGED" "$BASE"
assert_eq "a forged checked commit gives no report" "" "$(changes_report "$BASE" "$CHANGED")"
queue_directly t1 build 20260401T000000 "$CHANGED"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
FORGED_RESULT="$AGENTBOX_HOME/results/t1/$QUEUED_ID.json"
assert_eq "the forged commit is a failure" "1" "$([ "$(jq .exit_code "$FORGED_RESULT")" -ne 0 ] && echo 1 || echo 0)"
assert_eq "recorded with changesChecks true" "true" "$(jq -r .changesChecks "$FORGED_RESULT")"
assert_eq "no job ran on the forged tree" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_status "and the verified import refuses it" 1 import_into "$(mktemp -d "$TMP/verified.XXXXXX")" "$BASE" "$CHANGED"
rm -rf "$W" && cp -R "$TMP/w-intact" "$W"

git -C "$W" commit-graph write --reachable 2>/dev/null
GRAPH="$W/.git/objects/info/commit-graph"
if [ -f "$GRAPH" ]; then
  chmod u+w "$GRAPH"
  python3 - "$GRAPH" "$(git -C "$W" rev-parse "$CHANGED^{tree}")" "$(git -C "$W" rev-parse "$BASE^{tree}")" <<'PY'
import sys
path, real, fake = sys.argv[1], bytes.fromhex(sys.argv[2]), bytes.fromhex(sys.argv[3])
data = open(path, "rb").read()
open(path, "wb").write(data.replace(real, fake))
PY
  assert_eq "(plain git trusts a forged commit-graph)" "" "$(git -C "$W" diff --name-only "$BASE" "$CHANGED")"
  assert_reported "a commit-graph that swaps the tree of the checked commit"
fi
rm -f "$GRAPH"

git -C "$W" repack -a -d -q 2>/dev/null
git -C "$W" multi-pack-index write 2>/dev/null
chmod -R u+w "$W/.git/objects/pack" 2>/dev/null
MIDX="$W/.git/objects/pack/multi-pack-index"
if [ -f "$MIDX" ]; then
  head -c 4096 /dev/urandom | dd of="$MIDX" bs=1 seek=200 conv=notrunc 2>/dev/null
  assert_reported "a damaged multi-pack-index"
fi
rm -f "$MIDX"
assert_contains "every sandbox git call turns the auxiliary files off" "core.commitGraph=false" "$(grep -h 'HARDENED_OPTIONS' -A5 "$BOX_ROOT/image/runner/boxgit.py")"
assert_contains "gitenv.sh does the same for the shell scripts" "core.multiPackIndex" "$(cat "$BOX_ROOT/image/runner/gitenv.sh")"
finish
