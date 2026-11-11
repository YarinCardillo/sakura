#!/bin/bash
# The test counts of a result are what the test command printed, nothing more: the command runs
# code of the commit, so a commit can print any summary line it likes without running a test.
# The result says where the counts come from ("source": "log"), and nothing that tells whether
# a result can be trusted depends on them: a result with counts and one without say the same
# about the checks.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/t1"
# run_test <summary the job prints>: a new commit, its test verb, the newest result file.
run_test() {
  export FAKE_JOB_SUMMARY="$1"
  commit_in_task t1 "change $RANDOM"
  "$BOX" verify t1 test --no-wait 2>/dev/null
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/t1"/*-test.json | head -1
}
trust() {
  jq -c '[.changesChecks, .checksReason, .checksTouched, .testsTouched]' "$1"
}

echo " a command that runs no test and prints one summary line"
FORGED="$(run_test '      Tests  100 passed (100)')"
assert_eq "the counts are recorded as printed" "100 0" "$(jq -r '"\(.tests.ran) \(.tests.skipped)"' "$FORGED")"
assert_eq "and marked as read from the log" "log" "$(jq -r .tests.source "$FORGED")"
assert_eq "box facts carries the mark" "log" "$("$BOX" facts t1 --json | jq -r '.results.test.tests.source')"

echo " a command that prints no summary"
SILENT="$(run_test 'nothing that looks like a summary')"
assert_eq "no counts" "null" "$(jq -c .tests "$SILENT")"

echo " what the two results say about trust is the same"
assert_eq "the checks fields are equal" "$(trust "$SILENT")" "$(trust "$FORGED")"
assert_eq "neither counts as a change to the checks" "false false" "$(jq -r .changesChecks "$FORGED") $(jq -r .changesChecks "$SILENT")"
assert_eq "and neither gives the counts as a reason" "null null" "$(jq -r .checksReason "$FORGED") $(jq -r .checksReason "$SILENT")"

echo " printed counts do not make a commit that changes the checks look clean"
echo '{"x":1}' >"$WORKSPACE/sub/package.json"
git -C "$WORKSPACE" add -A
TOUCHED="$(run_test '      Tests  100 passed (100)')"
assert_eq "changesChecks true" "true" "$(jq -r .changesChecks "$TOUCHED")"
assert_eq "the file is named" '["sub/package.json"]' "$(jq -c .checksTouched "$TOUCHED")"

echo " the interface says so"
assert_eq "docs/interface.md names the source and what the app labels" "1" "$(grep -c 'as the test command reported' "$BOX_ROOT/docs/interface.md")"
finish
