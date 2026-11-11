#!/bin/bash
# The log of a job is written by code the agent controls: what is kept of it is bounded, what
# is parsed from it is bounded and has a deadline, and a log that reached the limit is unknown.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
readonly LOG_LIMIT=4000
readonly CUT_NOTICE_ROOM=400

# run_test <task>: queue, run, print the newest test result file of the task.
run_test() {
  "$BOX" verify "$1" test --no-wait 2>/dev/null
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-test.json | head -1
}

echo " a job that floods its log"
export BOX_MAX_LOG_BYTES="$LOG_LIMIT"
FLOOD="$(head -c 20000 /dev/zero | tr '\0' 'x')"
export FAKE_JOB_SUMMARY="$FLOOD"'\n      Tests  12 passed (12)'
commit_in_task t1 flood
R="$(FAKE_JOB_EXIT=3 run_test t1)"
LOG="${R%.json}.log"
SIZE="$(wc -c <"$LOG" | tr -d ' ')"
assert_eq "the log on disk stays at the limit (got $SIZE bytes)" "1" "$([ "$SIZE" -le $((LOG_LIMIT + CUT_NOTICE_ROOM)) ] && echo 1 || echo 0)"
assert_contains "its last line says it was cut" "[box] log cut at $LOG_LIMIT bytes" "$(tail -n 1 "$LOG")"
assert_eq "the job went on and its exit code is recorded" "3" "$(jq -r .exit_code "$R")"
assert_eq "the counts of a cut log are unknown" "null" "$(jq -c .tests "$R")"
assert_eq "which says nothing about the checks: the counts are no part of that" "false null" "$(jq -r .changesChecks "$R") $(jq -r .checksReason "$R")"

echo " a log under the limit is parsed"
export FAKE_JOB_SUMMARY='      Tests  12 passed (12)'
commit_in_task t1 small
R="$(run_test t1)"
assert_eq "counts are read" "12 0" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
assert_eq "and nothing counts as changed" "false" "$(jq -r .changesChecks "$R")"
SMALL_LOG="${R%.json}.log"
unset FAKE_JOB_SUMMARY BOX_MAX_LOG_BYTES

echo " parsing the log has a deadline"
# counts_with <shell code run after the libraries are loaded>: what tests_json gives for the small log.
counts_with() {
  AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project sandbox journal task request checks; do . "$BOX_ROOT/lib/$lib.sh"; done
    load_task_project t1
    eval "$1"
    tests_json t1 test "$2"' _ "$1" "$SMALL_LOG"
}
assert_eq "within the deadline the counts are read" "12" "$(counts_with ':' | jq -r .ran)"
assert_eq "past the deadline the counts are unknown" "null" "$(counts_with 'run_with_timeout() { return 142; }')"
finish
