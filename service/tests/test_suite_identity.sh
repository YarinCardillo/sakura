#!/bin/bash
# The base counts of a test result come from the same suite: two profiles can share a repository
# and a base commit and still run different tests (another subdirectory, another command).
. "$(dirname "$0")/lib.sh"
setup_sandbox
mkdir -p "$SOURCE/backend"
echo '{}' >"$SOURCE/backend/package.json"
echo '{"backend":1}' >"$SOURCE/backend/package-lock.json"
git -C "$SOURCE" add -A
git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m backend
cat >"$PROJECTS_DIR/backend.env" <<PROJECT
REPO=$SOURCE
BASE=main
SUBDIR=backend
INSTALL=true
VERB_test=echo backend tests
PROTECTED=package.json package-lock.json
PROJECT

# run_test <task>: queue, run, print the newest test result file of the task.
run_test() {
  "$BOX" verify "$1" test --no-wait 2>/dev/null
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-test.json | head -1
}

echo " the frontend suite records its counts for the base commit"
"$BOX" new fe-base sandbox >/dev/null
export FAKE_JOB_SUMMARY='      Tests  100 passed | 4 skipped (104)'
R="$(run_test fe-base)"
assert_eq "counts recorded at the base" "100 4" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
assert_eq "the result names its suite" "sandbox sub echo test" "$(jq -r '.suite | "\(.profile) \(.subdir) \(.command)"' "$R")"

echo " another profile on the same repository and base does not get them"
"$BOX" new be backend >/dev/null
assert_eq "both tasks start from the same commit" "$(workspace_head fe-base)" "$(workspace_head be)"
commit_in_task be work
export FAKE_JOB_SUMMARY='      Tests  7 passed (7)'
R="$(run_test be)"
assert_eq "the backend ran its own tests" "7 0" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
assert_eq "the frontend counts are not its base" "null null" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"

echo " a task of the same suite does"
"$BOX" new fe sandbox >/dev/null
commit_in_task fe work
export FAKE_JOB_SUMMARY='      Tests  99 passed | 4 skipped (103)'
R="$(run_test fe)"
assert_eq "base is the frontend count" "100 4" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"

echo " once the backend has a base result, the backend gets that one"
"$BOX" new be-base backend >/dev/null
export FAKE_JOB_SUMMARY='      Tests  8 passed (8)'
run_test be-base >/dev/null
commit_in_task be more
export FAKE_JOB_SUMMARY='      Tests  7 passed (7)'
R="$(run_test be)"
assert_eq "base is the backend count" "8 0" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"

echo " a changed test command is another suite"
edit_in_place 's/^VERB_test=.*/VERB_test=echo test --only-some/' "$PROJECTS_DIR/sandbox.env"
"$BOX" new fe-narrow sandbox >/dev/null
commit_in_task fe-narrow narrowed
export FAKE_JOB_SUMMARY='      Tests  3 passed (3)'
R="$(run_test fe-narrow)"
assert_eq "counts of the old command are not the base" "null null" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"
finish
